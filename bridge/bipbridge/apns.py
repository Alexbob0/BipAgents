"""APNs HTTP/2 client with token-based (ES256 JWT) authentication."""
from __future__ import annotations

import json
import logging
import time
from dataclasses import dataclass
from typing import Any, Callable, Dict, Optional

import httpx
import jwt

from .config import ApnsConfig
from .logs import fields, redact

log = logging.getLogger("bipbridge.apns")

HOSTS = {
    "production": "https://api.push.apple.com",
    "sandbox": "https://api.sandbox.push.apple.com",
}
TOKEN_TTL_SECONDS = 50 * 60  # Apple rejects provider tokens older than 60 min
# Reasons meaning "this device token will never work again for this topic".
DEAD_TOKEN_REASONS = {"BadDeviceToken", "Unregistered", "DeviceTokenNotForTopic"}


@dataclass
class ApnsResult:
    status: int
    reason: Optional[str] = None
    apns_id: Optional[str] = None

    @property
    def ok(self) -> bool:
        return self.status == 200

    @property
    def token_is_dead(self) -> bool:
        return self.status == 410 or (self.status == 400 and self.reason in DEAD_TOKEN_REASONS)


class ApnsClient:
    def __init__(self, config: ApnsConfig, client: httpx.AsyncClient,
                 clock: Callable[[], float] = time.time):
        self.config = config
        self.client = client
        self.clock = clock
        self._key: Optional[str] = None
        self._token: Optional[str] = None
        self._token_iat = 0.0

    def _signing_key(self) -> str:
        if self._key is None:
            with open(self.config.p8_path, "r", encoding="utf-8") as fh:
                self._key = fh.read()
        return self._key

    def provider_token(self, force: bool = False) -> str:
        now = self.clock()
        if force or self._token is None or now - self._token_iat >= TOKEN_TTL_SECONDS:
            iat = int(now)
            self._token = jwt.encode({"iss": self.config.team_id, "iat": iat}, self._signing_key(),
                                     algorithm="ES256", headers={"kid": self.config.key_id})
            self._token_iat = iat
        return self._token

    async def send(self, device_token: str, payload: Dict[str, Any], *, environment: str,
                   push_type: str = "alert", priority: int = 10, collapse_id: Optional[str] = None,
                   expiration: Optional[int] = None) -> ApnsResult:
        host = HOSTS.get(environment, HOSTS["production"])
        body = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        result = await self._post(host, device_token, body, push_type, priority, collapse_id, expiration, False)
        if result.status == 403 and result.reason in ("ExpiredProviderToken", "InvalidProviderToken"):
            result = await self._post(host, device_token, body, push_type, priority, collapse_id, expiration, True)
        level = logging.INFO if result.ok else logging.WARNING
        log.log(level, "apns push", extra=fields(token=redact(device_token), env=environment, type=push_type,
                                                 status=result.status, reason=result.reason))
        return result

    async def _post(self, host: str, device_token: str, body: bytes, push_type: str, priority: int,
                    collapse_id: Optional[str], expiration: Optional[int], force_token: bool) -> ApnsResult:
        headers = {
            "authorization": f"bearer {self.provider_token(force=force_token)}",
            "apns-topic": self.config.bundle_id,
            "apns-push-type": push_type,
            "apns-priority": str(priority),
            "content-type": "application/json",
        }
        if collapse_id:
            headers["apns-collapse-id"] = collapse_id[:64]
        if expiration is not None:
            headers["apns-expiration"] = str(expiration)
        try:
            resp = await self.client.post(f"{host}/3/device/{device_token}", content=body, headers=headers,
                                          timeout=15.0)
        except httpx.HTTPError as exc:
            log.warning("apns unreachable", extra=fields(error=type(exc).__name__))
            return ApnsResult(0, "NetworkError")
        reason = None
        if resp.status_code != 200:
            try:
                reason = resp.json().get("reason")
            except ValueError:
                reason = None
        return ApnsResult(resp.status_code, reason, resp.headers.get("apns-id"))
