"""Settings an install made before a change gets when it updates (`bipinstall update`): what a fresh install now
sets itself. Each one is applied once (`~/.local/share/bipagents/migrations.json` keeps the ids done) and can run
again without harm. A Hermes the person had before BipAgents (existing.py) keeps its settings: the update says what
we would change, the person decides.
"""
from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Callable, List, Optional, Tuple

from . import hermes

STATE = Path.home() / ".local" / "share" / "bipagents" / "migrations.json"
REPO = Path(__file__).resolve().parents[2]


class NotYet(Exception):
    """A migration that couldn't be applied this time (safe to show): not marked done, tried at the next update."""


def hermes_configs(home: Path) -> List[Tuple[Path, Optional[str]]]:
    """Every Hermes config of this account: (its HERMES_HOME, its profile — None for the hub's own)."""
    homes = [home / ".hermes"] + sorted((home / ".local" / "share" / "bipagents" / "spaces").glob("*"))
    found: List[Tuple[Path, Optional[str]]] = []
    for root in homes:
        if (root / "config.yaml").is_file():
            found.append((root, None))
        for profile in sorted((root / "profiles").glob("*/config.yaml")):
            found.append((root, profile.parent.name))
    return found


def _set(runner: hermes.Runner, root: Path, profile: Optional[str], key: str, value: str) -> None:
    runner.env = {**runner.env, "HERMES_HOME": str(root)}
    runner.run([hermes.hermes_bin()] + (["-p", profile] if profile else []) + ["config", "set", key, value])


def approvals_manual(runner: hermes.Runner, home: Path) -> str:
    for root, profile in hermes_configs(home):
        _set(runner, root, profile, "approvals.mode", "manual")
    return "risky commands always ask you (approvals.mode manual)"


def local_compression(runner: hermes.Runner, home: Path) -> str:
    local = re.compile(r"^\s+base_url:\s*['\"]?http://", re.M)
    count = 0
    for root, profile in hermes_configs(home):
        config = root / ("profiles/" + profile if profile else "") / "config.yaml"
        if local.search(config.read_text(encoding="utf-8", errors="ignore")):
            _set(runner, root, profile, "compression.threshold_tokens", str(hermes.LOCAL_COMPRESSION_TOKENS))
            count += 1
    return f"discussions with a local model compressed at {hermes.LOCAL_COMPRESSION_TOKENS:,} tokens ({count} agent(s))"


def browsers_on_demand(runner: hermes.Runner, home: Path) -> str:
    """The agents' browsers start when used and stop after 10 minutes unused (bridge-rs/src/browser_mcp.rs): their
    MCP server and units set up again, the gateways restarted to connect to it."""
    units = sorted((home / ".config" / "systemd" / "user").glob("bipagents-browser-*.service"))
    if not units:
        return "no agent browser here"
    configs = [home / ".config" / "bipagents" / "bridge.toml", home / ".config" / "hermes-ios" / "bridge.toml"]
    config = next((path for path in configs if path.is_file()), None)
    if config is None:
        return "agents' browsers left as they are (no bridge config found)"
    from .rustbridge import cli
    runner.run(cli(home, REPO) + ["browser", "add", "--all", "--config", str(config)], cwd=REPO / "bridge")
    from . import update
    update.restart(runner)
    return (f"the agents' browsers ({len(units)}) start when used and stop after 10 minutes unused; gateways "
            "restarted")


def _bridge_config(home: Path) -> Optional[Path]:
    configs = [home / ".config" / "bipagents" / "bridge.toml", home / ".config" / "hermes-ios" / "bridge.toml"]
    return next((path for path in configs if path.is_file()), None)


def with_browser_range(text: str, first: int, count: int) -> str:
    """The bridge config with its [browser] section handing out `count` ports from `first` (the rest kept)."""
    keys = f"first_port = {first}\nports = {count}\n"
    found = re.search(r"^\[browser\]\n((?:(?!\[).*\n?)*)", text, re.M)
    if not found:
        return text.rstrip("\n") + "\n\n[browser]\nenabled = true\n" + keys
    body = re.sub(r"^(first_port|ports)\s*=.*\n?", "", found.group(1), flags=re.M)
    return text[:found.start(1)] + keys + body + text[found.end(1):]


def browser_ports(runner: hermes.Runner, home: Path) -> str:
    """Each install of the machine hands its agents' browsers ports of its own range (ports.browser_ports): two
    accounts (a family, a shared server) gave their first browsers the same ones, and only one could run at a time.
    The range is written in the bridge's config, the browsers set up again (moved into it, their profiles kept) and
    the gateways restarted."""
    from .ports import browser_ports as range_of
    units = sorted((home / ".config" / "systemd" / "user").glob("bipagents-browser-*.service"))
    config = _bridge_config(home)
    if not units or config is None:
        return "no agent browser here"
    text = config.read_text(encoding="utf-8")
    port = re.search(r"^port\s*=\s*(\d+)", text.split("\n[", 1)[0], re.M)   # the bridge's own: its block + 3
    if not port:
        return "agents' browsers left as they are (no bridge port in its config)"
    first, count = range_of(int(port.group(1)) - 3)
    runner.log.append(f"write {config}")
    if not runner.dry:
        config.write_text(with_browser_range(text, first, count), encoding="utf-8")
    from .rustbridge import cli
    runner.run(cli(home, REPO) + ["browser", "add", "--all", "--config", str(config)], cwd=REPO / "bridge")
    from . import update
    update.restart(runner)
    return f"the agents' browsers ({len(units)}) use ports {first} to {first + count - 1}, of this install only"


