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


async def require_bridge_key(request: Request) -> None:
    expected = request.app.state.config.bridge_key
    if not key_matches(expected, _extract_bearer(request.headers.get("authorization"))):
        raise HTTPException(status_code=401, detail="unauthorized", headers={"WWW-Authenticate": "Bearer"})


def websocket_authorized(websocket: WebSocket) -> bool:
    expected = websocket.app.state.config.bridge_key
    return key_matches(expected, _extract_bearer(websocket.headers.get("authorization")))
