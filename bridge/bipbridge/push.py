"""Push notifications to the agent's registered devices (SPEC §A6 / §C2 payloads).

Content travels on the tailnet, not through Apple: by default alerts carry a generic body and the
Notification Service Extension (``mutable-content: 1``) fetches the full text from the bridge.
Set ``[push] previews = true`` to put a short preview in the alert body instead.
"""
from __future__ import annotations

import asyncio
import logging
import time
import uuid
from collections import OrderedDict
from typing import Any, Dict, List, Optional, Tuple

from .apns import ApnsClient
from .config import Config
from .logs import fields, redact
from .store import Store

log = logging.getLogger("bipbridge.push")

MESSAGE_BODY = "Nouveau message"
TEXTS_KEPT = 200
TEXT_TTL = 24 * 3600.0
REPLY_BODY = "Ta réponse est prête."
QUESTION_BODY = "Une question pour toi"
APPROVAL_BODY = "Approbation requise"


def _alert(title: str, preview: Optional[str], fallback: str) -> Dict[str, Any]:
    """The alert: the preview when there is one, else a generic body that iOS translates (``loc-key`` is looked
    up in the app's string catalog, whose keys are these French texts; ``body`` is kept for older apps)."""
    if preview:
        return {"title": title, "body": preview}
    return {"title": title, "body": fallback, "loc-key": fallback}


def _preview(text: str, limit: int = 160) -> str:
    text = " ".join(text.split())
    return text if len(text) <= limit else text[: limit - 1].rstrip() + "…"


def message_payload(agent: str, title: str, outbox_id: str, session_id: Optional[str] = None,
                    preview: Optional[str] = None) -> Dict[str, Any]:
    payload: Dict[str, Any] = {
        "aps": {
            "alert": _alert(title, preview, MESSAGE_BODY),
            "thread-id": agent,
            "mutable-content": 1,
            "category": "MESSAGE",
            "sound": "default",
        },
        "outbox_id": outbox_id,
        "agent": agent,
    }
    if session_id:
        payload["session_id"] = session_id
    return payload


def approval_payload(agent: str, title: str, run_id: str, request_id: Optional[str],
                     choices: Optional[List[str]] = None, preview: Optional[str] = None) -> Dict[str, Any]:
    payload: Dict[str, Any] = {
        "aps": {
            "alert": _alert(title, preview, APPROVAL_BODY),
            "thread-id": agent,
            "mutable-content": 1,
            "category": "APPROVAL",
            "sound": "default",
        },
        "agent": agent,
        "run_id": run_id,
    }
    if request_id:
        payload["request_id"] = request_id
    if choices:
        payload["choices"] = list(choices)
    return payload


def reply_payload(agent: str, title: str, run_id: str, session_id: Optional[str] = None,
                  preview: Optional[str] = None, reply_id: Optional[str] = None) -> Dict[str, Any]:
    """A run finished while nobody followed it: tapping opens its conversation."""
    payload: Dict[str, Any] = {
        "aps": {
            "alert": _alert(title, preview, REPLY_BODY),
            "thread-id": agent,
            "mutable-content": 1,  # the extension shows it as a message from the agent's Bip
            "category": "MESSAGE",
            "sound": "default",
        },
        "agent": agent,
        "run_id": run_id,
        "kind": "reply",
    }
    if session_id:
        payload["session_id"] = session_id
    if reply_id:
        payload["reply_id"] = reply_id  # the extension fetches the text on the device (GET /v1/replies/{id})
    return payload


def question_payload(agent: str, title: str, run_id: str, request_id: Optional[str], session_id: Optional[str] = None,
                     preview: Optional[str] = None, reply_id: Optional[str] = None) -> Dict[str, Any]:
    """The agent asks something mid-run (clarify) while nobody follows it: tapping opens its conversation."""
    payload: Dict[str, Any] = {
        "aps": {
            "alert": _alert(title, preview, QUESTION_BODY),
            "thread-id": agent,
            "mutable-content": 1,
            "category": "MESSAGE",
            "sound": "default",
        },
        "agent": agent,
        "run_id": run_id,
        "kind": "question",
    }
    if request_id:
        payload["request_id"] = request_id
    if session_id:
        payload["session_id"] = session_id
    if reply_id:
        payload["reply_id"] = reply_id
    return payload


