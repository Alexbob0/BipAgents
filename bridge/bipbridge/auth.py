"""Single bearer "bridge key" authentication (constant-time comparison)."""
from __future__ import annotations

import hmac
from typing import Optional

from fastapi import HTTPException, Request
from starlette.websockets import WebSocket


def _extract_bearer(header: Optional[str]) -> Optional[str]:
    if not header:
        return None
    scheme, _, token = header.partition(" ")
    if scheme.lower() != "bearer":
        return None
    token = token.strip()
    return token or None


def key_matches(expected: str, presented: Optional[str]) -> bool:
    if not presented:
        # Still run a comparison so timing does not reveal "missing" vs "wrong".
        hmac.compare_digest(expected.encode(), b"\0" * len(expected.encode()))
        return False
    return hmac.compare_digest(expected.encode("utf-8"), presented.encode("utf-8"))


def address_of(request) -> Optional[str]:
    return request.client.host if request.client else None


async def require_bridge_key(request: Request) -> None:
    """The bridge key (the app), or a web console's session (console.py; `request.state.console` then says so). An
    address that kept presenting wrong keys is refused for a while (guard.py)."""
    guard = getattr(request.app.state, "guard", None)
    if guard is not None and guard.blocked(address_of(request)):
        raise HTTPException(status_code=429, detail="too many wrong keys, try again later")
    expected = request.app.state.config.bridge_key
    presented = _extract_bearer(request.headers.get("authorization"))
    if key_matches(expected, presented):
        request.state.console = False
        return
    from .console import is_console
    if is_console(request):
        request.state.console = True
        return
    if guard is not None and presented:
        guard.fail(address_of(request), "bridge key")
    raise HTTPException(status_code=401, detail="unauthorized", headers={"WWW-Authenticate": "Bearer"})


def websocket_authorized(websocket: WebSocket) -> bool:
    guard = getattr(websocket.app.state, "guard", None)
    if guard is not None and guard.blocked(address_of(websocket)):
        return False
    presented = _extract_bearer(websocket.headers.get("authorization"))
    if key_matches(websocket.app.state.config.bridge_key, presented):
        return True
    if guard is not None and presented:
        guard.fail(address_of(websocket), "bridge key")
    return False
