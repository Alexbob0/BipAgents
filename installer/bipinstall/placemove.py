"""A planned move of a place of the offer to another server, the place's side (decided 2026-10-26).

The servers' seat agents run the move as root (installer/cloud/place-move.py): the source copies the place to the
target with rsync over a one-time SSH channel while its agents work, drains it in a quiet moment (the bridge:
bridge-rs/src/place_move.rs), stops it, sends the last changes and the target starts it with the same keys. What
only the place itself knows is done here, as its account:

    bipinstall move-out plan      (source) what to copy: moving.py's roots and rsync filter rules that leave out what
                                  backups and exports leave out (Hermes' code, caches, runtimes); the manifest the
                                  target needs to adjust the settings, in the stage folder
    bipinstall move-out stop      (source) its services stopped and kept from starting (`bipagents resume` undoes it),
                                  its sandboxes stopped, the agents' own podman volumes exported into the stage
    bipinstall move-in prepare    (target, a fresh place) what it keeps of its own (door, ports, backup token) noted,
                                  its services stopped while the person's files come over it
    bipinstall move-in finish     (target) the settings moved to this place (as a restore does: backup.settle), the
                                  volumes imported, the migrations run, the services started again

Each prints one line of JSON (the seat agent reads it). Unlike a restore after an outage, nothing needs the person's
phone: the files come from the living place, the backup key with them.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Dict, List, Optional

from . import moving

STAGE = Path(".local") / "state" / "bipagents" / "move-stage"        # the source's manifest and volumes
MOVE_IN = Path(".local") / "state" / "bipagents" / "move-in"         # the target's notes and the stage's copy
CONFIG = Path(".config") / "bipagents" / "bridge.toml"
# Copied, unlike in backups: a stopped place's SQLite write-ahead logs hold its last writes.
KEEP_SIDECARS = True


def home() -> Path:
    return Path(os.environ.get("HOME") or Path.home())


def _write(path: Path, data: Dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        json.dump(data, fh)
    os.replace(tmp, path)


def rules(inv: moving.Inventory, root: Path) -> List[str]:
    """rsync filter rules for one root, the same choices as `Inventory.skipped` (backups, exports) but for the
    SQLite sidecars: `-wal` and `-journal` go (the last copy is made with the place stopped), `-shm` doesn't."""
    found = [f"- {name}" for name in sorted(moving.RUNTIME_FILES)] + ["- *.sock", "- *-shm"]
    if not KEEP_SIDECARS:
        found += ["- *-wal", "- *-journal"]

    def prefix(inner: Path) -> Optional[str]:
        parts = moving._relative(inner, root)
        if parts is None:
            return None
        return "/" + "/".join(parts) + "/" if parts else "/"

    for hermes_home in inv.hermes_homes():
        at = prefix(hermes_home)
        if at is None:
            continue
        found += [f"- {at}{name}" for name in sorted(moving.HERMES_SKIP_ROOT)]
        found += [f"- {at}profiles/*/{name}" for name in sorted(moving.HERMES_SKIP_PROFILE)]
    at = prefix(inv.data_dir)
    if at is not None:
        found += [f"- {at}{name}" for name in sorted(moving.DATA_SKIP)]
    at = prefix(inv.browsers_root)
    if at is not None:
        for name in sorted(moving.CHROME_SKIP):
            found += [f"- {at}*/profile/{name}", f"- {at}*/profile/**/{name}"]
    return found


def plan(base: Optional[Path] = None) -> Dict:
    """What the source's seat agent copies, and the manifest (in the stage, copied last)."""
    base = base or home()
    inv = moving.read_inventory(base / CONFIG, base)
    stage = base / STAGE
    stage.mkdir(parents=True, exist_ok=True)
    from types import SimpleNamespace
    from . import volumes
    machine = SimpleNamespace(system="linux" if os.uname().sysname == "Linux" else "macos", tailscale=None,
                              tailnet_name=None)
    info = moving.manifest(inv, machine)
    info["stage"] = str(stage)
    info["volumes"] = volumes.kept(inv.data_dir)
    _write(stage / "manifest.json", info)
    roots = inv.roots()
    return {"home": str(base), "stage": str(stage),
            "roots": {name: str(path) for name, path in roots.items()},
            "dirs": {name: path.is_dir() for name, path in roots.items()},
            "rules": {name: rules(inv, path) for name, path in roots.items() if path.is_dir()},
            "volumes": info["volumes"], "bridge": {"port": inv.toml.get("port", 8643),
                                                   "host": inv.toml.get("host", "127.0.0.1")}}