def isolated_sandboxes(runner: hermes.Runner, home: Path) -> str:
    """The agents' sandboxes isolated (bridge-rs/src/sandbox.rs, 2026-10-22): no folder of this machine mounted in
    them any more (the files the person sends are copied into a volume of the agent's own, shared folders go through
    the bridge's tools), and no internet but the package registries. Hermes then stops asking before harmless work
    there. The bridge does the same at each start; this says what each agent got in the update's log, and restarts
    the gateways so their sandboxes are made again without the old mounts. Linux with podman only."""
    import platform
    import shutil
    if platform.system() != "Linux" or not shutil.which("podman"):
        return "agents' sandboxes left as they are (isolating them needs Linux and podman)"
    config = _bridge_config(home)
    if config is None:
        return "agents' sandboxes left as they are (no bridge config found)"
    from .rustbridge import cli
    out = runner.run(cli(home, REPO) + ["sandbox", "--config", str(config)], cwd=REPO / "bridge")
    from . import update
    update.restart(runner)
    lines = [line.strip() for line in out.splitlines() if line.strip()]
    return "agents' sandboxes isolated, gateways restarted" + ("".join(f"\n  {line}" for line in lines))


def rust_bridge(runner: hermes.Runner, home: Path) -> str:
    """The bridge in Rust only (2026-10-21): the Python bridge left the code with this update, so a service still
    running it is put on this version's binaries (rustbridge.py), restarted, and the agents' browser tools moved to
    its helper. The installer gets its own environment (installer/.venv: the backup timer and the bridge run it with
    that), and the Python bridge's (bridge/.venv) goes. No binaries yet (the release still building, no network):
    nothing changes, the next update tries again. The way back is the Python bridge of BipAgents' history."""
    from . import rustbridge
    from .__main__ import INSTALLER_PYTHON, venv
    said = []
    if rustbridge.python_service(home):
        try:
            rustbridge.ensure(home, REPO, runner)
        except rustbridge.RustBridgeError as exc:
            raise NotYet(f"the bridge stays on Python for now ({exc}); `bipagents update` tries again") from exc
        runner.log.append(f"put the bridge's service on {rustbridge.program(home, REPO)[0]}, restart it")
        if not runner.dry:
            rustbridge.rewrite_service(home, rustbridge.program(home, REPO))
            rustbridge.restart(home)
            rustbridge.browsers_follow(home)
        said.append(f"the bridge now runs in Rust ({rustbridge.key(REPO)})")
    else:
        said.append("the bridge already ran in Rust")
    venv(REPO / "installer", runner)
    backup_unit = home / ".config" / "systemd" / "user" / "bipagents-backup.service"
    if backup_unit.is_file() and "bridge/.venv" in backup_unit.read_text(encoding="utf-8", errors="ignore"):
        from . import backup
        backup.install_timer(runner, REPO, INSTALLER_PYTHON, home)
        said.append("backups run with the installer's own Python")
    old = REPO / "bridge" / ".venv"
    runner.log.append(f"remove {old} and {home / rustbridge.ENGINE}")
    if not runner.dry:
        import shutil
        shutil.rmtree(old, ignore_errors=True)
        (home / rustbridge.ENGINE).unlink(missing_ok=True)
    return "; ".join(said)


# In order; an id never changes once released. rust-bridge comes first: the others run the bridge's command line.
MIGRATIONS: List[Tuple[str, Callable[[hermes.Runner, Path], str]]] = [
    ("2026-10-21-rust-bridge", rust_bridge),
    ("2026-10-15-local-compression", local_compression),
    ("2026-10-16-approvals-manual", approvals_manual),
    ("2026-10-18-browsers-on-demand", browsers_on_demand),
    ("2026-10-19-browser-ports", browser_ports),
    ("2026-10-22-isolated-sandboxes", isolated_sandboxes),
]


def done() -> List[str]:
    try:
        return list(json.loads(STATE.read_text(encoding="utf-8")).get("done", []))
    except (OSError, ValueError):
        return []


def mark(ids: List[str], runner: hermes.Runner) -> None:
    runner.log.append(f"write {STATE}")
    if runner.dry:
        return
    STATE.parent.mkdir(parents=True, exist_ok=True)
    STATE.write_text(json.dumps({"done": sorted(set(done()) | set(ids))}, indent=2) + "\n", encoding="utf-8")


def pending() -> List[str]:
    finished = set(done())
    return [mid for mid, _ in MIGRATIONS if mid not in finished]


def run(runner: hermes.Runner, home: Optional[Path] = None, own_hermes: bool = True) -> List[str]:
    """The pending ones, applied (or, on the person's own Hermes, only described). Returns what each did."""
    home = home or Path.home()
    said: List[str] = []
    for mid, apply in MIGRATIONS:
        if mid not in pending():
            continue
        if not own_hermes:
            said.append(f"{mid}: not applied to your own Hermes (run `install.sh update --apply-to-my-hermes` "
                        "to apply it)")
            continue
        try:
            said.append(apply(runner, home))
        except NotYet as exc:
            said.append(f"{mid}: {exc}")
            continue
        mark([mid], runner)
    return said
