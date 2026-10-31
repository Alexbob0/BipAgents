"""Root's own tools on a server of the all-included offer (/usr/local/lib/bipagents: the seat agent, add-seat.sh,
place-move.py...), brought to the version of root's BipAgents checkout (/usr/local/lib/bipagents/repo).

The first-boot script fetches them once (the guichet's guichet-rs/src/bootstrap.rs) and nothing replaced them after:
a server installed before a change of the seat agent kept the old one for its whole life (2026-10-26). Root's checkout
moves to each BipAgents version the guichet's update waves bring (seat-agent.py: prepare_shared), so its tools follow
it there: refreshed by `shared_images pull`, which every seat agent since 2026-10-27 runs from that checkout as root
(and each night), and by the seat agent itself. Only ever forward: a wave's way back to an older commit leaves the
newer tools in place (they read what the older places say), and a checkout that isn't a descendant of the tools'
commit changes nothing. ASCII only, like the files it copies.
"""
from __future__ import annotations

import os
import subprocess
from pathlib import Path
from typing import List, Optional

LIB = Path("/usr/local/lib/bipagents")
# Fetched by every first boot (bootstrap.rs) and replaced whatever: the others only where a server already has them.
ALWAYS = ("seat-agent.py", "add-seat.sh")
FILES = ALWAYS + ("place-move.py", "nightly-reboot.sh", "space-hardening.sh")
MARK = "tools-commit"


def _git(repo: Path, *args: str) -> Optional[str]:
    try:
        done = subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True, timeout=120)
    except (OSError, subprocess.SubprocessError):
        return None
    return done.stdout.strip() if done.returncode == 0 else None


def forward(repo: Path, old: Optional[str], new: str) -> bool:
    """Whether the checkout's commit `new` is `old` itself or after it (unknown `old`: the first refresh)."""
    if not old:
        return True
    if old == new:
        return True
    try:
        done = subprocess.run(["git", "-C", str(repo), "merge-base", "--is-ancestor", old, new], capture_output=True,
                              timeout=120)
    except (OSError, subprocess.SubprocessError):
        return False
    return done.returncode == 0


def refresh(repo: Path, lib: Path = LIB, root_only: bool = True) -> List[str]:
    """The tools at `lib` replaced by the checkout's own when it is newer; returns the files replaced."""
    if root_only and os.geteuid() != 0:
        return []
    head = _git(repo, "rev-parse", "HEAD")
    if not head or not (repo / "installer" / "cloud" / "seat-agent.py").is_file():
        return []
    mark = lib / MARK
    try:
        old = mark.read_text().strip() or None
    except OSError:
        old = None
    if old == head or not forward(repo, old, head):
        return []
    replaced = []
    for name in FILES:
        source, target = repo / "installer" / "cloud" / name, lib / name
        if not source.is_file() or (name not in ALWAYS and not target.exists()):
            continue
        data = source.read_bytes()
        try:
            if target.read_bytes() == data:
                continue
        except OSError:
            pass
        lib.mkdir(parents=True, exist_ok=True)
        tmp = target.with_name(target.name + ".tmp")
        tmp.write_bytes(data)
        os.chmod(tmp, 0o755)
        os.replace(tmp, target)
        replaced.append(name)
    tmp = mark.with_name(MARK + ".tmp")
    tmp.write_text(head + "\n")
    os.replace(tmp, mark)
    return replaced


def refresh_here() -> List[str]:
    """`refresh` for the checkout this module runs from, when it is root's (never a place's own BipAgents)."""
    repo = Path(__file__).resolve().parents[2]
    if repo != (LIB / "repo").resolve():
        return []
    return refresh(repo)
