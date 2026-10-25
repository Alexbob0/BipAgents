"""The Bips' voices without PyTorch: the phones' 8-bit packages (voices/onnx/build.py), for pocket_server.py --onnx.

Never at a public address: a place of the all-included offer gets them from its guichet (guichet/bipguichet/
voicefiles.py) with the backup token of its setup; a test may give another address (BIPAGENTS_VOICES_URL).
Downloaded there (``index.json``, then each package's ``manifest.json`` and the files it lists, each one checked
against its sha256) into ~/.local/share/bipagents/voices: about 110 to 130 MB a language and 41 MB for the
decoder they share, instead of PyTorch's 5.5 GB. The server then loads a language only while it speaks (a podcast,
an audio file, a reply for a phone that can't speak itself) and frees it after. When the packages can't be reached,
the install falls back to Pocket's own models."""
from __future__ import annotations

import hashlib
import json
import os
import shutil
from pathlib import Path
from typing import Callable, List, Optional, Tuple

import httpx

from . import hermes

VOICES_URL = os.environ.get("BIPAGENTS_VOICES_URL", "")
NAMES = {"fr": "french", "en": "english", "es": "spanish", "de": "german"}


def folder(home: Path) -> Path:
    return home / ".local" / "share" / "bipagents" / "voices"


def source(home: Path) -> Tuple[Optional[str], Optional[str]]:
    """Where this account may fetch them, and with what token: the test address, else its guichet (a place of the
    offer: its backup settings, https://…/v1/backup/ -> https://…/v1/voices/); (None, None) elsewhere."""
    if VOICES_URL:
        return VOICES_URL, None
    from . import backup
    settings = backup.read_json(home / backup.SETTINGS)
    url, token = settings.get("url"), settings.get("token")
    if isinstance(url, str) and url.rstrip("/").endswith("/v1/backup") and isinstance(token, str):
        return url.rstrip("/")[: -len("backup")] + "voices/", token
    return None, None


def _package(client: httpx.Client, base: str, name: str, target: Path) -> None:
    """A package's folder: its manifest, then each file it lists, checked, into a temporary folder moved in place."""
    done = target / name
    if (done / "manifest.json").is_file():
        return
    manifest_bytes = client.get(f"{base}{name}/manifest.json").raise_for_status().content
    files = json.loads(manifest_bytes)["files"]
    partial = target / f"{name}.part"
    shutil.rmtree(partial, ignore_errors=True)
    for path, spec in sorted(files.items()):
        if ".." in path:
            raise ValueError(f"bad path in {name}: {path}")
        out = partial / path
        out.parent.mkdir(parents=True, exist_ok=True)
        digest = hashlib.sha256()
        with client.stream("GET", f"{base}{name}/{path}") as response, open(out, "wb") as fh:
            response.raise_for_status()
            for block in response.iter_bytes(1 << 20):
                digest.update(block)
                fh.write(block)
        if digest.hexdigest() != spec["sha256"]:
            raise ValueError(f"{name}/{path}: checksum")
    (partial / "manifest.json").write_bytes(manifest_bytes)
    shutil.rmtree(done, ignore_errors=True)
    partial.rename(done)


def install(languages: List[str], home: Path, runner: hermes.Runner, say: Callable[[str], None],
            base: Optional[str] = None, transport: Optional[httpx.BaseTransport] = None,
            token: Optional[str] = None) -> Optional[Path]:
    """The packages of `languages` (fr, en, es, de) and the decoder, in this account's voices folder: the folder, or
    None when they can't be had (the caller installs Pocket's models instead). `base`: where (else `source`)."""
    target = folder(home)
    if base is None:
        base, token = source(home)
    if runner.dry:
        say(f"  (dry run) would download the voices of {', '.join(languages)} from {base or 'the guichet'} into {target}")
        return target
    if base is None:
        say("  the light voices come from the offer's guichet only: Pocket's own models instead")
        return None
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    try:
        with httpx.Client(timeout=httpx.Timeout(30, read=120), follow_redirects=True, transport=transport,
                          headers=headers) as client:
            index = client.get(f"{base}index.json").raise_for_status().json()
            target.mkdir(parents=True, exist_ok=True)
            wanted = [NAMES[code] for code in languages if NAMES.get(code) in index.get("languages", {})]
            if not wanted:
                raise ValueError("no package for these languages")
            _package(client, base, index["mimi"]["folder"], target)
            kept = {}
            for name in wanted:
                entry = index["languages"][name]
                _package(client, base, entry["folder"], target)
                kept[name] = entry
        (target / "index.json").write_text(json.dumps({"languages": kept, "mimi": index["mimi"]}), encoding="utf-8")
        used = {entry["folder"] for entry in kept.values()} | {index["mimi"]["folder"], "index.json"}
        for item in target.iterdir():
            if item.name not in used:
                shutil.rmtree(item, ignore_errors=True) if item.is_dir() else item.unlink(missing_ok=True)
        return target
    except (httpx.HTTPError, OSError, ValueError, KeyError) as exc:
        say(f"  the light voices can't be had ({type(exc).__name__}): Pocket's own models instead")
        return None


