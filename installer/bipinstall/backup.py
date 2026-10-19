"""Hourly backups of a place of the all-included offer, to the guichet (guichet/bipguichet/backups.py), with restic.

Everything `bipagents export` would take (moving.py's inventory: Hermes' settings and keys, each agent's memory,
conversations, skills and files, the bridge's settings and the phones' keys, the browsers' logins, without caches),
encrypted on the place with a key made here (`password`) and kept by the person's phones (the bridge hands it to its
app: GET /v1/backup). The guichet only stores what it can't read; restic sends what changed since the last backup.

    ~/.config/bipagents/backup.json         {url, token, password} (chmod 600): the guichet's address and this
                                            place's token (from its setup), the key
    ~/.local/state/bipagents/backup.json    {last_ok, last_error, snapshot, files, bytes, pruned}: the seat agent
                                            tells the guichet `last_ok` only

Nothing is backed up before the app has claimed the place (nothing of anyone is here yet). SQLite databases are
copied consistently first (a gateway may be writing), into the stage folder, and the copies go instead of the live
files. Each day, the history is thinned: every hour for two days, every day for a month, every week for three
months. A place moved after an outage gets the same repository and its phone gives it the key back (restore)."""
from __future__ import annotations

import json
import os
import secrets
import sqlite3
import subprocess
import time
from pathlib import Path
from types import SimpleNamespace
from typing import Dict, Iterator, List, Optional, Tuple

SETTINGS = Path(".config") / "bipagents" / "backup.json"
STATE = Path(".local") / "state" / "bipagents" / "backup.json"
STAGE = Path(".local") / "state" / "bipagents" / "backup-stage"
HOST = "place"                      # the same on every server the place lives on: restic finds its last backup
KEEP = ["--keep-hourly", "48", "--keep-daily", "30", "--keep-weekly", "12"]
PRUNE_EVERY = 86400
SQLITE = b"SQLite format 3\x00"
UNIT = "bipagents-backup"


class BackupError(Exception):
    """Safe to show (no key in it)."""


def home() -> Path:
    return Path(os.environ.get("HOME") or Path.home())


