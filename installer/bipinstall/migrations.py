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
    """The agents' browsers start when used and stop after 10 minutes unused (bridge/bipbridge/browser_mcp.py): their
    MCP server and units set up again, the gateways restarted to connect to it."""
    units = sorted((home / ".config" / "systemd" / "user").glob("bipagents-browser-*.service"))
    if not units:
        return "no agent browser here"
    configs = [home / ".config" / "bipagents" / "bridge.toml", home / ".config" / "hermes-ios" / "bridge.toml"]
    config = next((path for path in configs if path.is_file()), None)
    if config is None:
        return "agents' browsers left as they are (no bridge config found)"
    repo = Path(__file__).resolve().parents[2]
    runner.run([str(repo / "bridge" / ".venv" / "bin" / "python"), "-m", "bipbridge", "browser", "add", "--all",
                "--config", str(config)], cwd=repo / "bridge")
    from . import update
    update.restart(runner)
    return (f"the agents' browsers ({len(units)}) start when used and stop after 10 minutes unused; gateways "
            "restarted")


# In order; an id never changes once released.
MIGRATIONS: List[Tuple[str, Callable[[hermes.Runner, Path], str]]] = [
    ("2026-10-15-local-compression", local_compression),
    ("2026-10-16-approvals-manual", approvals_manual),
    ("2026-10-18-browsers-on-demand", browsers_on_demand),
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
        said.append(apply(runner, home))
        mark([mid], runner)
    return said
