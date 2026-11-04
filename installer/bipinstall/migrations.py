"""Settings an install made before a change gets when it updates (`bipinstall update`): what a fresh install now
sets itself. Each one is applied once (`~/.local/share/bipagents/migrations.json` keeps the ids done) and can run
again without harm. A Hermes the person had before BipAgents (existing.py) keeps its settings: the update says what
we would change, the person decides.
"""
from __future__ import annotations

import json
from pathlib import Path
from typing import Callable, List, Optional, Tuple

from . import hermes

STATE = Path.home() / ".local" / "share" / "bipagents" / "migrations.json"
REPO = Path(__file__).resolve().parents[2]


class NotYet(Exception):
    """A migration that couldn't be applied this time (safe to show): not marked done, tried at the next update."""


def is_place(home: Path) -> bool:
    """A place of the all-included offer (its account is the person's alone: its caches are ours to empty)."""
    return (home / ".config" / "bipagents" / "setup.json").is_file()


GATEWAY_UNITS = ("hermes-gateway.service", "bipagents-hermes.service")
SANDBOXES_DROPIN = "60-bipagents-sandboxes.conf"
SANDBOXES_TEXT = """# BipAgents (installer/bipinstall/migrations.py): the agents' sandboxes this gateway started die with it (their
# fuse-overlayfs mounts live in its cgroup) and Hermes would keep the dead containers: every terminal and file tool then
# fails (« transport endpoint is not connected », bench of 2026-10-31). Removed before it starts; Hermes makes them
# again at their next use (the agents' files are in their volumes, kept).
[Service]
ExecStartPre=-/bin/sh -c 'podman ps -a --format "{{.Names}}" | grep "^hermes-" | xargs -r podman rm -f >/dev/null 2>&1; exit 0'
"""


def sandboxes_at_gateway_start(runner: hermes.Runner, home: Path) -> str:
    """The gateway removes the agents' dead sandboxes before it starts (2026-10-31): a drop-in for each name its unit
    may have (written even before the unit exists: systemd reads it when it does). Linux only."""
    import sys
    if sys.platform == "darwin":
        return "macOS: the gateway's sandboxes left as they are"
    units = home / ".config" / "systemd" / "user"
    for unit in GATEWAY_UNITS:
        path = units / (unit + ".d") / SANDBOXES_DROPIN
        runner.log.append(f"write {path}")
        if not runner.dry:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(SANDBOXES_TEXT, encoding="utf-8")
    runner.run(["systemctl", "--user", "daemon-reload"], check=False)
    return "the gateway removes the agents' dead sandboxes before it starts (from its next start)"


# In order; an id never changes once released. The ones from 2026-10-15 to 2026-11-02 left on 2026-11-04 (every
# install had them from that day's release): their code is in git history.
MIGRATIONS: List[Tuple[str, Callable[[hermes.Runner, Path], str]]] = []


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
