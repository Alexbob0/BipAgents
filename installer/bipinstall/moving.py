"""Moving an install to another machine, or keeping a copy of it.

`bipagents export` writes into one encrypted file (sealed.py) everything this account's agents need: Hermes' settings
and keys, each agent's personality, memory, skills, conversations, routines and files, the bridge (the phones' keys
and notification tokens, the vault, shared data, the family's links) and the agents' browser profiles (their
logins). Not the programs: `install.sh import <file>` on the other machine (Linux or a Mac, another user name, another
processor) installs BipAgents and Hermes in the same versions, puts everything back, adjusts paths, ports and the
machine's name, starts it all and checks the agents answer.

`bipagents export --moving` also stops this install once the copy is made (no routine runs twice, no bot is polled from
two places); `bipagents resume` starts it again here if the move is called off.

Nothing goes through a third party: the file goes from one machine to the other as the person copies it.
"""
from __future__ import annotations

import argparse
import datetime
import getpass
import io
import json
import os
import platform
import re
import shutil
import socket
import sqlite3
import subprocess
import tarfile
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, Iterator, List, Optional, Tuple

from . import hermes, ui
from .sealed import SealedError, SealedReader, SealedWriter, new_passphrase

REPO = Path(__file__).resolve().parents[2]
FORMAT = 1
MANIFEST = "manifest.json"
SUMMARY = "summary.json"
SQLITE = b"SQLite format 3\x00"
SMALL = 64 * 1024 * 1024        # read whole before archiving: a file growing meanwhile cannot break the archive
MOVED = "moved.json"            # in the config folder: what `--moving` stopped, for `bipagents resume`

# A HERMES_HOME (the hub's, each space's): Hermes' code and runtimes (installed again for the target's processor),
# caches, logs and what a running gateway or sandbox leaves, at its root and in each profile.
HERMES_SKIP_ROOT = {"hermes-agent", "tools", "models", "runtimes", "node", "cache", "logs", "checkpoints", "backups",
                    "state-snapshots", "sandboxes", "bot-desktop"}
HERMES_SKIP_PROFILE = {"models", "runtimes", "node", "cache", "logs", "checkpoints", "sandboxes", "bot-desktop"}
RUNTIME_FILES = {"gateway.pid", "gateway.lock", "gateway.sock", "gateway_state.json", hermes.UNFINISHED, "busy"}
SIDECARS = ("-wal", "-shm", "-journal")
# A browser's Chrome profile: caches and locks only.
CHROME_SKIP = {"Cache", "Code Cache", "GPUCache", "ShaderCache", "GrShaderCache", "DawnCache", "DawnGraphiteCache",
               "DawnWebGPUCache", "Crashpad", "SingletonLock", "SingletonSocket", "SingletonCookie"}
# Settings rewritten for the target (paths, ports, names); conversations, files and browser profiles stay as they are.
TEXT_SUFFIXES = {".toml", ".yaml", ".yml", ".json", ".env", ".conf", ".txt", ".service", ".plist", ".sh"}
TEXT_NAMES = {".env", "relay-clients"}
# The bridge's derived data, made again after a restore or a move: the archives' search indexes (bridge-rs/src/archives/).
DATA_SKIP = {"archive-index"}
NOT_SETTINGS = {"profile", "uploads", "media", "audio", "agent-audio", "image_store", "image_cache", "audio_cache",
                "shares", "sessions", "output", "skills", "memories"}


def say(text: str = "") -> None:
    print(text, flush=True)


# -- what an install is made of ---------------------------------------------------------------------------------------

@dataclass
class Inventory:
    """One account's install, read from its bridge config."""
    config: Path
    toml: Dict[str, Any]
    home: Path

    @property
    def data_dir(self) -> Path:
        return Path(self.toml.get("data_dir") or self.home / ".local" / "share" / "hermes-bridge").expanduser()

    @property
    def hermes_home(self) -> Path:
        return Path((self.toml.get("hermes") or {}).get("home") or self.home / ".hermes").expanduser()

    @property
    def spaces_root(self) -> Path:
        return Path((self.toml.get("spaces") or {}).get("root") or self.data_dir / "spaces").expanduser()

    @property
    def browsers_root(self) -> Path:
        return Path((self.toml.get("browser") or {}).get("root") or self.data_dir / "browsers").expanduser()

    def agents(self) -> Dict[str, Dict[str, Any]]:
        return {name: table for name, table in (self.toml.get("agents") or {}).items() if isinstance(table, dict)}

    def hermes_homes(self) -> List[Path]:
        homes = [self.hermes_home]
        homes += sorted(p for p in self.spaces_root.glob("*") if p.is_dir()) if self.spaces_root.is_dir() else []
        return homes

    def roots(self) -> Dict[str, Path]:
        """What is archived, by name: folders, then files the config points to outside them (an APNs key…)."""
        found: Dict[str, Path] = {"config": self.config.parent, "data": self.data_dir, "hermes": self.hermes_home}
        outbox = (self.toml.get("outbox") or {}).get("db_path")
        if outbox and not _inside(Path(outbox).expanduser().parent, list(found.values())):
            found["outbox"] = Path(outbox).expanduser().parent
        for extra in (self.spaces_root, self.browsers_root):
            if extra.exists() and not _inside(extra, list(found.values())):
                found[extra.name] = extra
        loose = sorted({Path(v).expanduser() for v in _file_settings(self.toml)
                        if Path(v).expanduser().is_file() and not _inside(Path(v).expanduser(), list(found.values()))})
        for i, path in enumerate(loose):
            found[f"file-{i}"] = path
        return found

    def ports(self) -> Dict[str, int]:
        bridge = int(self.toml.get("port", 8643))
        hub = _port(((self.toml.get("hermes") or {}).get("hub_url")) or "") or bridge - 1
        lan = int((self.toml.get("lan") or {}).get("port", bridge + 1))
        return {"hub": hub, "bridge": bridge, "lan": lan, "desk": bridge + 2}

    def skipped(self, path: Path) -> bool:
        name = path.name
        if name in RUNTIME_FILES or name.endswith(".sock") or name.endswith(SIDECARS):
            return True
        for home in self.hermes_homes():
            parts = _relative(path, home)
            if parts is None:
                continue
            if parts and parts[0] in HERMES_SKIP_ROOT:
                return True
            if len(parts) >= 3 and parts[0] == "profiles" and parts[2] in HERMES_SKIP_PROFILE:
                return True
        parts = _relative(path, self.data_dir)
        if parts and parts[0] in DATA_SKIP:
            return True
        parts = _relative(path, self.browsers_root)
        if parts and len(parts) >= 3 and parts[1] == "profile" and any(p in CHROME_SKIP for p in parts[2:]):
            return True
        return False


