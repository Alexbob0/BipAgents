"""The web console (`/console/`): the person's agents from a computer's browser, on the tailnet (or the LAN door), served
by this bridge. The page (`console/`) is static; it uses the same routes as the iPhone app, with a session instead of
the bridge key:

- **Logging in**: a one-time link (`python -m bipbridge console`, valid 10 minutes, its code in the link's `#` so it
  never reaches a log) or a code the console shows and the person approves in the iPhone app
  (`POST /v1/console/pairings/{code}`). Either gives a session cookie: HttpOnly (no script reads it), Secure, SameSite
  Strict, 30 days, listed and revocable (`/v1/console/devices`).
- **Calling the bridge**: the cookie plus an `X-Bip-Console: 1` header (a page from elsewhere cannot send it without the
  browser asking first) stands for the bridge key (`auth.require_bridge_key`).
- **Calling an agent's Hermes**: through `/hermes/<agent>/…`, the bridge adding the agent's key itself: no Hermes key
  ever reaches the browser (`/v1/agents` leaves them out for a console).
"""
from __future__ import annotations

import hashlib
import json
import os
import secrets
import time
from pathlib import Path
from typing import Any, Dict, List, Optional

from fastapi import APIRouter, HTTPException, Request, Response
from fastapi.responses import FileResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel, Field

COOKIE = "bip_console"
HEADER = "x-bip-console"
SESSION_DAYS = 30
LINK_MINUTES = 10
PAIRING_MINUTES = 5
PAGE = Path(__file__).with_name("console")


def _digest(token: str) -> str:
    return hashlib.sha256(token.encode()).hexdigest()


