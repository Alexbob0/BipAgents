"""Notifications for installs without an APNs key: another person's bridge on the same server sends its pushes to
the administrator's bridge (``POST /v1/relay/push``, its own key), which forwards them to APNs with the team key. The
key never leaves the administrator's install; each relaying install has its own key and an hourly quota."""
from __future__ import annotations

import collections
import logging
import re
import time
from typing import Any, Deque, Dict, Optional

import httpx
from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel, Field

from .apns import ApnsResult
from .auth import key_matches, _extract_bearer
from .logs import fields, redact

log = logging.getLogger("bipbridge.relay")

DEVICE_TOKEN = re.compile(r"^[0-9a-f]{32,512}$")
MAX_PAYLOAD_BYTES = 4096  # APNs' own limit for an alert


class RelayClient:
    """The sending side: same `send` as ApnsClient, through another bridge's relay."""

    def __init__(self, url: str, key: str, client: httpx.AsyncClient):
        self.url = url.rstrip("/")
        self.key = key
        self.client = client

    async def send(self, device_token: str, payload: Dict[str, Any], *, environment: str, push_type: str = "alert",
                   priority: int = 10, collapse_id: Optional[str] = None, expiration: Optional[int] = None) -> ApnsResult:
        body = {"token": device_token, "environment": environment, "payload": payload, "push_type": push_type,
                "priority": priority, "collapse_id": collapse_id}
        try:
            resp = await self.client.post(f"{self.url}/v1/relay/push", json=body, timeout=15.0,
                                          headers={"Authorization": f"Bearer {self.key}"})
        except httpx.HTTPError as exc:
            log.warning("relay unreachable", extra=fields(error=type(exc).__name__))
            return ApnsResult(status=503, reason="RelayUnreachable")
        if resp.status_code != 200:
            return ApnsResult(status=resp.status_code, reason=f"Relay{resp.status_code}")
        data = resp.json()
        return ApnsResult(status=int(data.get("status", 0)), reason=data.get("reason"), apns_id=data.get("apns_id"))


class RelayPush(BaseModel):
    token: str = Field(..., max_length=512)
    environment: str
    payload: Dict[str, Any]
    push_type: str = "alert"
    priority: int = 10
    collapse_id: Optional[str] = Field(default=None, max_length=64)


class _Quota:
    def __init__(self) -> None:
        self.sent: Dict[str, Deque[float]] = collections.defaultdict(collections.deque)

    def allow(self, client: str, per_hour: int) -> bool:
        now = time.monotonic()
        window = self.sent[client]
        while window and window[0] < now - 3600:
            window.popleft()
        if len(window) >= per_hour:
            return False
        window.append(now)
        return True


def relay_router() -> APIRouter:
    router = APIRouter()
    quota = _Quota()

    @router.post("/v1/relay/push")
    async def relay_push(body: RelayPush, request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        relay = services.config.relay
        if not relay.enabled:
            raise HTTPException(status_code=404, detail="not found")
        presented = _extract_bearer(request.headers.get("authorization"))
        client = next((name for key, name in relay.clients.items() if key_matches(key, presented)), None)
        if client is None:
            raise HTTPException(status_code=401, detail="unauthorized", headers={"WWW-Authenticate": "Bearer"})
        if services.apns is None:
            raise HTTPException(status_code=503, detail="APNs not configured on the relay")
        token = body.token.strip().lower()
        if not DEVICE_TOKEN.match(token) or body.environment not in ("sandbox", "production") \
                or body.push_type not in ("alert", "background") or body.priority not in (5, 10):
            raise HTTPException(status_code=400, detail="invalid push")
        import json
        if len(json.dumps(body.payload, ensure_ascii=False).encode("utf-8")) > MAX_PAYLOAD_BYTES:
            raise HTTPException(status_code=413, detail="payload too large")
        if not quota.allow(client, relay.per_hour):
            raise HTTPException(status_code=429, detail="quota exceeded")
        result = await services.apns.send(token, body.payload, environment=body.environment, push_type=body.push_type,
                                          priority=body.priority, collapse_id=body.collapse_id)
        log.info("relayed push", extra=fields(client=client, token=redact(token), status=result.status))
        return {"status": result.status, "reason": result.reason, "apns_id": result.apns_id}

    return router
