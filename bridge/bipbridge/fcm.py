"""Firebase Cloud Messaging (HTTP v1) for the Android app, with the Firebase project's service account.

Same contract as ApnsClient (`send` returns an ApnsResult), so PushService and the relay treat both alike. The
APNs-style payload is turned into an FCM *data* message: the app's messaging service builds the notification itself,
fetching the text over the tailnet as the iPhone's notification extension does; nothing readable goes through Google
unless `[push] previews` is on.
"""
from __future__ import annotations

import json
import logging
import re
import time
from typing import Any, Callable, Dict, Optional

import httpx

from .apns import ApnsResult
from .logs import fields, redact

log = logging.getLogger("bipbridge.fcm")

TOKEN_URL = "https://oauth2.googleapis.com/token"
SCOPE = "https://www.googleapis.com/auth/firebase.messaging"
ACCESS_TTL_SECONDS = 50 * 60
# An FCM registration token: base64url with a colon, ~150–200 characters.
FCM_TOKEN = re.compile(r"^[A-Za-z0-9_:\-]{20,4096}$")
DEAD = {"UNREGISTERED", "INVALID_ARGUMENT", "SENDER_ID_MISMATCH"}


def data_message(payload: Dict[str, Any]) -> Dict[str, str]:
    """An APNs payload as FCM data (strings only): the alert's title and body, its category, and the ids."""
    aps = payload.get("aps") or {}
    alert = aps.get("alert") or {}
    data: Dict[str, str] = {}
    if isinstance(alert, dict):
        for key in ("title", "body"):
            if alert.get(key):
                data[key] = str(alert[key])
    if aps.get("category"):
        data["category"] = str(aps["category"])
    if aps.get("content-available") and not alert:
        data["silent"] = "1"
    for key, value in payload.items():
        if key == "aps" or value is None:
            continue
        data[key] = value if isinstance(value, str) else json.dumps(value, ensure_ascii=False)
    return data


class FcmClient:
    def __init__(self, service_account_path: str, client: httpx.AsyncClient, clock: Callable[[], float] = time.time):
        with open(service_account_path, "r", encoding="utf-8") as fh:
            account = json.load(fh)
        self.project = account["project_id"]
        self.email = account["client_email"]
        self.key = account["private_key"]
        self.token_uri = account.get("token_uri") or TOKEN_URL
        self.client = client
        self.clock = clock
        self._access: Optional[str] = None
        self._access_at = 0.0

    async def access_token(self, force: bool = False) -> str:
        now = self.clock()
        if force or self._access is None or now - self._access_at >= ACCESS_TTL_SECONDS:
            import jwt  # here, not at start: a bridge that pushes through the relay never signs (~15 MB less)

            assertion = jwt.encode({"iss": self.email, "scope": SCOPE, "aud": self.token_uri, "iat": int(now),
                                    "exp": int(now) + 3600}, self.key, algorithm="RS256")
            resp = await self.client.post(self.token_uri, data={
                "grant_type": "urn:ietf:params:oauth:grant-type:jwt-bearer", "assertion": assertion}, timeout=15.0)
            resp.raise_for_status()
            self._access = resp.json()["access_token"]
            self._access_at = now
        return self._access

    async def send(self, device_token: str, payload: Dict[str, Any], *, environment: str = "fcm",
                   push_type: str = "alert", priority: int = 10, collapse_id: Optional[str] = None,
                   expiration: Optional[int] = None) -> ApnsResult:
        message: Dict[str, Any] = {
            "token": device_token,
            "data": data_message(payload),
            "android": {"priority": "HIGH" if priority >= 10 else "NORMAL"},
        }
        if collapse_id:
            message["android"]["collapse_key"] = collapse_id[:64]
        result = await self._post(message, force=False)
        if result.status == 401:
            result = await self._post(message, force=True)
        level = logging.INFO if result.ok else logging.WARNING
        log.log(level, "fcm push", extra=fields(token=redact(device_token), status=result.status, reason=result.reason))
        return result

    async def _post(self, message: Dict[str, Any], force: bool) -> ApnsResult:
        try:
            token = await self.access_token(force=force)
            resp = await self.client.post(f"https://fcm.googleapis.com/v1/projects/{self.project}/messages:send",
                                          json={"message": message}, timeout=15.0,
                                          headers={"Authorization": f"Bearer {token}"})
        except httpx.HTTPError as exc:
            log.warning("fcm unreachable", extra=fields(error=type(exc).__name__))
            return ApnsResult(0, "NetworkError")
        if resp.status_code == 200:
            return ApnsResult(200, None, resp.json().get("name"))
        reason = None
        try:
            error = resp.json().get("error", {})
            details = [d.get("errorCode") for d in error.get("details", []) if isinstance(d, dict)]
            reason = next((d for d in details if d), None) or error.get("status")
        except ValueError:
            pass
        # A token FCM will never accept again reads as APNs' 410, so PushService drops the device.
        status = 410 if reason in DEAD and resp.status_code in (400, 404) else resp.status_code
        return ApnsResult(status, reason)
