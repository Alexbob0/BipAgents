"""One's own Hermes into this place (installer/bipinstall/to_place.py): the app asks for a one-time code, a computer
proves it knows it and sends its export, then the installer puts it in, in place of this place's agents, in a unit of
its own (restarting the bridge must not stop it).

    POST   /v1/import               {state: waiting, code, address, command, command_alone, expires}   a new code
    GET    /v1/import               {state: none | waiting | receiving | importing | done | failed, …}
    DELETE /v1/import               the code withdrawn, or a finished import's result cleared
    GET    /v1/import/hello?nonce=  {fingerprint, challenge, proof}   the computer (no bridge key: the code instead)
    PUT    /v1/import/archive       the export; X-Import-Challenge, X-Import-Proof (an HMAC with the code)

A code lasts an hour and serves once (a sending cut short may start again with it). The computer learns this place's
certificate from the proof (an HMAC over its nonce, the certificate's fingerprint and a challenge) and pins it before
sending anything. Wrong proofs count as wrong keys (guard.py). Only on a place of the all-included offer.

    ~/.local/state/bipagents/import/    code.json, the archive and its passphrase, progress.json (the installer's)
"""
from __future__ import annotations

import asyncio
import hashlib
import hmac
import json
import logging
import os
import secrets
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Dict, Optional, Tuple

from fastapi import APIRouter, HTTPException, Query, Request
from starlette.requests import ClientDisconnect

from .auth import address_of
from .cloud_setup import Challenges
from .config import Config
from .logs import fields

log = logging.getLogger("bipbridge.place_import")

IMPORT_CONTEXT = "bipagents-import-v1"
UPLOAD_CONTEXT = "bipagents-import-upload-v1"
ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"   # the installer's (to_place.py)
CODE_LENGTH = 16
CODE_SECONDS = 3600
STALE_SECONDS = 2 * 3600       # an import still « running » after that stopped half-way (the server restarted)
MAX_BYTES = 10 * 2**30
SPARE_BYTES = 2 * 2**30        # left free on the server beyond the export unpacked (about three times its size)
FOLDER = Path(".local") / "state" / "bipagents" / "import"
UNIT = "bipagents-import"
INSTALLER = ("https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh")


def proof(code: str, *parts: str) -> str:
    return hmac.new(code.encode(), "\n".join(parts).encode(), hashlib.sha256).hexdigest()


def shown(code: str) -> str:
    return "-".join(code[i:i + 4] for i in range(0, len(code), 4))


def _home() -> Path:
    return Path(os.environ.get("HOME") or Path.home())


def _read(path: Path) -> Dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def _write_private(path: Path, data: Any) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        fh.write(data if isinstance(data, str) else json.dumps(data))
    os.replace(tmp, path)


def start_import(folder: Path, archive: Path, passphrase: Path) -> None:
    """The installer's import-place, in a systemd unit of its own."""
    repo = Path(__file__).resolve().parents[2]
    subprocess.Popen(["systemd-run", "--user", "--unit", UNIT, "--collect",
                      f"--setenv=PYTHONPATH={repo / 'installer'}", f"--working-directory={repo}",
                      sys.executable, "-m", "bipinstall", "import-place", "--archive", str(archive),
                      "--passphrase-file", str(passphrase)],
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)


