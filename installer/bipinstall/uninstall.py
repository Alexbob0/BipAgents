"""``python -m bipinstall uninstall [--all] [--dry-run]``: removes what the installer set up for this account.

By default the agents' data stays (Hermes in ~/.hermes: conversations, memory, files) so a reinstall finds them;
`--all` removes it too, after a confirmation. The voices' server is shared by every account of the machine: it is
only removed when this account runs it."""
from __future__ import annotations

import os
import re
import shutil
from pathlib import Path
from typing import List, Optional

from .detect import detect
from .hermes import Runner, hermes_bin, hermes_home

SERVICES = {"macos": ["io.github.bipagents.bridge", "io.github.bipagents.pocket", "io.github.bipagents.hermes"],
            "linux": ["bipagents-bridge", "bipagents-pocket", "bipagents-hermes"]}


def service_path(system: str, label: str, home: Path) -> Path:
    if system == "macos":
        return home / "Library" / "LaunchAgents" / f"{label}.plist"
    return home / ".config" / "systemd" / "user" / f"{label}.service"


def install_ports(config: Path) -> List[int]:
    """The ports this install published on the tailnet, from its bridge config (bridge = block + 3)."""
    try:
        text = config.read_text(encoding="utf-8")
    except OSError:
        return []
    match = re.search(r"^port\s*=\s*(\d+)", text, flags=re.M)
    if not match:
        return []
    base = int(match.group(1)) - 3
    return [base + 2, base + 3, base + 5]   # agents, bridge, desk


def wait_until_stopped(seconds: float = 30) -> None:
    """The gateway writes its shutdown state on the way out: removing its folder before that races with it."""
    import subprocess
    import time
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        found = subprocess.run(["pgrep", "-u", str(os.getuid()), "-f", "hermes.*gateway"], capture_output=True)
        if found.returncode != 0:
            return
        time.sleep(1)


def remove(path: Path, runner: Runner) -> None:
    if not path.exists() and not path.is_symlink():
        return
    runner.log.append(f"remove {path}")
    if runner.dry:
        return
    if path.is_dir() and not path.is_symlink():
        for _ in range(3):   # a file written while removing (a process shutting down): try again
            try:
                shutil.rmtree(path)
                return
            except OSError:
                import time
                time.sleep(1)
        shutil.rmtree(path)
    else:
        path.unlink()


def uninstall(runner: Runner, *, everything: bool, home: Optional[Path] = None, system: Optional[str] = None,
              tailscale: Optional[str] = None) -> None:
    home = home or Path.home()
    machine = None if system else detect()
    system = system or machine.system
    tailscale = tailscale or (machine.tailscale if machine else None)
    config = home / ".config" / "bipagents" / "bridge.toml"

    # 1. Off the tailnet first, while the config still says which ports are ours.
    if tailscale:
        for port in install_ports(config):
            runner.run([tailscale, "serve", f"--https={port}", "off"], check=False)

    # 2. The bridge and the voices' services.
    for label in SERVICES[system]:
        path = service_path(system, label, home)
        if not path.exists():
            continue
        if system == "macos":
            runner.run(["launchctl", "bootout", f"gui/{os.getuid()}/{label}"], check=False)
        else:
            runner.run(["systemctl", "--user", "disable", "--now", label], check=False)
        remove(path, runner)
    if system == "linux":
        runner.run(["systemctl", "--user", "daemon-reload"], check=False)
        runner.run(["systemctl", "--user", "reset-failed"] + SERVICES["linux"], check=False)

    # The agents' visible browsers: their containers, then their image.
    if system == "linux":
        units = home / ".config" / "systemd" / "user"
        browsers = sorted(units.glob("bipagents-browser-*.service")) if units.is_dir() else []
        for unit in browsers:
            runner.run(["systemctl", "--user", "disable", "--now", unit.stem], check=False)
            remove(unit, runner)
        engine = shutil.which("podman") or shutil.which("docker")
        if browsers and engine:
            runner.run([engine, "image", "rm", "-f", "bipagents-browser"], check=False)
            runner.run(["systemctl", "--user", "daemon-reload"], check=False)

    # 3. A Hermes that was there before BipAgents: only our settings are undone, its gateway keeps running.
    from .existing import RECORD, restore
    preexisting = restore(runner, config.parent / RECORD)
    if preexisting and preexisting.get("gateway") == "service" and Path(hermes_bin()).exists():
        runner.run([hermes_bin(), "gateway", "restart"], check=False)   # its own service, without our settings
    #    Otherwise Hermes' gateway service (Hermes removes its own unit / plist).
    elif not preexisting and Path(hermes_bin()).exists():
        runner.run([hermes_bin(), "gateway", "stop"], check=False)
        runner.run([hermes_bin(), "gateway", "uninstall"], check=False)
        if not runner.dry:
            wait_until_stopped()

    # 4. The bridge's config, keys, certificate and outbox.
    remove(home / ".config" / "bipagents", runner)
    remove(home / ".local" / "share" / "bipagents", runner)

    # 5. Hermes itself and the agents' data, only when asked, and never a Hermes that was there before BipAgents.
    if everything and preexisting:
        runner.log.append(f"keep {hermes_home()} (Hermes was there before BipAgents)")
    elif everything:
        remove(hermes_home(), runner)
        for wrapper in ("hermes", "hermes-acp", "hermes-agent"):
            remove(home / ".local" / "bin" / wrapper, runner)
        remove(home / ".cache" / "huggingface" / "hub" / "models--kyutai--pocket-tts-without-voice-cloning", runner)