def silent_payload(agent: str, reason: str, **extra: Any) -> Dict[str, Any]:
    payload: Dict[str, Any] = {"aps": {"content-available": 1}, "agent": agent, "reason": reason}
    payload.update({k: v for k, v in extra.items() if v is not None})
    return payload


class PushService:
    def __init__(self, config: Config, store: Store, apns: Optional[ApnsClient]):
        self.config = config
        self.store = store
        self.apns = apns
        if apns is None:
            log.warning("APNs not configured: push notifications disabled")
        # Texts of recent reply / question pushes, fetched by the Notification Service Extension over the
        # tailnet (GET /v1/replies/{id}): the alert shows the message, which never goes through Apple.
        self._texts: "OrderedDict[str, Tuple[float, str]]" = OrderedDict()

    def remember(self, text: str) -> str:
        reply_id = uuid.uuid4().hex
        self._texts[reply_id] = (time.time(), text)
        while len(self._texts) > TEXTS_KEPT or (self._texts and next(iter(self._texts.values()))[0] < time.time() - TEXT_TTL):
            self._texts.popitem(last=False)
        return reply_id

    def text(self, reply_id: str) -> Optional[str]:
        entry = self._texts.get(reply_id)
        return entry[1] if entry and entry[0] >= time.time() - TEXT_TTL else None

    def _title(self, agent: str) -> str:
        cfg = self.config.agent(agent)
        return cfg.display_name if cfg else agent

    async def _fanout(self, agent: str, payload: Dict[str, Any], push_type: str, priority: int,
                      collapse_id: Optional[str] = None) -> int:
        if self.apns is None:
            return 0
        devices = await self.store.devices_for_agent(agent)
        if not devices:
            return 0
        results = await asyncio.gather(*[
            self.apns.send(d["token"], payload, environment=d["environment"], push_type=push_type,
                           priority=priority, collapse_id=collapse_id)
            for d in devices], return_exceptions=True)
        sent = 0
        for device, result in zip(devices, results):
            if isinstance(result, BaseException):
                log.warning("apns send crashed", extra=fields(error=type(result).__name__))
                continue
            if result.ok:
                sent += 1
            elif result.token_is_dead:
                await self.store.delete_device(device["token"])
                log.info("device token removed", extra=fields(token=redact(device["token"]), reason=result.reason))
        return sent

    async def notify_message(self, agent: str, outbox_id: str, text: str, session_id: Optional[str] = None,
                             title: Optional[str] = None) -> int:
        preview = _preview(text) if self.config.push_previews else None
        payload = message_payload(agent, self._title(agent), outbox_id, session_id, preview)
        return await self._fanout(agent, payload, "alert", 10)

    async def notify_approval(self, agent: str, run_id: str, request_id: Optional[str],
                              choices: Optional[List[str]] = None, command: Optional[str] = None) -> int:
        preview = None
        if self.config.push_previews and command:
            preview = APPROVAL_BODY + " : " + _preview(command, 120)
        payload = approval_payload(agent, self._title(agent), run_id, request_id, choices, preview)
        return await self._fanout(agent, payload, "alert", 10, collapse_id=request_id or run_id)

    async def notify_reply(self, agent: str, run_id: str, text: str, session_id: Optional[str] = None) -> int:
        preview = _preview(text) if self.config.push_previews else None
        payload = reply_payload(agent, self._title(agent), run_id, session_id, preview, self.remember(text))
        return await self._fanout(agent, payload, "alert", 10, collapse_id=run_id)

    async def notify_question(self, agent: str, run_id: str, request_id: Optional[str], question: Optional[str],
                              session_id: Optional[str] = None) -> int:
        preview = _preview(question) if self.config.push_previews and question else None
        reply_id = self.remember(question) if question else None
        payload = question_payload(agent, self._title(agent), run_id, request_id, session_id, preview, reply_id)
        return await self._fanout(agent, payload, "alert", 10, collapse_id=request_id or run_id)

    async def notify_silent(self, agent: str, reason: str, **extra: Any) -> int:
        return await self._fanout(agent, silent_payload(agent, reason, **extra), "background", 5)