class ConsoleStore:
    """Sessions, one-time links and pairings, in `<data>/console.json` (0600): digests only, never a token."""

    def __init__(self, path: str):
        self.path = Path(path)

    def load(self) -> Dict[str, Any]:
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            data = {}
        now = time.time()
        return {"sessions": {k: v for k, v in (data.get("sessions") or {}).items() if v.get("expires", 0) > now},
                "links": {k: v for k, v in (data.get("links") or {}).items() if v > now},
                "pairings": {k: v for k, v in (data.get("pairings") or {}).items() if v.get("expires", 0) > now}}

    def save(self, data: Dict[str, Any]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_name(self.path.name + ".tmp")
        tmp.write_text(json.dumps(data, indent=1), encoding="utf-8")
        os.chmod(tmp, 0o600)
        os.replace(tmp, self.path)

    # -- one-time links (`python -m bipbridge console`)
    def new_link(self) -> str:
        code = secrets.token_urlsafe(24)
        data = self.load()
        data["links"][_digest(code)] = time.time() + LINK_MINUTES * 60
        self.save(data)
        return code

    def use_link(self, code: str) -> bool:
        data = self.load()
        if data["links"].pop(_digest(code), None) is None:
            return False
        self.save(data)
        return True

    # -- pairings approved from the iPhone app
    def new_pairing(self) -> Dict[str, str]:
        """A short code to read on the console, and the secret the console waits with."""
        code = f"{secrets.randbelow(10**6):06d}"
        wait = secrets.token_urlsafe(24)
        data = self.load()
        data["pairings"][code] = {"wait": _digest(wait), "expires": time.time() + PAIRING_MINUTES * 60,
                                  "approved": None}
        self.save(data)
        return {"code": code, "wait": wait}

    def approve(self, code: str, name: str) -> bool:
        data = self.load()
        pairing = data["pairings"].get(code)
        if pairing is None or pairing["approved"]:
            return False
        pairing["approved"] = name or "Navigateur"
        self.save(data)
        return True

    def take_pairing(self, code: str, wait: str) -> Optional[str]:
        """The name the person gave once they approved it (the pairing is then used up), else None."""
        data = self.load()
        pairing = data["pairings"].get(code)
        if pairing is None or not secrets.compare_digest(pairing["wait"], _digest(wait)) or not pairing["approved"]:
            return None
        del data["pairings"][code]
        self.save(data)
        return pairing["approved"]

    # -- sessions
    def new_session(self, name: str) -> str:
        token = secrets.token_urlsafe(32)
        data = self.load()
        now = time.time()
        data["sessions"][_digest(token)] = {"id": secrets.token_hex(6), "name": name[:60], "created": now,
                                            "seen": now, "expires": now + SESSION_DAYS * 86400}
        self.save(data)
        return token

    def valid(self, token: Optional[str]) -> bool:
        if not token:
            return False
        data = self.load()
        session = data["sessions"].get(_digest(token))
        if session is None:
            return False
        if time.time() - session.get("seen", 0) > 3600:   # last use, kept to the hour
            session["seen"] = time.time()
            self.save(data)
        return True

    def end(self, token: Optional[str]) -> None:
        if token:
            data = self.load()
            if data["sessions"].pop(_digest(token), None) is not None:
                self.save(data)

    def devices(self) -> List[Dict[str, Any]]:
        return [{k: v for k, v in s.items() if k != "expires"} for s in self.load()["sessions"].values()]

    def revoke(self, device: str) -> bool:
        data = self.load()
        found = [k for k, s in data["sessions"].items() if s.get("id") == device]
        for key in found:
            del data["sessions"][key]
        if found:
            self.save(data)
        return bool(found)


def is_console(request: Request) -> bool:
    """A console's call: its session cookie, and the header a page from elsewhere cannot send unasked."""
    store: Optional[ConsoleStore] = getattr(request.app.state, "console", None)
    return (store is not None and request.headers.get(HEADER) == "1"
            and store.valid(request.cookies.get(COOKIE)))


def _set_cookie(response: Response, token: str) -> None:
    response.set_cookie(COOKIE, token, max_age=SESSION_DAYS * 86400, httponly=True, secure=True, samesite="strict",
                        path="/")


class LinkLogin(BaseModel):
    code: str = Field(..., max_length=100)
    name: str = Field("Navigateur", max_length=60)


class PairingWait(BaseModel):
    code: str = Field(..., max_length=6)
    wait: str = Field(..., max_length=100)


class Approval(BaseModel):
    name: str = Field("Navigateur", max_length=60)


def console_router() -> APIRouter:
    """The page and its login, without the bridge key (`/console/…`)."""
    router = APIRouter()

    def store(request: Request) -> ConsoleStore:
        return request.app.state.console

    @router.get("/console")
    async def to_page() -> Response:
        return Response(status_code=307, headers={"Location": "/console/"})

    @router.get("/console/")
    async def page() -> FileResponse:
        return FileResponse(PAGE / "index.html", headers=SECURITY_HEADERS)

    @router.post("/console/api/login")
    async def login(body: LinkLogin, request: Request, response: Response) -> Dict[str, bool]:
        if not store(request).use_link(body.code):
            raise HTTPException(status_code=401, detail="this link was used or has expired: ask for a new one")
        _set_cookie(response, store(request).new_session(body.name))
        return {"ok": True}

    @router.post("/console/api/pairing")
    async def pairing(request: Request) -> Dict[str, str]:
        return store(request).new_pairing()

    @router.post("/console/api/pairing/wait")
    async def pairing_wait(body: PairingWait, request: Request, response: Response) -> Dict[str, bool]:
        name = store(request).take_pairing(body.code, body.wait)
        if name is None:
            return {"ok": False}
        _set_cookie(response, store(request).new_session(name))
        return {"ok": True}

    @router.get("/console/api/me")
    async def me(request: Request) -> Dict[str, bool]:
        return {"logged_in": is_console(request)}

    @router.post("/console/api/logout")
    async def logout(request: Request, response: Response) -> Dict[str, bool]:
        store(request).end(request.cookies.get(COOKIE))
        response.delete_cookie(COOKIE, path="/")
        return {"ok": True}

    return router


def console_admin_router() -> APIRouter:
    """For the app (bridge key): approve a console, list and revoke them (`/v1/console/…`)."""
    router = APIRouter()

    @router.post("/console/pairings/{code}")
    async def approve(code: str, body: Approval, request: Request) -> Dict[str, bool]:
        if not request.app.state.console.approve(code, body.name):
            raise HTTPException(status_code=404, detail="no console waits with this code")
        return {"ok": True}

    @router.get("/console/devices")
    async def devices(request: Request) -> Dict[str, Any]:
        return {"devices": request.app.state.console.devices()}

    @router.delete("/console/devices/{device}", status_code=204)
    async def revoke(device: str, request: Request) -> Response:
        if not request.app.state.console.revoke(device):
            raise HTTPException(status_code=404, detail="unknown device")
        return Response(status_code=204)

    return router


# The page runs only its own scripts and styles, talks only to this bridge, and is never framed.
SECURITY_HEADERS = {
    "Content-Security-Policy": "default-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:; "
                               "connect-src 'self'; style-src 'self'; script-src 'self'; frame-ancestors 'none'; "
                               "base-uri 'none'; form-action 'self'",
    "X-Content-Type-Options": "nosniff",
    "Referrer-Policy": "no-referrer",
    "Cache-Control": "no-store",
}


def console_files() -> StaticFiles:
    return StaticFiles(directory=str(PAGE / "static"))
