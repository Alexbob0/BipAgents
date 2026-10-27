"""The bridge's config file (bridge.toml) changed safely, by every part of the installer (migrations, family, moves,
restores, voice server, shared Hermes, a new install).

The bridge rewrites the same file while it runs (an agent made or moved, a voice picked: bridge-rs/src/config.rs,
« the config file on disk »). On 2026-10-26 (aibox) rewrites running at once broke it twice, the second time leaving
nothing but four agents' voices; the bridge then couldn't start. Both sides now go the same way:
- `flock` on `bridge.toml.lock` beside it, held from the read to the rename;
- the new text checked: it parses, keeps the bridge's key and every agent's address (but the agents being removed),
  else nothing is written (BridgeConfigError);
- written to a temporary file in the same folder, synced, renamed over it (0600; root gives it to the file's owner).
The bridge also keeps `bridge.toml.last-good` and puts it back at start when the file doesn't load.
"""
from __future__ import annotations

import fcntl
import os
import sys
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Callable, Iterable, Iterator, Optional, Union

try:
    import tomllib
except ImportError:  # pragma: no cover
    import tomli as tomllib   # type: ignore[no-redef]

LOCK_SUFFIX = ".lock"          # bridge-rs/src/config.rs LOCK_SUFFIX
LOCK_WAIT = 30.0               # seconds: a write takes milliseconds


class BridgeConfigError(Exception):
    """A change refused: the file stays as it was."""


class _Any:
    """`removing=ANY`: the whole file replaced (an imported place's agents in place of this one's)."""


ANY = _Any()
Removing = Union[Iterable[str], _Any]


def beside(path: Path, suffix: str) -> Path:
    return path.with_name(path.name + suffix)


def _same_owner(fd: int, like: Path) -> None:
    """root (an administrator's command) gives what it makes to the config's owner: the bridge runs as that account."""
    if os.geteuid() != 0:
        return
    try:
        st = os.stat(like)
    except FileNotFoundError:
        st = os.stat(like.parent)
    os.fchown(fd, st.st_uid, st.st_gid)


@contextmanager
def locked(path: Path, wait: float = LOCK_WAIT) -> Iterator[None]:
    """The config's lock (the bridge takes the same), held inside the block."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(beside(path, LOCK_SUFFIX), os.O_RDWR | os.O_CREAT, 0o600)
    try:
        _same_owner(fd, path)
        deadline = time.monotonic() + wait
        while True:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise BridgeConfigError(f"{path}: another program kept it locked") from None
                time.sleep(0.02)
        yield
    finally:
        os.close(fd)   # releases the lock


def _agents(doc: dict) -> dict:
    agents = doc.get("agents")
    if not isinstance(agents, dict):
        return {}
    return {name.strip().lower(): isinstance(t, dict) and bool(str(t.get("hermes_url") or "").strip())
            for name, t in agents.items()}


def _has_key(doc: dict) -> bool:
    return any(doc.get(k) not in (None, "") for k in ("bridge_key", "bridge_key_file"))


def refused(old: str, new: str, removing: Removing = ()) -> Optional[str]:
    """Why `new` mustn't replace `old`, None when it may: it must parse, keep the bridge's key and every agent's
    `hermes_url` (but the agents in `removing`); an agent it adds needs one too."""
    try:
        new_doc = tomllib.loads(new)
    except tomllib.TOMLDecodeError as exc:
        return f"the new text isn't valid TOML ({exc})"
    after = _agents(new_doc)
    try:
        old_doc = tomllib.loads(old) if old.strip() else {}
    except tomllib.TOMLDecodeError:
        old_doc = None   # broken already: the new text must at least hold what the bridge needs
    if old_doc is None:
        if not _has_key(new_doc):
            return "the file doesn't parse and the new text has no bridge_key"
    elif _has_key(old_doc) and not _has_key(new_doc):
        return "it would lose bridge_key"
    before = _agents(old_doc or {})
    going = None if isinstance(removing, _Any) else {name.strip().lower() for name in removing}
    for name, had_url in before.items():
        if had_url and (going is not None and name not in going) and not after.get(name):
            return f"it would lose agent {name} (or its hermes_url)"
    for name, has_url in after.items():
        if name not in before and not has_url:
            return f"agent {name} has no hermes_url"
    return None


def _write(path: Path, text: str) -> None:
    """The whole file: a temporary file beside it (0600 from the start), synced, renamed over it."""
    tmp = path.with_name(f".{path.name}.{os.getpid()}.{time.monotonic_ns()}.tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(text)
            fh.flush()
            os.fsync(fh.fileno())
            _same_owner(fh.fileno(), path)
        os.replace(tmp, path)
    except BaseException:
        tmp.unlink(missing_ok=True)
        raise
    try:   # the rename itself on disk
        folder = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(folder)
        finally:
            os.close(folder)
    except OSError:
        pass


def update(path: Path, change: Callable[[str], str], removing: Removing = ()) -> bool:
    """The bridge's config changed by `change` (given its text, read under the lock; "" when there is none): checked,
    then written whole. Returns whether it changed; BridgeConfigError when the change is refused."""
    path = Path(path)
    with locked(path):
        try:
            old = path.read_text(encoding="utf-8")
        except FileNotFoundError:
            old = ""
        new = change(old)
        if new == old:
            return False
        why = refused(old, new, removing)
        if why:
            print(f"bipinstall: {path}: change refused, {why} (the file stays as it was)", file=sys.stderr)
            raise BridgeConfigError(f"{path}: change refused, {why}")
        _write(path, new)
    return True


def write(path: Path, text: str, removing: Removing = ()) -> bool:
    """`text` as the whole file, checked against what it holds (`removing=ANY`: any agent may go)."""
    return update(path, lambda _old: text, removing)
