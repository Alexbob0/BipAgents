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