def stop(runner, base: Optional[Path] = None) -> Dict:
    """The source stopped for the last copy: its services (noted for `bipagents resume`), its sandboxes, then the
    agents' own volumes exported into the stage."""
    base = base or home()
    found = moving.services(base, "linux" if os.uname().sysname == "Linux" else "macos")
    moving.stop(runner, found, base)
    from . import volumes
    tool = volumes.podman()
    if tool:
        runner.run([tool, "stop", "--all", "--time", "5"], check=False)
    inv = moving.read_inventory(base / CONFIG, base)
    names = volumes.kept(inv.data_dir)
    exported = volumes.export_all(names, base / STAGE / "volumes", tool) if names else []
    return {"stopped": [name for _, name in found], "volumes": [p.name for p in exported]}


def prepare(runner, base: Optional[Path] = None) -> Dict:
    """The target, a fresh place: what it keeps of its own, then its services stopped (they come back in `finish`)."""
    from . import backup
    base = base or home()
    notes = base / MOVE_IN / "fresh.json"
    if notes.exists():   # asked again (the source tried again): the first notes are the fresh place's
        return {"prepared": True, "again": True}
    fresh = backup.fresh_state(base)
    found = moving.services(base, "linux" if os.uname().sysname == "Linux" else "macos")
    fresh["services"] = [list(item) for item in found]
    _write(notes, fresh)
    moving.stop(runner, found, base)
    return {"prepared": True, "ports": fresh["ports"]}


def finish(runner, base: Optional[Path] = None, repo: Optional[Path] = None, address: Optional[str] = None) -> Dict:
    """The target, once the person's files are here: settings moved to this place, volumes in podman, migrations,
    services started (the bridge last: it comes back with the person's keys and certificate)."""
    from . import backup, migrations
    base = base or home()
    repo = repo or moving.REPO
    folder = base / MOVE_IN
    fresh = json.loads((folder / "fresh.json").read_text(encoding="utf-8"))
    info = json.loads((folder / "stage" / "manifest.json").read_text(encoding="utf-8"))
    backup.settle(info, base, repo, address, fresh)
    config = base / ".config" / "bipagents"
    for name in (moving.MOVED, "existing-hermes.json", "setup.json", "restoring.json"):
        (config / name).unlink(missing_ok=True)   # the source's notes of its own stop; claimed: no pairing secret
    staged = base / moving.VOLUMES_STAGE
    exported = folder / "stage" / "volumes"
    filled: List[str] = []
    if exported.is_dir() and any(exported.iterdir()):
        staged.mkdir(parents=True, exist_ok=True)
        for tar in exported.iterdir():
            os.replace(tar, staged / tar.name)
        filled = moving.import_volumes(base)
    for line in migrations.run(runner):
        runner.log.append(line)
    from .family import bridge_restart
    services = [tuple(item) for item in fresh.get("services", [])]
    for how, name in services:
        if how == "systemd" and not name.startswith("bipagents-browser-") and "bridge" not in name:
            runner.run(["systemctl", "--user", "enable", "--now", name], check=False)
    for how, name in services:
        if how == "systemd" and "bridge" in name:
            runner.run(["systemctl", "--user", "enable", name], check=False)
    runner.run(bridge_restart(base), check=False)
    if info.get("browsers"):
        from .rustbridge import cli
        runner.run(cli(base, repo) + ["browser", "add", "--all", "--config", str(config / "bridge.toml")],
                   cwd=repo / "bridge", check=False)
    return {"finished": True, "volumes": filled}


def main(argv: List[str]) -> int:
    import argparse
    from . import hermes
    parser = argparse.ArgumentParser(prog="bipinstall move-out|move-in", description="A planned move of this place "
                                     "to another server (run by the servers' seat agents)")
    parser.add_argument("side", choices=["move-out", "move-in"])
    parser.add_argument("step", choices=["plan", "stop", "prepare", "finish"])
    args = parser.parse_args(argv)
    runner = hermes.Runner()
    try:
        if args.side == "move-out" and args.step == "plan":
            out = plan()
        elif args.side == "move-out" and args.step == "stop":
            out = stop(runner)
        elif args.side == "move-in" and args.step == "prepare":
            out = prepare(runner)
        elif args.side == "move-in" and args.step == "finish":
            from . import cloud
            out = finish(runner, address=cloud.public_address())
        else:
            parser.error(f"{args.side} has no step {args.step}")
            return 2
    except (OSError, ValueError, KeyError) as exc:
        print(json.dumps({"error": f"{type(exc).__name__}: {exc}"}), flush=True)
        return 1
    print(json.dumps(out), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
