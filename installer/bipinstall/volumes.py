"""The podman volumes that hold an agent's own data, in backups and exports (2026-10-23).

`bipagents sandbox adopt` (bridge-rs/src/sandbox_adopt.rs) moves an agent's working folders (its data, scripts,
plans…) out of this machine's folders into named volumes, `bipagents-<agent>-<name>`: from then on the agent writes
there only, so the hourly backup (backup.py) and `bipagents export` (moving.py) must take them. The bridge lists them
in `<data>/sandbox.json` (`adopted.<agent>.volumes`, `kind: "work"`). Its « linked » folders too (`kind: "link"`,
2026-10-23: a folder programs of this machine keep writing, moved into a volume with a symbolic link left at its path):
the volume is taken here, and the link, at its path, as a link (backup.py and moving.py never follow one), so its
content is counted once.

Each one goes as `podman volume export` writes it (a tar, read as the sandboxes' own « root » of rootless podman: the
files a sandbox's user made, unreadable to this account, are in it too), and comes back with `podman volume import`
into a volume of the same name (made first).

Not taken, on purpose:

- `bipagents-files-<agent>`: a copy of the agent's uploads folder, which is kept (the bridge fills it again);
- `bipagents-envoi-<agent>`: the outbox a sandbox hands files out from, transient (what was handed out is kept by
  the bridge, `sent-files/`);
- adopt's code volumes (`kind: "code"`): refreshed from their folder at each bridge start;
- the shared folders' volumes, `bipagents-share-<agent>` (bridge-rs/src/shares.rs): the bridge keeps their content in
  step with `<data>/shares/<agent>/`, which is kept, and fills a fresh volume from it by itself (a move, a restore).
"""
from __future__ import annotations

import json
import re
import shutil
import subprocess
from pathlib import Path
from typing import Callable, List, Optional

NAME = re.compile(r"^bipagents-[a-z0-9][a-z0-9_.-]{0,120}$")
SUFFIX = ".tar"

Run = Callable[..., subprocess.CompletedProcess]


def podman() -> Optional[str]:
    return shutil.which("podman")


def kept(data_dir: Path) -> List[str]:
    """The volumes whose content lives nowhere else: adopt's working folders and linked folders."""
    try:
        state = json.loads((data_dir / "sandbox.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return []
    adopted = state.get("adopted") if isinstance(state, dict) else None
    names = set()
    for entry in adopted.values() if isinstance(adopted, dict) else []:
        listed = entry.get("volumes") if isinstance(entry, dict) else None
        for volume in listed if isinstance(listed, list) else []:
            if isinstance(volume, dict) and volume.get("kind") in ("work", "link") and NAME.match(str(volume.get("name", ""))):
                names.add(str(volume["name"]))
    return sorted(names)


def _run(args: List[str], timeout: float = 3600) -> subprocess.CompletedProcess:
    return subprocess.run(args, capture_output=True, text=True, timeout=timeout)


def export_all(names: List[str], folder: Path, engine: Optional[str], run: Run = _run) -> List[Path]:
    """Each volume as `<folder>/<name>.tar` (the tars of volumes no longer kept removed). Without podman, or for a
    volume podman can't export (removed by hand), nothing for it."""
    folder.mkdir(parents=True, exist_ok=True)
    for old in folder.glob(f"*{SUFFIX}"):
        if old.name[:-len(SUFFIX)] not in names:
            old.unlink(missing_ok=True)
    if not engine:
        return []
    made = []
    for name in names:
        target = folder / f"{name}{SUFFIX}"
        partial = folder / f".{name}{SUFFIX}.part"
        done = run([engine, "volume", "export", name, "--output", str(partial)])
        if done.returncode != 0 or not partial.exists():
            partial.unlink(missing_ok=True)
            continue
        partial.chmod(0o600)
        partial.replace(target)
        made.append(target)
    return made


def import_all(folder: Path, engine: Optional[str], run: Run = _run) -> List[str]:
    """The tars of `folder` into volumes of the same names (made when missing). Returns the volumes filled."""
    if not engine or not folder.is_dir():
        return []
    filled = []
    for tar in sorted(folder.glob(f"*{SUFFIX}")):
        name = tar.name[:-len(SUFFIX)]
        if not NAME.match(name):
            continue
        if run([engine, "volume", "create", "--ignore", name]).returncode != 0:
            continue
        if run([engine, "volume", "import", name, str(tar)]).returncode == 0:
            filled.append(name)
    return filled