def read_json(path: Path) -> Dict:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def write_private(path: Path, data: Dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        json.dump(data, fh)
    os.replace(tmp, path)


def settings(base: Optional[Path] = None) -> Dict:
    return read_json((base or home()) / SETTINGS)


def configure(url: str, token: str, base: Optional[Path] = None) -> Dict:
    """At install (`--cloud`): where the backups go and this place's token, with a new key. An earlier key is kept
    (the same repository needs the same key)."""
    path = (base or home()) / SETTINGS
    data = read_json(path)
    data.update({"url": url, "token": token})
    data.setdefault("password", secrets.token_urlsafe(32))
    write_private(path, data)
    return data


def set_password(password: str, base: Optional[Path] = None) -> None:
    """The key a phone gave back after a move: this place now writes to its person's repository with it."""
    path = (base or home()) / SETTINGS
    data = read_json(path)
    if not data.get("url"):
        raise BackupError("this place has no backups set up")
    data["password"] = password
    write_private(path, data)


def repository(data: Dict) -> str:
    """restic's REST URL, the token as the password of its basic authentication."""
    url = str(data["url"])
    if not url.startswith(("https://", "http://")):
        return url   # a local repository (tests)
    scheme, _, rest = url.partition("://")
    return f"rest:{scheme}://place:{data['token']}@{rest}"


def environment(data: Dict, base: Path) -> Dict[str, str]:
    """Secrets go to restic through its environment (only this account and root can read it), never its arguments."""
    env = dict(os.environ)
    env.update({"RESTIC_REPOSITORY": repository(data), "RESTIC_PASSWORD": str(data["password"]),
                "RESTIC_CACHE_DIR": str(base / ".cache" / "restic"), "RESTIC_PROGRESS_FPS": "0.016"})
    return env


def claimed(base: Path) -> bool:
    """The app has paired this place (the bridge deleted its pairing secret): there is someone's data to keep."""
    return not (base / ".config" / "bipagents" / "setup.json").exists()


def _is_sqlite(path: Path) -> bool:
    try:
        with open(path, "rb") as fh:
            return fh.read(16) == SQLITE
    except OSError:
        return False


def _signature(path: Path) -> List[int]:
    found = []
    for item in (path, path.with_name(path.name + "-wal")):
        try:
            st = item.stat()
            found += [st.st_mtime_ns, st.st_size]
        except OSError:
            found += [0, 0]
    return found


def sqlite_copy(path: Path, stage: Path, seen: Dict[str, List[int]]) -> Optional[Path]:
    """A consistent copy of a database in the stage (SQLite's backup), made again only when it changed. None when it
    can't be read (then the file goes as it is)."""
    target = stage / "sqlite" / str(path).lstrip("/")
    signature = _signature(path)
    if target.exists() and seen.get(str(path)) == signature:
        return target
    target.parent.mkdir(parents=True, exist_ok=True)
    partial = target.with_name(target.name + ".part")
    partial.unlink(missing_ok=True)
    try:
        source = sqlite3.connect(f"file:{path}?mode=ro", uri=True, timeout=10)
        copy = sqlite3.connect(partial)
        with copy:
            source.backup(copy)
        source.close()
        copy.close()
    except sqlite3.Error:
        partial.unlink(missing_ok=True)
        return None
    os.replace(partial, target)
    seen[str(path)] = signature
    return target


def files(inv, stage: Path, seen: Dict[str, List[int]]) -> Iterator[Path]:
    """What goes: moving.py's roots, minus what it skips (caches, runtimes, sockets), the databases' copies instead
    of the databases, and the manifest."""
    yield stage / "manifest.json"
    for root in inv.roots().values():
        if root.is_file():
            entries: Iterator[Path] = iter([root])
        elif root.is_dir():
            entries = _walk(inv, root)
        else:
            continue
        for path in entries:
            if path.is_symlink():
                yield path
                continue
            if not path.is_file():
                continue   # sockets, pipes
            if _is_sqlite(path):
                yield sqlite_copy(path, stage, seen) or path
            else:
                yield path


def _walk(inv, root: Path) -> Iterator[Path]:
    for folder, dirs, names in os.walk(root, followlinks=False):
        here = Path(folder)
        dirs[:] = sorted(d for d in dirs if not inv.skipped(here / d))
        for name in sorted(names):
            path = here / name
            if not inv.skipped(path) and os.access(path, os.R_OK, follow_symlinks=False):
                yield path


def restic(args: List[str], env: Dict[str, str], timeout: float = 3600) -> subprocess.CompletedProcess:
    return subprocess.run(["restic", *args], env=env, capture_output=True, text=True, timeout=timeout)


def _initialized(env: Dict[str, str]) -> bool:
    found = restic(["cat", "config"], env, timeout=120)
    if found.returncode == 0:
        return True
    if found.returncode == 10 or "does not exist" in found.stderr or "Is there a repository" in found.stderr:
        return False
    raise BackupError("the guichet doesn't answer: " + _short(found.stderr))


def _short(text: str) -> str:
    lines = [line for line in text.strip().splitlines() if line.strip()]
    return (lines[-1] if lines else "no output")[:300]


def run(base: Optional[Path] = None, config: Optional[Path] = None, now: Optional[float] = None) -> Dict:
    """One backup (and the history thinned once a day). Returns the state written."""
    from . import moving
    base = base or home()
    data = settings(base)
    state_path = base / STATE
    state = read_json(state_path)
    if not data.get("url") or not data.get("token") or not data.get("password"):
        raise BackupError("this place has no backups set up")
    if not claimed(base):
        state["skipped"] = "not paired yet"
        return state
    now = time.time() if now is None else now
    env = environment(data, base)
    try:
        if not _initialized(env):
            made = restic(["init"], env, timeout=300)
            if made.returncode != 0:
                raise BackupError("the repository couldn't be made: " + _short(made.stderr))
        inv = moving.read_inventory(config or base / ".config" / "bipagents" / "bridge.toml", base)
        stage = base / STAGE
        stage.mkdir(parents=True, exist_ok=True)
        seen = {k: v for k, v in read_json(stage / "seen.json").items() if isinstance(v, list)}
        machine = SimpleNamespace(system="linux" if os.uname().sysname == "Linux" else "macos", tailscale=None,
                                  tailnet_name=None)
        manifest = moving.manifest(inv, machine)
        manifest["stage"] = str(stage)
        write_private(stage / "manifest.json", manifest)
        listed = list(files(inv, stage, seen))
        write_private(stage / "seen.json", seen)
        listing = stage / "files"
        with open(listing, "wb") as fh:
            for path in listed:
                fh.write(os.fsencode(str(path)) + b"\0")
        done = restic(["backup", "--files-from-raw", str(listing), "--host", HOST, "--tag", "bipagents", "--json",
                       "--quiet"], env)
        if done.returncode not in (0, 3):   # 3: some files couldn't be read (written by a sandbox): the rest is kept
            raise BackupError("the backup failed: " + _short(done.stderr))
        summary = _summary(done.stdout)
        state.update({"last_ok": now, "snapshot": summary.get("snapshot_id"),
                      "files": summary.get("total_files_processed"), "bytes": summary.get("total_bytes_processed"),
                      "added": summary.get("data_added")})
        state.pop("last_error", None)
        if now - float(state.get("pruned") or 0) >= PRUNE_EVERY:
            thinned = restic(["forget", "--host", HOST, "--prune", *KEEP], env)
            if thinned.returncode == 0:
                state["pruned"] = now
    except BackupError as exc:
        state["last_error"] = str(exc)
        state["failed_at"] = now
        write_private(state_path, state)
        raise
    write_private(state_path, state)
    return state


def _summary(output: str) -> Dict:
    for line in reversed(output.strip().splitlines()):
        try:
            item = json.loads(line)
        except ValueError:
            continue
        if isinstance(item, dict) and item.get("message_type") == "summary":
            return item
    return {}


def units(repo: Path, python: Path) -> List[Tuple[str, str]]:
    """The systemd user service and its hourly timer (spread over the hour: four places on a server don't all back
    up at once), at the lowest priority."""
    service = ("[Unit]\nDescription=BipAgents: this place's backup to the guichet\nAfter=network-online.target\n\n"
               "[Service]\nType=oneshot\nNice=19\nIOSchedulingClass=idle\n"
               f"Environment=PYTHONPATH={repo / 'installer'}\nWorkingDirectory={repo}\n"
               f"ExecStart={python} -m bipinstall backup --quiet\n")
    timer = ("[Unit]\nDescription=BipAgents: back this place up every hour\n\n[Timer]\nOnCalendar=hourly\n"
             "RandomizedDelaySec=50min\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n")
    return [(f"{UNIT}.service", service), (f"{UNIT}.timer", timer)]


def install_timer(runner, repo: Path, python: Path, base: Optional[Path] = None) -> None:
    folder = (base or home()) / ".config" / "systemd" / "user"
    for name, text in units(repo, python):
        runner.log.append(f"write {folder / name}")
        if not runner.dry:
            folder.mkdir(parents=True, exist_ok=True)
            (folder / name).write_text(text, encoding="utf-8")
    runner.run(["systemctl", "--user", "daemon-reload"], check=False)
    runner.run(["systemctl", "--user", "enable", "--now", f"{UNIT}.timer"], check=False)


def main(argv: List[str]) -> int:
    import argparse
    parser = argparse.ArgumentParser(prog="bipinstall backup", description="Back this place up to the guichet now "
                                     "(the hourly timer runs it)")
    parser.add_argument("--quiet", action="store_true", help="say nothing when it worked (the timer)")
    args = parser.parse_args(argv)
    try:
        state = run()
    except BackupError as exc:
        print(f"Backup failed: {exc}", flush=True)
        return 1
    if not args.quiet:
        if state.get("skipped"):
            print(f"Nothing backed up: {state['skipped']}.")
        else:
            print(f"Backed up: {state.get('files')} files, {(state.get('added') or 0) / 2**20:.1f} MB new.")
    return 0