# -- which Pocket speaks: the bridge prepares the text for it ([pocket] engine) ------------------------------------

def _key(version: str) -> Tuple[int, ...]:
    return tuple(int(part) if part.isdigit() else 0 for part in version.split("."))


def major(version: Optional[str]) -> Optional[int]:
    head = (version or "").split(".")[0]
    return int(head) if head.isdigit() else None


def engine_of(packages: Path) -> Optional[str]:
    """The Pocket version of the packages in `packages` (« 4.0.1 »: their manifests' `engine`), the lowest when they
    differ; None without any."""
    try:
        index = json.loads((packages / "index.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    found = []
    for entry in (index.get("languages") or {}).values():
        try:
            manifest = json.loads((packages / str(entry["folder"]) / "manifest.json").read_text(encoding="utf-8"))
        except (OSError, ValueError, KeyError, TypeError):
            continue
        version = str(manifest.get("engine", "")).split()[-1:]
        if version and version[0][:1].isdigit():
            found.append(version[0])
    return min(found, key=_key) if found else None


def languages_of(packages: Path) -> List[str]:
    """The language codes whose package is in `packages` (fr, en…)."""
    try:
        names = (json.loads((packages / "index.json").read_text(encoding="utf-8")).get("languages") or {}).keys()
    except (OSError, ValueError, AttributeError):
        return []
    return [code for code, name in NAMES.items() if name in names]


def refresh(home: Path, runner: hermes.Runner, say: Callable[[str], None],
            transport: Optional[httpx.BaseTransport] = None) -> bool:
    """The light voices brought to what the guichet now serves (new packages: a new Pocket, new Bips' voices), the
    old ones removed once the new are here: whether anything changed (Pocket then restarts on them)."""
    target = folder(home)
    languages = languages_of(target)
    base, token = source(home)
    if not languages or base is None:
        return False
    if runner.dry:
        say(f"  (dry run) would fetch the guichet's newer voices of {', '.join(languages)}, if any")
        return False
    before = (target / "index.json").read_text(encoding="utf-8")
    if install(languages, home, runner, say, base=base, transport=transport, token=token) is None:
        return False
    return (target / "index.json").read_text(encoding="utf-8") != before


def engine_at(url: str, wait: float = 0, transport: Optional[httpx.BaseTransport] = None) -> Optional[str]:
    """The Pocket version a voices server at `url` says it speaks with (its /health `engine`); None when it doesn't
    answer within `wait` seconds (it may be loading its model after a restart) or doesn't say (before 2026-10-25)."""
    import time
    deadline = time.monotonic() + wait
    while True:
        try:
            with httpx.Client(timeout=5, transport=transport) as client:
                response = client.get(url.rstrip("/") + "/health")
            if response.status_code == 200:
                engine = response.json().get("engine")
                return engine if isinstance(engine, str) and engine[:1].isdigit() else None
        except (httpx.HTTPError, ValueError, AttributeError):
            pass
        if time.monotonic() >= deadline:
            return None
        time.sleep(3)


def pinned_pocket(repo: Path) -> Optional[str]:
    """The pocket-tts version pocket/requirements.txt pins (« 4.0.1 »)."""
    try:
        for line in (repo / "pocket" / "requirements.txt").read_text(encoding="utf-8").splitlines():
            if line.strip().lower().startswith("pocket-tts=="):
                return line.split("==", 1)[1].strip()
    except OSError:
        pass
    return None


def installed_pocket(repo: Path) -> Optional[str]:
    """The pocket-tts version in pocket/.venv (Pocket's own models), None without it."""
    import subprocess
    python = repo / "pocket" / ".venv" / "bin" / "python"
    if not python.exists():
        return None
    out = subprocess.run([str(python), "-c", "from importlib.metadata import version; print(version('pocket-tts'))"],
                         capture_output=True, text=True)
    return (out.stdout.strip() or None) if out.returncode == 0 else None
