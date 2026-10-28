"""The Bips' voices without PyTorch: the phones' 8-bit packages (voices/onnx/build.py), for pocket_server.py --onnx.

Where from: a place of the all-included offer gets them from its guichet (guichet/bipguichet/voicefiles.py) with the
backup token of its setup, ``index.json`` then each package's ``manifest.json`` and the files it lists; every other
install (one's own server, a family's, a Mac) from the latest ``voices-*`` release of the public repository whose
Pocket is the one this checkout pins (published by scripts/publish-voices.sh, voices/onnx/release.py): ``SHA256SUMS``,
``index.json``, then one ``<folder>.tar.gz`` per package. A test may give another address (BIPAGENTS_VOICES_URL), or a
release (BIPAGENTS_VOICES_RELEASE=voices-4.0.1-…). Every file is checked against its sha256 (the archive against
SHA256SUMS, each file against its manifest, each manifest against the index) into ~/.local/share/bipagents/voices:
only the languages the install uses, about 110 to 130 MB a language and 41 MB for the decoder they share, instead of
PyTorch's 5.5 GB. The server then loads a language only while it speaks (a podcast, an audio file, a reply for a phone
that can't speak itself) and frees it after. When the packages can't be had, the install falls back to Pocket's own
models."""
from __future__ import annotations

import hashlib
import json
import os
import shutil
import tarfile
from pathlib import Path
from typing import Callable, List, Optional, Tuple

import httpx

from . import hermes, release_key

VOICES_URL = os.environ.get("BIPAGENTS_VOICES_URL", "")
RELEASE = os.environ.get("BIPAGENTS_VOICES_RELEASE", "")   # a given release instead of the latest
SLUG = "Alexbob0/BipAgents"
API = f"https://api.github.com/repos/{SLUG}/releases"
DOWNLOADS = f"https://github.com/{SLUG}/releases/download"
PAGES = 5            # releases looked through (100 a page, the newest first: the bridge's are many)
REPO = Path(__file__).resolve().parents[2]
NAMES = {"fr": "french", "en": "english", "es": "spanish", "de": "german", "nl": "dutch"}
# Languages that came after installs set up « all of them » (2026-10-26: Dutch): an update adds them to a Pocket that
# serves every older one, never to one that chose fewer (the « mini » profile: the person's language only).
NEW_LANGUAGES = ("nl",)


def with_new_languages(codes: List[str]) -> List[str]:
    """`codes` with the newer languages added when it has every older one."""
    older = [code for code in NAMES if code not in NEW_LANGUAGES]
    if not all(code in codes for code in older):
        return list(codes)
    return list(codes) + [code for code in NEW_LANGUAGES if code not in codes]


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


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as fh:
        for block in iter(lambda: fh.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def latest_release(client: httpx.Client, pinned: Optional[str] = None) -> Optional[str]:
    """The newest `voices-<engine>-<hash>` release of the public repository whose Pocket has the major version this
    checkout pins (pocket/requirements.txt: its pocket_server.py and the bridge's text rules are made for it); None
    without one. BIPAGENTS_VOICES_RELEASE gives one instead."""
    if RELEASE:
        return RELEASE
    pinned = pinned if pinned is not None else pinned_pocket(REPO)
    for page in range(1, PAGES + 1):
        listed = client.get(API, params={"per_page": 100, "page": page},
                            headers={"Accept": "application/vnd.github+json"}).raise_for_status().json()
        found = [item for item in listed if isinstance(item, dict) and not item.get("draft")
                 and str(item.get("tag_name", "")).startswith("voices-")
                 and (pinned is None or major(str(item["tag_name"]).split("-")[1]) == major(pinned))]
        if found:
            newest = max(found, key=lambda item: str(item.get("published_at") or item.get("created_at") or ""))
            return str(newest["tag_name"])
        if len(listed) < 100:
            break
    return None


def sums_of(text: str) -> dict:
    """SHA256SUMS' lines (« <sha256>  <name> ») as {name: sha256}."""
    found = {}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) == 2 and len(parts[0]) == 64:
            found[parts[1].lstrip("*")] = parts[0].lower()
    return found


def _archive(client: httpx.Client, base: str, sums: dict, entry: dict, target: Path) -> None:
    """A package from a release: `<folder>.tar.gz` downloaded and checked against SHA256SUMS, only its regular files
    under `<folder>/` taken out into a temporary folder, its manifest checked against the index and each file against
    its manifest, then moved in place."""
    name = str(entry["folder"])
    done = target / name
    if (done / "manifest.json").is_file():
        return
    if "/" in name or ".." in name or not name:
        raise ValueError(f"bad package name: {name}")
    asset = f"{name}.tar.gz"
    expected = sums.get(asset)
    if expected is None:
        raise ValueError(f"{asset} isn't in the release's SHA256SUMS")
    download = target / f".{asset}.part"
    partial = target / f"{name}.part"
    shutil.rmtree(partial, ignore_errors=True)
    try:
        digest = hashlib.sha256()
        with client.stream("GET", f"{base}{asset}") as response, open(download, "wb") as fh:
            response.raise_for_status()
            for block in response.iter_bytes(1 << 20):
                digest.update(block)
                fh.write(block)
        if digest.hexdigest() != expected:
            raise ValueError(f"{asset}: checksum")
        with tarfile.open(download, "r:gz") as tar:
            for member in tar:
                parts = member.name.split("/")
                if parts[0] != name or ".." in parts or "" in parts[1:]:
                    raise ValueError(f"{asset}: {member.name} outside its folder")
                if member.isdir():
                    continue
                if not member.isfile() or len(parts) < 2:
                    raise ValueError(f"{asset}: {member.name} isn't a file of the package")
                out = partial.joinpath(*parts[1:])
                out.parent.mkdir(parents=True, exist_ok=True)
                source = tar.extractfile(member)
                with open(out, "wb") as fh:
                    shutil.copyfileobj(source, fh, 1 << 20)
        manifest = partial / "manifest.json"
        if not manifest.is_file():
            raise ValueError(f"{asset}: no manifest.json")
        if entry.get("manifest_sha256") and _sha256(manifest) != entry["manifest_sha256"]:
            raise ValueError(f"{name}/manifest.json: checksum")
        for path, spec in json.loads(manifest.read_text(encoding="utf-8"))["files"].items():
            if ".." in path or not (partial / path).is_file() or _sha256(partial / path) != spec["sha256"]:
                raise ValueError(f"{name}/{path}: checksum")
        shutil.rmtree(done, ignore_errors=True)
        partial.rename(done)
    except BaseException:
        shutil.rmtree(partial, ignore_errors=True)
        raise
    finally:
        download.unlink(missing_ok=True)