class Imports:
    def __init__(self, home: Optional[Path] = None, clock=time.time, start=None):
        self.home = home            # None: $HOME, read each time
        self.clock = clock
        self.start = start          # None: start_import
        self.challenges = Challenges()
        self.receiving: Optional[Dict[str, int]] = None

    @property
    def folder(self) -> Path:
        return (self.home or _home()) / FOLDER

    def code(self) -> Optional[str]:
        data = _read(self.folder / "code.json")
        code = data.get("code")
        if not isinstance(code, str) or len(code) != CODE_LENGTH or float(data.get("expires") or 0) < self.clock():
            return None
        return code

    def progress(self) -> Dict[str, Any]:
        return _read(self.folder / "progress.json")

    def importing(self) -> bool:
        step = self.progress()
        return bool(step) and not step.get("done") and not step.get("error") \
            and self.clock() - float(step.get("at") or 0) < STALE_SECONDS

    def new_code(self, config: Config) -> Dict[str, Any]:
        if self.receiving or self.importing():
            raise HTTPException(status_code=409, detail="an export is being received or put in")
        code = "".join(secrets.choice(ALPHABET) for _ in range(CODE_LENGTH))
        _write_private(self.folder / "code.json", {"code": code, "expires": self.clock() + CODE_SECONDS})
        (self.folder / "progress.json").unlink(missing_ok=True)
        log.info("import code given")
        return self.status(config)

    def withdraw(self) -> None:
        if self.receiving or self.importing():
            raise HTTPException(status_code=409, detail="an export is being received or put in")
        (self.folder / "code.json").unlink(missing_ok=True)
        (self.folder / "progress.json").unlink(missing_ok=True)

    def status(self, config: Config) -> Dict[str, Any]:
        if self.receiving:
            return {"state": "receiving", **self.receiving}
        step = self.progress()
        if step:
            out = {k: step.get(k) for k in ("step", "total", "label")}
            if step.get("error"):
                return {"state": "failed", "error": step["error"], **out}
            if step.get("done"):
                return {"state": "done", **out}
            if self.clock() - float(step.get("at") or 0) >= STALE_SECONDS:
                return {"state": "failed", "error": "the import stopped half-way", **out}
            return {"state": "importing", **out}
        code = self.code()
        if code is None:
            return {"state": "none"}
        address = (config.public_url or "").removeprefix("https://").rstrip("/")
        expires = float(_read(self.folder / "code.json").get("expires") or 0)
        return {"state": "waiting", "code": shown(code), "address": address, "expires": int(expires),
                "command": f"bipagents send {address} {shown(code)}",
                "command_alone": f"curl -fsSL {INSTALLER} | bash -s -- send {address} {shown(code)}"}

    async def receive(self, request: Request, config: Config) -> Dict[str, Any]:
        code = self.code()
        if code is None:
            raise HTTPException(status_code=403, detail="no code waits: ask the app for a new one")
        if self.receiving or self.importing():
            raise HTTPException(status_code=409, detail="an export is being received or put in")
        challenge = request.headers.get("x-import-challenge", "")
        mac = request.headers.get("x-import-proof", "").lower()
        length = request.headers.get("content-length", "")
        if not length.isdigit():
            raise HTTPException(status_code=411, detail="the export's size is needed")
        size = int(length)
        if not self.challenges.take(challenge) or not hmac.compare_digest(
                proof(code, UPLOAD_CONTEXT, challenge, str(size)), mac):
            guard = getattr(request.app.state, "guard", None)
            if guard is not None:
                guard.fail(address_of(request), "import proof")
            raise HTTPException(status_code=401, detail="wrong proof")
        if size > MAX_BYTES:
            raise HTTPException(status_code=413, detail="an export of 10 GB at most")
        self.folder.mkdir(mode=0o700, parents=True, exist_ok=True)
        if shutil.disk_usage(self.folder).free < size * 3 + SPARE_BYTES:
            raise HTTPException(status_code=507, detail="not enough room on this server")
        part, archive = self.folder / "archive.part", self.folder / "archive.bipagents"
        self.receiving = {"received": 0, "size": size}
        try:
            fd = os.open(part, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, "wb") as out:
                async for chunk in request.stream():
                    self.receiving["received"] += len(chunk)
                    if self.receiving["received"] > size:
                        raise HTTPException(status_code=400, detail="more than announced")
                    out.write(chunk)
            if self.receiving["received"] != size:
                raise HTTPException(status_code=400, detail="the export came incomplete: send it again")
            os.replace(part, archive)
            secret = self.folder / "passphrase"
            _write_private(secret, code)
            (self.folder / "code.json").unlink(missing_ok=True)   # used: never twice
            _write_private(self.folder / "progress.json", {"step": 0, "total": 4, "label": "Received", "done": False,
                                                           "error": None, "at": self.clock()})
            await asyncio.to_thread(self.start or start_import, self.folder, archive, secret)
        except ClientDisconnect:
            part.unlink(missing_ok=True)
            raise HTTPException(status_code=400, detail="the sending stopped: start it again")
        except BaseException:
            part.unlink(missing_ok=True)
            raise
        finally:
            self.receiving = None
        log.info("import received", extra=fields(bytes=size, address=address_of(request)))
        return {"started": True}


def place_import_routers(home: Optional[Path] = None) -> Tuple[APIRouter, APIRouter]:
    """(the app's routes, behind the bridge key; the computer's, proved with the code)."""
    imports = Imports(home)
    admin, open_ = APIRouter(), APIRouter()

    def cloud(request: Request) -> Config:
        config: Config = request.app.state.services.config
        if not config.cloud:
            raise HTTPException(status_code=404, detail="not found")
        return config

    def guarded(request: Request) -> Config:
        config = cloud(request)
        guard = getattr(request.app.state, "guard", None)
        if guard is not None and guard.blocked(address_of(request)):
            raise HTTPException(status_code=429, detail="too many wrong proofs, try again later")
        return config

    @admin.post("/import")
    async def new_code(request: Request) -> Dict[str, Any]:
        return imports.new_code(cloud(request))

    @admin.get("/import")
    async def import_status(request: Request) -> Dict[str, Any]:
        return imports.status(cloud(request))

    @admin.delete("/import", status_code=204)
    async def withdraw(request: Request) -> None:
        cloud(request)
        imports.withdraw()

    @open_.get("/import/hello")
    async def hello(request: Request, nonce: str = Query(..., min_length=32, max_length=128,
                                                         pattern=r"^[0-9a-fA-F]+$")) -> Dict[str, Any]:
        config = guarded(request)
        code = imports.code()
        if code is None or imports.receiving or imports.importing():
            raise HTTPException(status_code=404, detail="not found")
        from .lan import fingerprint
        print_fp = fingerprint(config) or ""
        challenge = imports.challenges.issue()
        return {"fingerprint": print_fp, "challenge": challenge,
                "proof": proof(code, IMPORT_CONTEXT, nonce.lower(), print_fp, challenge)}

    @open_.put("/import/archive")
    async def archive(request: Request) -> Dict[str, Any]:
        return await imports.receive(request, guarded(request))

    return admin, open_
