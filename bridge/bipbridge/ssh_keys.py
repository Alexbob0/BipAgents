"""The person's SSH keys for their place's account, from the app: the server is theirs, they may log in to it (as this
account, never root) and bring their own tools. Only on a server of the all-included offer (`cloud`): on someone's
own computer, the app never opens their account to a key.

    GET    /v1/ssh                  {enabled, login?, keys: [{fingerprint, type, comment}]}
    POST   /v1/ssh/keys             {key: "ssh-ed25519 AAAA… comment"} -> the key added
    DELETE /v1/ssh/keys?fingerprint=SHA256:…

A key is one public key line with nothing in front (no `command=` or other options); it goes to
~/.ssh/authorized_keys (700/600), which the account's other keys share."""
from __future__ import annotations

import asyncio
import base64
import hashlib
import os
import pwd
import re
from pathlib import Path
from typing import Any, Dict, List, Optional
from urllib.parse import urlsplit

from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel, Field

TYPES = ("ssh-ed25519", "ecdsa-sha2-nistp256", "ecdsa-sha2-nistp384", "ecdsa-sha2-nistp521", "ssh-rsa",
         "sk-ssh-ed25519@openssh.com", "sk-ecdsa-sha2-nistp256@openssh.com")
LINE = re.compile(r"(?P<type>[a-z0-9@.-]+) (?P<blob>[A-Za-z0-9+/]+={0,3})(?: (?P<comment>[^\r\n]{0,200}))?")
MAX_KEYS = 20


class SshKeyError(ValueError):
    pass


def _home() -> Path:
    return Path(os.environ.get("HOME") or Path.home())


def parse(text: str) -> Dict[str, str]:
    """One public key line, checked: its type, its blob (base64 of a key of that type), its comment."""
    lines = [line for line in text.strip().splitlines() if line.strip()]
    if len(lines) != 1:
        raise SshKeyError("one public key at a time")
    found = LINE.fullmatch(" ".join(lines[0].split()))
    if found is None or found["type"] not in TYPES:
        raise SshKeyError("not a public SSH key (ssh-ed25519 AAAA… name)")
    try:
        blob = base64.b64decode(found["blob"], validate=True)
    except ValueError as exc:
        raise SshKeyError("not a public SSH key (its middle part)") from exc
    if len(blob) < 4 or blob[4:4 + int.from_bytes(blob[:4], "big")].decode("ascii", "replace") != found["type"]:
        raise SshKeyError("not a public SSH key (its type and its content differ)")
    comment = (found["comment"] or "").strip()
    return {"type": found["type"], "blob": found["blob"], "comment": comment,
            "fingerprint": fingerprint(blob)}


def fingerprint(blob: bytes) -> str:
    """As `ssh-keygen -l` shows it: SHA256:… without padding."""
    return "SHA256:" + base64.b64encode(hashlib.sha256(blob).digest()).decode("ascii").rstrip("=")


def _file(home: Path) -> Path:
    return home / ".ssh" / "authorized_keys"


def keys(home: Path) -> List[Dict[str, str]]:
    """The account's keys (the ones it had before the app too); lines that aren't plain keys are left out."""
    try:
        lines = _file(home).read_text(encoding="utf-8").splitlines()
    except OSError:
        return []
    out = []
    for line in lines:
        try:
            key = parse(line)
        except SshKeyError:
            continue
        out.append({k: key[k] for k in ("fingerprint", "type", "comment")})
    return out


def _write(home: Path, lines: List[str]) -> None:
    folder = home / ".ssh"
    folder.mkdir(mode=0o700, exist_ok=True)
    os.chmod(folder, 0o700)
    path = _file(home)
    tmp = path.with_name(".authorized_keys.tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        fh.write("".join(line + "\n" for line in lines))
    os.replace(tmp, path)


def add(home: Path, text: str) -> Dict[str, str]:
    key = parse(text)
    try:
        lines = _file(home).read_text(encoding="utf-8").splitlines()
    except FileNotFoundError:
        lines = []
    if any(k["fingerprint"] == key["fingerprint"] for k in keys(home)):
        return {k: key[k] for k in ("fingerprint", "type", "comment")}
    if len(keys(home)) >= MAX_KEYS:
        raise SshKeyError(f"{MAX_KEYS} keys at most")
    comment = key["comment"] or "added from BipAgents"
    _write(home, lines + [f"{key['type']} {key['blob']} {comment}"])
    return {"fingerprint": key["fingerprint"], "type": key["type"], "comment": comment}


def remove(home: Path, fingerprint_: str) -> bool:
    try:
        lines = _file(home).read_text(encoding="utf-8").splitlines()
    except FileNotFoundError:
        return False
    kept = []
    for line in lines:
        try:
            if parse(line)["fingerprint"] == fingerprint_:
                continue
        except SshKeyError:
            pass
        kept.append(line)
    if len(kept) == len(lines):
        return False
    _write(home, kept)
    return True


def login(public_url: Optional[str]) -> Optional[str]:
    """`ssh bip@203.0.113.7`: this account at the server's address."""
    host = urlsplit(public_url or "").hostname
    if not host:
        return None
    return f"ssh {pwd.getpwuid(os.getuid()).pw_name}@{host}"


class KeyRequest(BaseModel):
    key: str = Field(min_length=20, max_length=16384)


def ssh_router() -> APIRouter:
    router = APIRouter()

    def enabled(request: Request) -> bool:
        return bool(request.app.state.services.config.cloud)

    @router.get("/ssh")
    async def ssh_status(request: Request) -> Dict[str, Any]:
        if not enabled(request):
            return {"enabled": False, "keys": []}
        return {"enabled": True, "login": login(request.app.state.services.config.public_url),
                "keys": await asyncio.to_thread(keys, _home())}

    @router.post("/ssh/keys")
    async def ssh_add(body: KeyRequest, request: Request) -> Dict[str, Any]:
        if not enabled(request):
            raise HTTPException(status_code=403, detail="only on a server of the all-included offer")
        try:
            return await asyncio.to_thread(add, _home(), body.key)
        except SshKeyError as exc:
            raise HTTPException(status_code=400, detail=str(exc)) from exc

    @router.delete("/ssh/keys")
    async def ssh_remove(fingerprint: str, request: Request) -> Dict[str, Any]:
        if not enabled(request):
            raise HTTPException(status_code=403, detail="only on a server of the all-included offer")
        if not await asyncio.to_thread(remove, _home(), fingerprint):
            raise HTTPException(status_code=404, detail="no such key")
        return {"ok": True}

    return router
