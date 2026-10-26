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
    of the databases, the agents' own podman volumes as `podman volume export` writes them (volumes.py: counted in
    the place's used space like the rest), and the manifest."""
    from . import volumes
    yield stage / "manifest.json"
    yield from volumes.export_all(volumes.kept(inv.data_dir), stage / "volumes", volumes.podman())
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


# What a person's agents install for themselves, never backed up (made again on first use) but counted in their 5 GB:
# MCP servers' packages (npx, uvx), pip and npm installs into their own account.
PERSONAL_RUNTIMES = (".npm", ".cache/uv", ".cache/pip", ".local/share/uv/tools", ".local/lib", ".local/share/pnpm")


def personal_runtime_bytes(inv, base: Path) -> int:
    """Those folders, and with the server's shared Hermes (shared_hermes.py) what Hermes added for this person's
    agents in their HERMES_HOMEs (its writable store and Python environments: an extra, a plugin's packages). With
    a Hermes of its own, those folders are that Hermes itself and don't count."""
    from .shared_hermes import size
    paths = [base / rel for rel in PERSONAL_RUNTIMES if (base / rel).is_dir()]
    if (inv.hermes_home / "shared").is_symlink():
        for hermes_home in inv.hermes_homes():
            paths += [hermes_home / name for name in ("tools", "installs")
                      if (hermes_home / name).is_dir() and not (hermes_home / name).is_symlink()]
    return size(paths)


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
        from . import volumes
        manifest["volumes"] = volumes.kept(inv.data_dir)
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
        runtimes = personal_runtime_bytes(inv, base)
        backed = summary.get("total_bytes_processed")
        # The place's used space (the bridge's /v1/backup used_bytes, against its 5 GB): what was backed up, and
        # what its agents installed for themselves.
        state.update({"last_ok": now, "snapshot": summary.get("snapshot_id"),
                      "files": summary.get("total_files_processed"),
                      "bytes": (backed + runtimes) if isinstance(backed, int) else backed,
                      "backed_bytes": backed, "runtime_bytes": runtimes, "added": summary.get("data_added")})
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


# -- restore: a place moved after an outage gets its person's last backup back ---------------------------------------

RESTORE_TMP = Path(".local") / "state" / "bipagents" / "restore"
MARKER = Path(".config") / "bipagents" / "restoring.json"   # the bridge says « restoring » while it is here
STEPS = ("Fetching your last backup", "Putting your agents back", "Adjusting them to this server",
         "Restarting your agents")


def progress(base: Path, step: int, done: bool = False, error: Optional[str] = None) -> None:
    """What the app shows (the bridge reads the install's progress file: GET /v1/setup)."""
    label = error or (STEPS[step - 1] if 1 <= step <= len(STEPS) else "Ready")
    write_private(base / ".config" / "bipagents" / "setup-progress.json",
                  {"step": step, "total": len(STEPS), "label": label, "done": done, "restoring": not done,
                   "error": error})


def fetch(base: Path, key: str) -> Path:
    """The last backup, into a folder of this account, with the key the phone gave back. The key becomes this
    place's (its backups go on into the same repository)."""
    data = dict(settings(base), password=key)
    if not data.get("url"):
        raise BackupError("this place has no backups set up")
    env = environment(data, base)
    opened = restic(["cat", "config"], env, timeout=120)
    if opened.returncode != 0:
        wrong = "wrong password" in opened.stderr or opened.returncode == 12
        raise BackupError("this key doesn't open these backups" if wrong
                          else "the backups can't be reached: " + _short(opened.stderr))
    restic(["unlock"], env, timeout=120)   # locks the dead server left behind
    target = base / RESTORE_TMP
    if target.exists():
        import shutil
        shutil.rmtree(target)
    target.mkdir(parents=True)
    got = restic(["restore", "latest", "--host", HOST, "--target", str(target)], env)
    if got.returncode != 0:
        raise BackupError("the last backup couldn't be fetched: " + _short(got.stderr))
    set_password(key, base)
    return target


def _manifest(fetched: Path) -> Tuple[Dict, Path]:
    found = sorted(fetched.glob("**/.local/state/bipagents/backup-stage/manifest.json"))
    if not found:
        raise BackupError("the backup holds no manifest")
    return json.loads(found[0].read_text(encoding="utf-8")), found[0].parent