def install(languages: List[str], home: Path, runner: hermes.Runner, say: Callable[[str], None],
            base: Optional[str] = None, transport: Optional[httpx.BaseTransport] = None,
            token: Optional[str] = None, release: Optional[str] = None) -> Optional[Path]:
    """The packages of `languages` (fr, en, es, de, nl) and the decoder, in this account's voices folder: the folder,
    or None when they can't be had (the caller installs Pocket's models instead). From `base` (else `source`: the
    guichet of a place of the offer), else from the public repository's release `release` (else the latest that fits
    this checkout)."""
    target = folder(home)
    if base is None and release is None:
        base, token = source(home)
    if runner.dry:
        where = base or (f"the release {release}" if release else "BipAgents' latest voices release on GitHub")
        say(f"  (dry run) would download the voices of {', '.join(languages)} from {where} into {target}")
        return target
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    try:
        with httpx.Client(timeout=httpx.Timeout(30, read=120), follow_redirects=True, transport=transport,
                          headers=headers) as client:
            target.mkdir(parents=True, exist_ok=True)
            if base is not None:
                index = client.get(f"{base}index.json").raise_for_status().json()

                def fetch(entry: dict) -> None:
                    _package(client, base, entry["folder"], target)
            else:
                tag = release or latest_release(client)
                if tag is None:
                    raise ValueError("no voices release for this Pocket yet")
                where = f"{DOWNLOADS}/{tag}/"
                raw_sums = client.get(f"{where}SHA256SUMS").raise_for_status().content
                signature = None
                if release_key.pinned():   # the sums are trusted only once signed by BipAgents' release key
                    answer = client.get(f"{where}{release_key.SIGNATURE}")
                    signature = answer.content if answer.status_code == 200 else None
                release_key.check(raw_sums, signature)
                sums = sums_of(raw_sums.decode("utf-8"))
                raw = client.get(f"{where}index.json").raise_for_status().content
                if hashlib.sha256(raw).hexdigest() != sums.get("index.json"):
                    raise ValueError("index.json: checksum")
                index = json.loads(raw)

                def fetch(entry: dict) -> None:
                    _archive(client, where, sums, entry, target)
            wanted = [NAMES[code] for code in languages if NAMES.get(code) in index.get("languages", {})]
            if not wanted:
                raise ValueError("no package for these languages")
            fetch(index["mimi"])
            kept = {}
            for name in wanted:
                entry = index["languages"][name]
                fetch(entry)
                kept[name] = entry
        (target / "index.json").write_text(json.dumps({"languages": kept, "mimi": index["mimi"]}), encoding="utf-8")
        used = {entry["folder"] for entry in kept.values()} | {index["mimi"]["folder"], "index.json"}
        for item in target.iterdir():
            if item.name not in used:
                shutil.rmtree(item, ignore_errors=True) if item.is_dir() else item.unlink(missing_ok=True)
        return target
    except (httpx.HTTPError, OSError, ValueError, KeyError, TypeError, tarfile.TarError) as exc:
        say(f"  the light voices can't be had ({type(exc).__name__}: {str(exc)[:120]}): Pocket's own models instead")
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
    """The light voices brought to what the guichet now serves, or the latest release has (new packages: a new
    Pocket, new Bips' voices, a new language when this account has all the others), the old ones removed once the new
    are here: whether anything changed (Pocket then restarts on them)."""
    target = folder(home)
    languages = languages_of(target)
    if languages:
        languages = with_new_languages(languages)
    base, token = source(home)
    if not languages:
        return False
    if runner.dry:
        say(f"  (dry run) would fetch the newer voices of {', '.join(languages)} from "
            f"{'the guichet' if base else 'the latest release'}, if any")
        return False
    before = (target / "index.json").read_text(encoding="utf-8")
    said: List[str] = []
    if install(languages, home, runner, said.append, base=base, transport=transport, token=token) is None:
        reason = said[-1].split("(", 1)[-1].split(")", 1)[0] if said else "no answer"
        say(f"  the Bips' newer voices can't be had now ({reason}): they keep the ones they have")
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
    return python_pocket(repo / "pocket" / ".venv" / "bin" / "python")


def python_pocket(python: Path) -> Optional[str]:
    """The pocket-tts version a Python environment has (a Pocket service's own, set up by hand or not)."""
    import subprocess
    if not python.exists():
        return None
    try:
        out = subprocess.run([str(python), "-c", "from importlib.metadata import version; print(version('pocket-tts'))"],
                             capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return (out.stdout.strip() or None) if out.returncode == 0 else None