def _relative(path: Path, root: Path) -> Optional[Tuple[str, ...]]:
    try:
        return path.relative_to(root).parts
    except ValueError:
        return None


def _inside(path: Path, roots: List[Path]) -> bool:
    return any(_relative(path, root) is not None for root in roots)


def _port(url: str) -> Optional[int]:
    found = re.search(r":(\d+)(?:/|$)", url)
    return int(found.group(1)) if found else None


def _file_settings(table: Any) -> Iterator[str]:
    """Every `*_file` / `*_path` / `service_account` value of the config: files it reads (keys, passwords)."""
    if isinstance(table, dict):
        for key, value in table.items():
            if isinstance(value, str) and (key.endswith(("_file", "_path")) or key == "service_account"):
                yield value
            else:
                yield from _file_settings(value)


def read_inventory(config: Path, home: Optional[Path] = None) -> Inventory:
    import tomllib
    return Inventory(config, tomllib.loads(config.read_text(encoding="utf-8")), home or Path.home())


def _git_head(folder: Path) -> Optional[str]:
    try:
        out = subprocess.run(["git", "-C", str(folder), "rev-parse", "HEAD"], capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return None
    return out.stdout.strip() or None


def published_ports(tailscale: Optional[str]) -> Optional[List[int]]:
    """The ports this machine publishes on its tailnet (`tailscale serve status`), None when it cannot tell."""
    if not tailscale:
        return None
    try:
        out = subprocess.run([tailscale, "serve", "status", "--json"], capture_output=True, text=True, timeout=10)
        return sorted(int(p) for p in (json.loads(out.stdout or "{}").get("TCP") or {}))
    except (OSError, ValueError, subprocess.SubprocessError):
        return None


def manifest(inv: Inventory, machine: Any) -> Dict[str, Any]:
    import yaml
    try:
        hub = yaml.safe_load((inv.hermes_home / "config.yaml").read_text(encoding="utf-8")) or {}
    except (OSError, yaml.YAMLError):
        hub = {}
    ports = inv.ports()
    published = published_ports(getattr(machine, "tailscale", None))
    ours = [p for p in (published or [ports["hub"], ports["bridge"]]) if p in ports.values()]
    agents = []
    for name, table in inv.agents().items():
        agents.append({"name": name, "display_name": table.get("display_name", name), "space": bool(table.get("space")),
                       "space_port": _port(table.get("hermes_url", "")) if table.get("space") else None,
                       "browser": bool(table.get("desktop_vnc")) and (inv.browsers_root / name).is_dir()})
    hermes_t = inv.toml.get("hermes") or {}
    return {
        "format": FORMAT,
        "created": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "bipagents": _git_head(REPO),
        "hermes_commit": _git_head(hermes.hermes_checkout()),
        "source": {"user": os.environ.get("USER", ""), "home": str(inv.home), "host": socket.gethostname(),
                   "system": getattr(machine, "system", platform.system().lower()), "arch": platform.machine(),
                   "tailnet_name": getattr(machine, "tailnet_name", None), "repo": str(REPO),
                   "lan_address": (inv.toml.get("lan") or {}).get("address")},
        "roots": {name: str(path) for name, path in inv.roots().items()},
        "ports": ports,
        "published": ours,
        "isolated": bool(hermes_t.get("isolated")),
        "sandbox": hermes.sandbox_runtime(),
        "browsers": any(a["browser"] for a in agents),
        "desktop": bool((hub.get("bot_desktop") or {}).get("auto_start")),
        "model_url": (hub.get("model") or {}).get("base_url"),
        "agents": agents,
    }


# -- export -----------------------------------------------------------------------------------------------------------

@dataclass
class Tally:
    files: int = 0
    size: int = 0
    skipped: List[str] = field(default_factory=list)


def _info(tar: tarfile.TarFile, path: Path, arcname: str) -> tarfile.TarInfo:
    info = tar.gettarinfo(str(path), arcname)
    if info.type == tarfile.LNKTYPE:   # a hard link: stored as the file it is
        info.type, info.linkname = tarfile.REGTYPE, ""
    info.uid = info.gid = 0
    info.uname = info.gname = ""
    return info


def _sqlite_copy(path: Path, folder: Path) -> Optional[Path]:
    """A consistent copy of a SQLite database, even while a gateway writes to it (SQLite's backup)."""
    target = folder / "copy.db"
    target.unlink(missing_ok=True)
    try:
        source = sqlite3.connect(f"file:{path}?mode=ro", uri=True, timeout=10)
        copy = sqlite3.connect(target)
        with copy:
            source.backup(copy)
        source.close()
        copy.close()
        return target
    except sqlite3.Error:
        return None   # locked by its program (a running Chrome): the file as it is


def _is_sqlite(path: Path) -> bool:
    try:
        with open(path, "rb") as handle:
            return handle.read(16) == SQLITE
    except OSError:
        return False


def add_tree(tar: tarfile.TarFile, inv: Inventory, name: str, root: Path, tally: Tally, scratch: Path) -> None:
    if root.is_file():
        entries: List[Path] = [root]
    elif root.is_dir():
        entries = [root]
        for folder, dirs, files in os.walk(root, followlinks=False):
            here = Path(folder)
            dirs[:] = sorted(d for d in dirs if not inv.skipped(here / d))
            entries += [here / d for d in dirs] + [here / f for f in sorted(files) if not inv.skipped(here / f)]
    else:
        return
    for path in entries:
        rel = path.relative_to(root).as_posix() if path != root else ""
        arcname = f"{name}/{rel}" if rel else name
        try:
            if path.is_symlink() or path.is_dir():
                tar.addfile(_info(tar, path, arcname))
                continue
            if not path.is_file():
                continue   # sockets, pipes, devices
            info = _info(tar, path, arcname)
            source: Optional[Path] = path
            if _is_sqlite(path):
                source = _sqlite_copy(path, scratch) or path
            if source.stat().st_size <= SMALL:
                data = source.read_bytes()
                info.size = len(data)
                tar.addfile(info, io.BytesIO(data))
            else:
                info.size = source.stat().st_size
                with open(source, "rb") as handle:
                    tar.addfile(info, handle)
            tally.files += 1
            tally.size += info.size
        except PermissionError:
            tally.skipped.append(str(path))   # e.g. written by a sandbox under another user id


def export(inv: Inventory, out: Path, passphrase: str, machine: Any,
           extra: Optional[Dict[str, Any]] = None) -> Tuple[Dict[str, Any], Tally]:
    """`extra`: more in the manifest (an export for a place says so: to_place.py)."""
    info = {**manifest(inv, machine), **(extra or {})}
    tally = Tally()
    out.parent.mkdir(parents=True, exist_ok=True)
    partial = out.with_name(out.name + ".part")
    with open(partial, "wb") as raw, tempfile.TemporaryDirectory() as scratch:
        os.chmod(partial, 0o600)
        writer = SealedWriter(raw, passphrase)
        with tarfile.open(fileobj=writer, mode="w|gz", format=tarfile.PAX_FORMAT) as tar:
            data = json.dumps(info, indent=2).encode()
            entry = tarfile.TarInfo(MANIFEST)
            entry.size, entry.mode = len(data), 0o600
            tar.addfile(entry, io.BytesIO(data))
            for name, root in info["roots"].items():
                add_tree(tar, inv, name, Path(root), tally, Path(scratch))
            data = json.dumps({"files": tally.files, "size": tally.size, "skipped": tally.skipped}).encode()
            entry = tarfile.TarInfo(SUMMARY)
            entry.size, entry.mode = len(data), 0o600
            tar.addfile(entry, io.BytesIO(data))
        writer.close()
    os.replace(partial, out)
    return info, tally


# -- stopping and resuming the source ---------------------------------------------------------------------------------

def services(home: Path, system: str) -> List[Tuple[str, str]]:
    """(how, name) of what runs this install here: its gateways, its bridge, its browsers, its nightly update."""
    from .family import bridge_restart
    from .update import gateway_units
    found = list(gateway_units())
    bridge = bridge_restart(home)
    found.append(("launchd" if system == "macos" else "systemd", bridge[-1].rsplit("/", 1)[-1]))
    units = home / ".config" / "systemd" / "user"
    if system != "macos" and units.is_dir():
        found += [("systemd", p.stem) for p in sorted(units.glob("bipagents-browser-*.service"))]
        if (units / "bipagents-update.timer").exists():
            found.append(("systemd", "bipagents-update.timer"))
    agents = home / "Library" / "LaunchAgents"
    if system == "macos" and (agents / "io.github.bipagents.update.plist").exists():
        found.append(("launchd", "io.github.bipagents.update"))
    return found


def stop(runner: hermes.Runner, found: List[Tuple[str, str]], home: Path) -> None:
    """Stopped and kept from starting again (at boot or login), each one noted for `resume`."""
    parked = home / ".config" / "bipagents" / "moved"
    for how, name in found:
        if how == "systemd":
            runner.run(["systemctl", "--user", "disable", "--now", name], check=False)
        elif how == "launchd":
            runner.run(["launchctl", "bootout", f"gui/{os.getuid()}/{name}"], check=False)
            plist = home / "Library" / "LaunchAgents" / f"{name}.plist"
            if plist.exists() and not runner.dry:
                parked.mkdir(parents=True, exist_ok=True)
                shutil.move(str(plist), str(parked / plist.name))
        else:
            runner.run([hermes.hermes_bin(), "gateway", "stop"], check=False)
            runner.run([hermes.hermes_bin(), "gateway", "uninstall"], check=False)
    record = home / ".config" / "bipagents" / MOVED
    runner.log.append(f"write {record}")
    if not runner.dry:
        record.parent.mkdir(parents=True, exist_ok=True)
        record.write_text(json.dumps({"stopped": found}, indent=2) + "\n", encoding="utf-8")


def resume(runner: hermes.Runner, home: Optional[Path] = None) -> List[str]:
    home = home or Path.home()
    record = home / ".config" / "bipagents" / MOVED
    try:
        found = [tuple(item) for item in json.loads(record.read_text(encoding="utf-8")).get("stopped", [])]
    except (OSError, ValueError):
        return []
    parked = home / ".config" / "bipagents" / "moved"
    started = []
    for how, name in found:
        if how == "systemd":
            if name.startswith("bipagents-browser-"):
                continue   # started by their first use (browsers on demand)
            runner.run(["systemctl", "--user", "enable", "--now", name], check=False)
        elif how == "launchd":
            plist = home / "Library" / "LaunchAgents" / f"{name}.plist"
            if (parked / plist.name).exists() and not runner.dry:
                shutil.move(str(parked / plist.name), str(plist))
            runner.run(["launchctl", "bootstrap", f"gui/{os.getuid()}", str(plist)], check=False)
        else:
            runner.run([hermes.hermes_bin(), "gateway", "install", "--start-now", "--start-on-login"], check=False)
        started.append(name)
    if not runner.dry:
        record.unlink(missing_ok=True)
    return started


# -- import -----------------------------------------------------------------------------------------------------------

@dataclass
class Target:
    home: Path
    repo: Path
    tailnet_name: Optional[str]
    lan_address: Optional[str]
    ports: Dict[int, int] = field(default_factory=dict)       # source port -> this machine's, when they differ
    sandbox: Optional[str] = None


def open_archive(path: Path, passphrase: str) -> Tuple[tarfile.TarFile, Dict[str, Any], Any]:
    handle = open(path, "rb")
    reader = io.BufferedReader(SealedReader(handle, passphrase), buffer_size=1 << 20)
    tar = tarfile.open(fileobj=reader, mode="r|gz")
    first = tar.next()
    if first is None or first.name != MANIFEST:
        raise SealedError("this export has no manifest")
    info = json.loads(tar.extractfile(first).read())   # type: ignore[union-attr]
    if info.get("format") != FORMAT:
        raise SealedError("this export was made by a newer BipAgents: update this one first")
    return tar, info, handle


def rehome(path: str, info: Dict[str, Any], target: Target) -> Path:
    """Where a path of the source goes here: under this home, or this checkout, else the same place."""
    source = info["source"]
    for old, new in ((source["repo"], target.repo), (source["home"], target.home)):
        parts = _relative(Path(path), Path(old))
        if parts is not None:
            return Path(new).joinpath(*parts)
    return Path(path)


def restore(tar: tarfile.TarFile, info: Dict[str, Any], target: Target) -> Dict[str, Any]:
    """The archive's files in their places here. Returns the archive's summary."""
    roots = {name: rehome(path, info, target) for name, path in info["roots"].items()}
    summary: Dict[str, Any] = {}
    while (member := tar.next()) is not None:
        tar.members = []   # a stream: nothing to go back to, nothing kept
        if member.name == SUMMARY:
            summary = json.loads(tar.extractfile(member).read())   # type: ignore[union-attr]
            continue
        name, _, rel = member.name.partition("/")
        if name not in roots:
            continue
        base = roots[name]
        dest = base / rel if rel else base
        if rel and (".." in Path(rel).parts or Path(rel).is_absolute()):
            continue
        if member.isdir():
            dest.mkdir(parents=True, exist_ok=True)
            os.chmod(dest, member.mode & 0o777 | 0o700)
        elif member.issym():
            dest.parent.mkdir(parents=True, exist_ok=True)
            if dest.is_symlink() or dest.exists():
                dest.unlink()
            link = member.linkname
            if os.path.isabs(link):
                link = str(rehome(link, info, target))
            os.symlink(link, dest)
        elif member.isfile():
            dest.parent.mkdir(parents=True, exist_ok=True)
            source = tar.extractfile(member)
            assert source is not None
            partial = dest.with_name(dest.name + ".bipagents-part")
            with open(partial, "wb") as out:
                shutil.copyfileobj(source, out, 1 << 20)
            os.chmod(partial, member.mode & 0o777 | 0o600)
            os.replace(partial, dest)
            os.utime(dest, (member.mtime, member.mtime))
    return summary


def replacements(info: Dict[str, Any], target: Target) -> List[Tuple[re.Pattern, str]]:
    """What changes in the settings: the checkout, the home, the machine's tailnet name, the ports."""
    source = info["source"]
    end = r"(?=[/\"'\s:,;\]\)}=]|$)"
    found: List[Tuple[re.Pattern, str]] = []
    for old, new in ((source["repo"], str(target.repo)), (source["home"], str(target.home))):
        if old and old != new:
            found.append((re.compile(re.escape(old) + end, re.M), new))
    name = source.get("tailnet_name")
    if name and target.tailnet_name and name != target.tailnet_name:
        found.append((re.compile(r"(?<![\w.-])" + re.escape(name) + r"(?![\w-])"), target.tailnet_name))
    hosts = [r"127\.0\.0\.1", "localhost"] + ([re.escape(target.tailnet_name)] if target.tailnet_name else [])
    for old, new in target.ports.items():
        for host in hosts:
            found.append((re.compile(rf"(?<={host}:){old}(?!\d)"), str(new)))
        found.append((re.compile(rf"(?<=API_SERVER_PORT=){old}(?!\d)"), str(new)))
        found.append((re.compile(rf"(?m)^(\s*port\s*=\s*){old}\s*$"), rf"\g<1>{new}"))
    return found


def settings_files(roots: List[Path]) -> Iterator[Path]:
    """The settings among the restored files: not conversations, files, memories or browser profiles."""
    for root in roots:
        if root.is_file():
            if root.suffix in TEXT_SUFFIXES or root.name in TEXT_NAMES:
                yield root
            continue
        for folder, dirs, files in os.walk(root):
            here = Path(folder)
            dirs[:] = [d for d in dirs if d not in NOT_SETTINGS]
            for name in files:
                path = here / name
                if (path.suffix in TEXT_SUFFIXES or name in TEXT_NAMES) and not path.is_symlink():
                    try:
                        if path.stat().st_size <= 4 * 1024 * 1024:
                            yield path
                    except OSError:
                        continue


def rewrite(paths: Iterator[Path], found: List[Tuple[re.Pattern, str]]) -> int:
    changed = 0
    for path in paths:
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        new = text
        for pattern, value in found:
            new = pattern.sub(value, new)
        if new != text:
            mode = path.stat().st_mode & 0o777
            path.write_text(new, encoding="utf-8")
            os.chmod(path, mode)
            changed += 1
    return changed


def set_in_section(text: str, section: str, key: str, value: Optional[str]) -> str:
    """`key = value` in a TOML section (added, replaced, or removed with value None); the rest as it is."""
    lines = text.splitlines()
    start = next((i for i, line in enumerate(lines) if line.strip() == f"[{section}]"), None)
    if start is None:
        if value is None:
            return text
        return text.rstrip("\n") + f"\n\n[{section}]\n{key} = {value}\n"
    end = next((i for i in range(start + 1, len(lines)) if lines[i].lstrip().startswith("[")), len(lines))
    at = next((i for i in range(start + 1, end) if re.match(rf"\s*{re.escape(key)}\s*=", lines[i])), None)
    if at is not None:
        if value is None:
            del lines[at]
        else:
            lines[at] = f"{key} = {value}"
    elif value is not None:
        lines.insert(start + 1, f"{key} = {value}")
    return "\n".join(lines) + "\n"


def adjust_bridge(config: Path, info: Dict[str, Any], target: Target, old_toml: Dict[str, Any]) -> None:
    """What the text rewrite cannot know: this machine's local network, old file paths still in conversations."""
    text = config.read_text(encoding="utf-8")
    if target.lan_address:
        text = set_in_section(text, "lan", "address", json.dumps(target.lan_address))
    else:   # a rented server: no door on its network (it would be on the Internet)
        text = set_in_section(text, "lan", "enabled", "false")
        text = set_in_section(text, "lan", "address", None)
    if info["source"]["home"] != str(target.home):
        # Conversations keep the files' old paths (MEDIA: lines): they are found in their new place.
        for old_key, old_value in ((old_toml.get("media") or {}).get("roots") or {}).items():
            new_value = str(rehome(old_value, info, target))
            if old_key != new_value and f"{json.dumps(old_key)} =" not in text:
                text = re.sub(r"(?m)^\[media\.roots\]\s*$", lambda m: f"{m.group(0)}\n{json.dumps(old_key)} = "
                              f"{json.dumps(new_value)}", text, count=1)
    if info.get("isolated") and not target.sandbox:
        text = re.sub(r"(?m)^(\s*(?:isolated|sandboxed)\s*=\s*)true\s*$", r"\g<1>false", text)
    config.write_text(text, encoding="utf-8")
    os.chmod(config, 0o600)


def set_env(path: Path, key: str, value: str) -> None:
    lines = path.read_text(encoding="utf-8").splitlines() if path.exists() else []
    lines = [line for line in lines if line.split("=", 1)[0].strip() != key] + [f"{key}={value}"]
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    os.chmod(path, 0o600)


def unpack(tar: tarfile.TarFile, info: Dict[str, Any], target: Target, config: Path) -> Tuple[Dict[str, Any], int]:
    """The export's files in place here and its settings adjusted to this machine. Returns the export's summary and
    how many settings files changed."""
    import tomllib
    summary = restore(tar, info, target)
    if not config.exists():
        raise SealedError("the export holds no bridge config: it was not made by `bipagents export`")
    old_toml = tomllib.loads(config.read_text(encoding="utf-8"))
    roots = [rehome(path, info, target) for path in info["roots"].values()]
    changed = rewrite(settings_files(roots), replacements(info, target))
    adjust_bridge(config, info, target, old_toml)
    (config.parent / "existing-hermes.json").unlink(missing_ok=True)   # Hermes is BipAgents' own here
    (config.parent / MOVED).unlink(missing_ok=True)
    return summary, changed


def import_main(argv: List[str]) -> int:
    from .__main__ import compress_memory, gateway_service, start_service, venv, wait_for, write
    from .detect import detect
    from .ports import Ports, pick_block, port_free
    from .stack import Install, bip_languages, bridge_service, pocket_service
    from .update import agents_of, check_agents, report

    parser = argparse.ArgumentParser(prog="bipinstall import", description="Install here an install exported with "
                                     "`bipagents export` on another machine")
    parser.add_argument("archive", help="the exported file")
    parser.add_argument("--passphrase-file", help="a file holding its passphrase (default: asked)")
    parser.add_argument("--yes", action="store_true", help="no questions (the passphrase still comes from a file)")
    parser.add_argument("--no-tailscale", action="store_true", help="nothing published on a tailnet (tests)")
    parser.add_argument("--dry-run", action="store_true", help="read the file and say what would be done")
    args = parser.parse_args(argv)
    runner = hermes.Runner(dry=args.dry_run)
    archive = Path(args.archive).expanduser()
    if not archive.is_file():
        say(ui.fail(f"No file at {archive}."))
        return 1
    home = Path.home()
    config = home / ".config" / "bipagents" / "bridge.toml"
    if config.exists():
        say(ui.fail("This account already has BipAgents. Import into an account without it (or `bipagents uninstall "
                    "--all` first: it removes this one's agents)."))
        return 1
    if (hermes.hermes_home() / "config.yaml").exists() and not (hermes.hermes_home() / hermes.UNFINISHED).exists():
        say(ui.fail(f"This account already has a Hermes ({hermes.hermes_home()}): import into an account without one."))
        return 1
    from . import rustbridge
    if not rustbridge.asset() and not os.environ.get(rustbridge.BUILD):
        say(ui.fail(rustbridge.unsupported()))   # before anything is changed
        return 1

    if args.passphrase_file:
        passphrase = Path(args.passphrase_file).expanduser().read_text(encoding="utf-8").strip()
    else:
        passphrase = getpass.getpass(ui.question("Its passphrase (shown on the other machine): "))
    try:
        tar, info, handle = open_archive(archive, passphrase)
    except SealedError as exc:
        say(ui.fail(str(exc).capitalize() + "."))
        return 1

    machine = detect()
    source = info["source"]
    say(ui.banner(os.environ.get("USER", ""), machine.summary))
    say(f"\nThe install of {source['user']} on {source['host']} ({source['system']}, {source['arch']}), exported "
        f"{info['created'][:16].replace('T', ' ')} UTC: {', '.join(a['display_name'] for a in info['agents']) or 'no agent'}.")
    if machine.system == "linux" and not args.dry_run:
        from .account import ensure_user_services
        if not ensure_user_services(say):
            return 1
    if args.no_tailscale:
        machine.tailnet_name = None
    elif not machine.tailnet_name:
        say(ui.sad("Tailscale isn't connected."))
        say("Install it and sign in with the same account as before (https://tailscale.com/download), then run this again.")
        return 1
    if machine.cloud:
        machine.lan_addresses = []

    # Ports: the same ones when they are free here (the phones then find everything where it was).
    ports = info["ports"]
    wanted = [ports["hub"], ports["bridge"], ports["lan"]] + ([ports["desk"]] if ports["desk"] in info["published"] else [])
    mapping: Dict[int, int] = {}
    if not all(port_free(p) for p in wanted):
        block = pick_block()
        mapping = {old: new for old, new in ((ports["hub"], block.hermes), (ports["bridge"], block.bridge),
                                             (ports["lan"], block.lan), (ports["desk"], block.desk)) if old != new}
    busy = [f"{a['display_name']} ({a['space_port']})" for a in info["agents"] if a.get("space_port")
            and not port_free(a["space_port"])]
    if busy:
        say(ui.fail(f"Ports of these agents' own spaces are taken on this machine: {', '.join(busy)}."))
        return 1
    target = Target(home=home, repo=REPO, tailnet_name=machine.tailnet_name,
                    lan_address=(machine.lan_addresses or [None])[0], ports=mapping, sandbox=hermes.sandbox_runtime())

    same_address = (not mapping or ports["bridge"] not in mapping) and source.get("tailnet_name") == target.tailnet_name
    if same_address:
        say(ui.note("Same name on the tailnet, same ports: your phones will find their agents by themselves."))
    elif source.get("tailnet_name") and target.tailnet_name and source["tailnet_name"] != target.tailnet_name:
        old_name, new_name = source["tailnet_name"].split(".")[0], target.tailnet_name.split(".")[0]
        say(ui.note(f"Your phones know these agents at {old_name}; this machine is {new_name} on the tailnet. For them to "
                    f"need nothing,\n    rename the old machine in Tailscale (e.g. {old_name}-old), name this one "
                    f"{old_name}, then run this again.\n    Otherwise each phone scans a new QR code at the end."))
    if mapping:
        say(ui.note(f"Ports taken here: this install moves to {min(mapping.values()) - 2}–{min(mapping.values()) + 7}."))
    if info.get("isolated") and not target.sandbox:
        say(ui.note("No sandbox here (podman, on Linux): your agents keep their memory and conversations, but lose "
                    "their terminal,\n    file access and vault until one is installed."))
    if info.get("browsers") and not target.sandbox:
        say(ui.note("No podman here: the agents' visible browsers stay off (their profiles are kept)."))
    free = shutil.disk_usage(home).free
    if free < archive.stat().st_size * 3:
        say(ui.fail(f"Not enough disk space here ({free // 2**30} GB free for a {archive.stat().st_size // 2**20} MB "
                    "export, which takes about three times its size once unpacked)."))
        return 1
    if args.dry_run:
        say("\nDry run: nothing changed.")
        return 0
    if not args.yes:
        from .__main__ import ask, yes
        if not yes(ask("\nInstall it here? (y/n)", "y")):
            return 1

    total = 5 + (1 if info.get("browsers") and target.sandbox else 0)
    steps = iter(range(1, total + 1))
    say(ui.step(next(steps), total, "Hermes, in the same version"))
    if info.get("hermes_commit"):
        hermes.HERMES_COMMIT = info["hermes_commit"]
    from .plan import make_plan
    hermes.install(runner, browser=make_plan(machine, False).browser != "none")
    skipped = hermes.apply_patches(runner, REPO / "hermes" / "patches")
    if skipped:
        say(f"  Patches not applied to this Hermes version: {', '.join(skipped)}.")

    say(ui.step(next(steps), total, "Your agents, their memory and their files"))
    try:
        summary, changed = unpack(tar, info, target, config)
    except SealedError as exc:
        say(ui.fail(str(exc).capitalize() + "."))
        return 1
    finally:
        tar.close()
        handle.close()
    inv = read_inventory(config, home)
    say(ui.done(f"{summary.get('files', '?')} files back in place, {changed} settings adjusted to this machine."))
    if summary.get("skipped"):
        say(ui.note(f"{len(summary['skipped'])} files could not be read on the other machine (written by a sandbox): "
                    "not moved."))
    if target.sandbox and info.get("isolated"):
        # The sandbox's engine where it is here (podman's path differs between systems).
        for home_dir in inv.hermes_homes():
            if (home_dir / ".env").exists():
                set_env(home_dir / ".env", "HERMES_DOCKER_BINARY", target.sandbox)
        if Path(target.sandbox).name == "podman":
            hermes.podman_short_names(runner)
    elif info.get("isolated"):
        # No sandbox here: as a fresh install without one, no terminal or disk for the agents (they would run on this
        # machine, next to the bridge's keys) until one is installed.
        profiles = [hermes.HUB_PROFILE] + sorted(p.name for p in (inv.hermes_home / "profiles").glob("*") if p.is_dir())
        for profile in profiles:
            prefix = [hermes.hermes_bin()] + ([] if profile == hermes.HUB_PROFILE else ["-p", profile])
            runner.run(prefix + ["config", "set", "terminal.backend", "local"], check=False)
            runner.run(prefix + ["tools", "disable", *hermes.DISK_TOOLS, "--platform", "api_server"], check=False)

    say(ui.step(next(steps), total, "The agents' gateway and voices"))
    base = (mapping.get(ports["bridge"], ports["bridge"])) - 3
    inst = Install(repo=REPO, home=home, ports=Ports(base), tailnet_name=target.tailnet_name,
                   lan_address=target.lan_address)
    try:   # the bridge's binaries for this machine (their helper runs the agents' browser tools)
        rustbridge.ensure(home, REPO, runner)
    except rustbridge.RustBridgeError as exc:
        say(ui.fail(f"The bridge couldn't be installed: {exc}"))
        say(ui.note("Try the import again in a few minutes (`bipagents uninstall --all` first if it finds BipAgents here)."))
        return 1
    venv(REPO / "installer", runner)   # what the bridge runs the installer with (`place import`, restores)
    gateway_service(inst, machine, runner)
    from .__main__ import pocket_running
    if not pocket_running():
        venv(REPO / "pocket", runner)
        label, path, content = pocket_service(inst, machine.system, bip_languages())
        write(path, content, runner)
        start_service(machine.system, label, path, runner)
    if machine.cloud:
        compress_memory(machine, runner)

    say(ui.step(next(steps), total, "The bridge"))
    label, path, content = bridge_service(inst, machine.system)
    write(path, content, runner)
    start_service(machine.system, label, path, runner)
    bridge = rustbridge.cli(home, REPO)
    if any(a.get("space") for a in info["agents"]):
        runner.run(bridge + ["spaces", "start", "--config", str(config)], cwd=REPO / "bridge", check=False)
    if info.get("browsers") and target.sandbox:
        say(ui.step(next(steps), total, "Your agents' visible browsers"))
        for agent in info["agents"]:
            ports_file = inv.browsers_root / agent["name"] / "ports.json"
            try:
                taken = json.loads(ports_file.read_text(encoding="utf-8")).values()
            except (OSError, ValueError):
                continue
            if not all(port_free(p) for p in taken):
                ports_file.unlink()   # others are picked
        try:
            hermes.build_browser(runner, target.sandbox, REPO)
            from . import browser_refresh
            from .__main__ import INSTALLER_PYTHON
            browser_refresh.install_timer(runner, REPO, INSTALLER_PYTHON)   # Chrome's security fixes, each night
            runner.run(bridge + ["browser", "add", "--all", "--config", str(config)], cwd=REPO / "bridge")
            if machine.system == "linux":
                runner.run(["systemctl", "--user", "restart", label], check=False)
            say(ui.done("Their browsers are back, logins included (some sites may ask again from a new address)."))
        except RuntimeError as exc:
            say(ui.fail("The browsers couldn't be set up: your agents work without them."))
            say(ui.note(str(exc)[-300:]))

    say(ui.step(next(steps), total, "Tailscale and checks"))
    if not args.no_tailscale:
        published = [mapping.get(p, p) for p in info["published"]]
        for port in published:
            runner.run(f"{machine.tailscale or 'tailscale'} serve --bg --https={port} http://127.0.0.1:{port}",
                       shell=True, check=False)
    from . import migrations
    from .update import restart
    from .upgrade import install_command
    install_command(runner)
    for line in migrations.run(runner):
        say(ui.done(line))
    restart(runner)   # the gateways connect to the bridge's tools and the browsers, now all up
    ok = report(agents_of(config), check_agents(agents_of(config), expect_clarify=False))
    wait_for(f"http://127.0.0.1:{inst.ports.bridge}/health")
    (hermes.hermes_home() / hermes.UNFINISHED).unlink(missing_ok=True)

    if same_address:
        say(ui.finale("Moved! Your phones find their agents by themselves."))
    else:
        say(ui.finale("Moved! On each phone, scan this code (then delete the old agents in the app)."))
        subprocess.run(bridge + ["qr", "--config", str(config)], cwd=REPO / "bridge", check=False)
    if info.get("model_url") and not _reachable(info["model_url"]):
        say(ui.note(f"The model at {info['model_url']} doesn't answer from here: choose another one in the app "
                    "(an agent › Model) or with `hermes model`."))
    say(f"On {source['host']}, once everything works here: `bipagents uninstall --all` removes the old copy.")
    return 0 if ok else 1


def _reachable(url: str) -> bool:
    import httpx
    try:
        httpx.get(url.rstrip("/") + "/models", timeout=5)
        return True
    except httpx.HTTPError:
        return False


def export_main(argv: List[str]) -> int:
    from .detect import detect
    from .family import find_config

    parser = argparse.ArgumentParser(prog="bipinstall export", description="Everything this account's agents need, in "
                                     "one encrypted file: to move them to another machine, or as a backup")
    parser.add_argument("-o", "--output", help="the file to write (default: ~/bipagents-<machine>-<date>.bipagents)")
    parser.add_argument("--moving", action="store_true", help="stop this install once the copy is made (it moves); "
                        "`bipagents resume` starts it again")
    parser.add_argument("--passphrase-file", help="take the passphrase from this file instead of making one")
    parser.add_argument("--config", help="the bridge's config (default: found)")
    args = parser.parse_args(argv)
    runner = hermes.Runner()
    home = Path.home()
    config = find_config(args.config)
    inv = read_inventory(config, home)
    if not (inv.toml.get("hermes") or {}).get("hub_url"):
        say(ui.fail("This bridge was set up by hand (no [hermes] hub_url): copy its machine's files yourself."))
        return 1
    machine = detect()
    stamp = datetime.date.today().isoformat()
    out = Path(args.output).expanduser() if args.output else home / f"bipagents-{socket.gethostname().split('.')[0]}-{stamp}.bipagents"
    passphrase = (Path(args.passphrase_file).expanduser().read_text(encoding="utf-8").strip() if args.passphrase_file
                  else new_passphrase())
    found: List[Tuple[str, str]] = []
    if args.moving:
        found = services(home, machine.system)
        say("Stopping this install while it is copied (it moves)…")
        stop(runner, found, home)
    say("Copying your agents, their memory, files and settings (encrypted)…")
    try:
        info, tally = export(inv, out, passphrase, machine)
    except BaseException:
        if found:
            resume(runner, home)
            say(ui.note("Export failed: this install runs again here."))
        raise
    say(ui.done(f"{tally.files} files, {tally.size / 2**20:.0f} MB before compression → {out} "
                f"({out.stat().st_size / 2**20:.0f} MB)."))
    if tally.skipped:
        say(ui.note(f"{len(tally.skipped)} files unreadable from this account (written by a sandbox) were left out."))
    say(f"\n{ui.BOLD}Its passphrase{ui.RESET} (needed to open it; write it down, it is shown only now):\n\n    {passphrase}\n")
    if args.moving:
        name = (machine.tailnet_name or "").split(".")[0]
        say("Next, on the new machine (Linux or a Mac, with Tailscale signed in to the same account):")
        say(f"  1. Copy the file there, e.g.  scp {out} user@new-machine:")
        if name:
            say(f"  2. For your phones to need nothing: in Tailscale's admin, rename this machine ({name}-old) and name "
                f"the new one {name}.")
        say(f"  3. On it: curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash -s -- "
            f"import {out.name}")
        say("This install is stopped here; `bipagents resume` starts it again if you change your mind.")
    else:
        say("Keep the file and its passphrase apart. It holds your agents' keys: whoever has both can run them.")
    return 0


def resume_main(argv: List[str]) -> int:
    started = resume(hermes.Runner())
    if not started:
        say("Nothing was stopped by `bipagents export --moving` here.")
        return 0
    say(ui.done(f"Started again here: {', '.join(started)}."))
    return 0