def _inside(fetched: Path, path: str) -> Path:
    return fetched / path.lstrip("/")


def _copy(src: Path, dst: Path) -> None:
    import shutil
    if src.is_symlink():
        dst.parent.mkdir(parents=True, exist_ok=True)
        if dst.is_symlink() or dst.exists():
            dst.unlink()
        os.symlink(os.readlink(src), dst)
    elif src.is_dir():
        shutil.copytree(src, dst, symlinks=True, dirs_exist_ok=True)
    elif src.is_file():
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst)


FRESH_WINS = (("", "public_url"), ("browser", "first_port"), ("browser", "ports"), ("lan", "enabled"),
              ("lan", "port"), ("lan", "address"), ("setup", "file"), ("setup", "progress"))


def _toml_value(text: str, section: str, key: str) -> Optional[str]:
    """A setting's raw value (as written) in a section ("": before the first one)."""
    import re
    inside = section == ""
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            inside = stripped == f"[{section}]"
            continue
        found = re.match(rf"\s*{re.escape(key)}\s*=\s*(.+?)\s*$", line) if inside else None
        if found:
            return found.group(1)
    return None


def _set_toml(text: str, section: str, key: str, value: Optional[str]) -> str:
    from . import moving
    if section:
        return moving.set_in_section(text, section, key, value)
    lines = text.splitlines()
    end = next((i for i, line in enumerate(lines) if line.lstrip().startswith("[")), len(lines))
    at = next((i for i in range(end) if lines[i].split("=", 1)[0].strip() == key), None)
    if at is not None:
        if value is None:
            del lines[at]
        else:
            lines[at] = f"{key} = {value}"
    elif value is not None:
        lines.insert(end, f"{key} = {value}")
    return "\n".join(lines) + "\n"


def fresh_state(base: Path) -> Dict:
    """What this place keeps of its own fresh install when another place's files come over it: its door, address,
    browser ports and setup files (FRESH_WINS, as written), its local ports and its backup settings (this seat's
    token). JSON-ready: a planned move (placemove.py) keeps it in a file while the files come."""
    from . import moving
    config = base / ".config" / "bipagents" / "bridge.toml"
    text = config.read_text(encoding="utf-8")
    return {"own": [[section, key, _toml_value(text, section, key)] for section, key in FRESH_WINS],
            "ports": moving.read_inventory(config, base).ports(), "backup": settings(base)}


def put_back(fetched: Path, base: Path, repo: Path, address: Optional[str]) -> Dict:
    """The backup's files in this account, over the fresh install, then its settings moved to this place: home and
    local ports (each port once: two blocks may overlap); this place's own door, address, browser ports, setup files
    and backup token stay as the fresh install made them. Returns the backup's manifest."""
    from . import moving
    info, stage = _manifest(fetched)
    fresh = fresh_state(base)
    target = moving.Target(home=base, repo=repo, tailnet_name=None, lan_address=address)
    for old in info["roots"].values():
        source = _inside(fetched, old)
        if source.exists() or source.is_symlink():
            _copy(source, moving.rehome(old, info, target))
    sqlite_root = _inside(fetched, info.get("stage") or str(stage)) / "sqlite"
    if sqlite_root.is_dir():
        for copy in sqlite_root.rglob("*"):
            if not copy.is_file():
                continue
            dest = moving.rehome("/" + str(copy.relative_to(sqlite_root)), info, target)
            for sidecar in ("-wal", "-shm", "-journal"):
                dest.with_name(dest.name + sidecar).unlink(missing_ok=True)
            _copy(copy, dest)
    # The agents' own podman volumes, under their names (volumes.py).
    from . import volumes
    exported = _inside(fetched, info.get("stage") or str(stage)) / "volumes"
    if exported.is_dir():
        staged = base / moving.VOLUMES_STAGE
        staged.mkdir(parents=True, exist_ok=True)
        for tar in exported.glob(f"*{volumes.SUFFIX}"):
            _copy(tar, staged / tar.name)
        info["volumes_restored"] = moving.import_volumes(base)
    settle(info, base, repo, address, fresh)
    return info


def settle(info: Dict, base: Path, repo: Path, address: Optional[str], fresh: Dict) -> None:
    """Once another place's files are here (a restore, a planned move): its settings moved to this place (home,
    local ports: first to marks, then to this place's, each port once), this place's own values (`fresh_state`) put
    back, the person's backup key kept with this seat's token."""
    from . import moving
    import re
    import tomllib
    config = base / ".config" / "bipagents" / "bridge.toml"
    new_ports = fresh["ports"]
    own_backup = fresh.get("backup") or {}
    target = moving.Target(home=base, repo=repo, tailnet_name=None, lan_address=address)
    old_toml = tomllib.loads(config.read_text(encoding="utf-8"))
    mapping = {int(info["ports"][k]): int(new_ports[k]) for k in ("hub", "bridge", "desk")
               if k in info["ports"] and int(info["ports"][k]) != int(new_ports[k])}
    roots = [moving.rehome(path, info, target) for path in info["roots"].values()]
    marks = {old: f"BIPAGENTSPORT{i}X" for i, old in enumerate(mapping)}   # first to marks, then to the new ports
    moving.rewrite(moving.settings_files(roots), moving.replacements(info, moving.Target(
        home=base, repo=repo, tailnet_name=None, lan_address=address, ports=marks)))
    moving.rewrite(moving.settings_files(roots), [(re.compile(re.escape(mark)), str(mapping[old]))
                                                  for old, mark in marks.items()])
    moving.adjust_bridge(config, info, target, old_toml)
    text = config.read_text(encoding="utf-8")
    for section, key, value in fresh["own"]:
        text = _set_toml(text, section, key, value)
    config.write_text(text, encoding="utf-8")
    os.chmod(config, 0o600)
    if own_backup.get("url"):   # this seat's token, the person's key
        write_private(base / SETTINGS, {**own_backup, "password": settings(base).get("password")
                                        or own_backup.get("password")})


def restore(key: str, runner, base: Optional[Path] = None, repo: Optional[Path] = None,
            address: Optional[str] = None) -> Dict:
    """The whole restore, as the bridge starts it (a unit of its own, so restarting the bridge doesn't stop it)."""
    from .__main__ import REPO
    from .update import gateway_units
    base = base or home()
    repo = repo or REPO
    try:
        progress(base, 1)
        fetched = fetch(base, key)
        progress(base, 2)
        for how, name in gateway_units():
            if how == "systemd":
                runner.run(["systemctl", "--user", "stop", name], check=False)
        info = put_back(fetched, base, repo, address)
        progress(base, 3)
        from . import migrations
        for line in migrations.run(runner):
            runner.log.append(line)
        progress(base, 4)
        for how, name in gateway_units():
            if how == "systemd":
                runner.run(["systemctl", "--user", "start", name], check=False)
        from .rustbridge import cli
        config = base / ".config" / "bipagents" / "bridge.toml"
        if info.get("browsers"):
            runner.run(cli(base, repo) + ["browser", "add", "--all", "--config", str(config)], cwd=repo / "bridge",
                       check=False)
    except BackupError as exc:
        progress(base, 0, error=str(exc))
        (base / MARKER).unlink(missing_ok=True)
        raise
    import shutil
    shutil.rmtree(base / RESTORE_TMP, ignore_errors=True)
    (base / ".config" / "bipagents" / "setup.json").unlink(missing_ok=True)   # claimed: the pairing secret goes
    (base / MARKER).unlink(missing_ok=True)
    progress(base, len(STEPS), done=True)
    from .family import bridge_restart
    runner.run(bridge_restart(base), check=False)   # last: it comes back with the restored keys and certificate
    return info


def restore_main(argv: List[str]) -> int:
    import argparse
    from . import cloud, hermes
    parser = argparse.ArgumentParser(prog="bipinstall restore", description="Put this place's person's last backup "
                                     "back here (after their server died); the bridge runs it with the key from the app")
    parser.add_argument("--key-file", required=True, help="a file holding the backup key (deleted once read)")
    args = parser.parse_args(argv)
    path = Path(args.key_file)
    key = path.read_text(encoding="utf-8").strip()
    path.unlink(missing_ok=True)
    try:
        restore(key, hermes.Runner(), address=cloud.public_address())
    except BackupError as exc:
        print(f"Restore failed: {exc}", flush=True)
        return 1
    print("Restored.", flush=True)
    return 0


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
