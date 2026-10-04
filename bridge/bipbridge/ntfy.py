"""ntfy -> outbox: one long-lived ``GET {ntfy}/{topic}/json`` subscription per agent.

Each ``message`` event is stored in the outbox, its audio pre-synthesized (Kyutai mp3, background
priority so live voice is never delayed), then pushed (``MESSAGE``) to the agent's devices. The
last ntfy message id is persisted and sent back as ``since=`` on reconnection, so nothing is lost
or duplicated across restarts (within ntfy's cache window).
"""
from __future__ import annotations

import asyncio
import base64
import json
import logging
import os
import re
from datetime import datetime, timezone
from typing import Any, Dict, Optional, Set, Tuple

import httpx

from .config import AgentConfig, Config
from .logs import fields
from .push import PushService
from .store import Store, iso
from .tts import BACKGROUND, TtsError, TtsService

log = logging.getLogger("bipbridge.ntfy")

BACKOFF_START, BACKOFF_MAX = 0.5, 8.0
_SESSION_TAG = re.compile(r"^session[:_=](.+)$")


_CRON_HEADER = re.compile(r"^\s*Cronjob Response:\s*(?P<name>[^\n]*?)\s*(?:\(job_id:\s*[\w-]+\))?\s*\n\s*-{3,}\s*\n?")
# Footer notes Hermes appends (wording varies by version), possibly several.
_CRON_FOOTER = re.compile(r"\n\s*(?:Note: The agent cannot see this message|To stop or manage this job)[^\n]*\s*$")


def unwrap_cron(text: str) -> Tuple[Optional[str], str]:
    """Hermes wraps cron deliveries in a header (« Cronjob Response: <job> (job_id: …) » + dashes) and a
    footer note. Returns ``(job name, agent output)``; other messages come back unchanged."""
    text = text.strip()
    match = _CRON_HEADER.match(text)
    if not match:
        return None, text
    body = text[match.end():].strip()
    while True:
        trimmed = _CRON_FOOTER.sub("", "\n" + body).strip()
        if trimmed == body:
            break
        body = trimmed
    return (match.group("name").strip() or None), body


def auth_header(token: Optional[str]) -> Dict[str, str]:
    if not token:
        return {}
    if ":" in token and not token.startswith("tk_"):
        return {"Authorization": "Basic " + base64.b64encode(token.encode()).decode()}
    return {"Authorization": f"Bearer {token}"}


class OutboxService:
    """Stores messages, synthesizes their audio and sends the push."""

    def __init__(self, config: Config, store: Store, tts: TtsService, push: PushService):
        self.config = config
        self.store = store
        self.tts = tts
        self.push = push
        self._audio_tasks: Dict[str, "asyncio.Task[Optional[str]]"] = {}
        self._tasks: Set["asyncio.Task[Any]"] = set()

    def audio_file(self, item_id: str) -> str:
        return os.path.join(self.config.outbox.audio_dir, f"{item_id}.mp3")

    async def ingest(self, agent: AgentConfig, message: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        job_title, text = unwrap_cron(str(message.get("message") or ""))
        if not text:
            return None
        title = message.get("title") if isinstance(message.get("title"), str) else job_title
        session_id = None
        for tag in message.get("tags") or []:
            match = _SESSION_TAG.match(str(tag))
            if match:
                session_id = match.group(1)
        sent_at = None
        if isinstance(message.get("time"), (int, float)):
            sent_at = iso(datetime.fromtimestamp(message["time"], tz=timezone.utc))
        item = await self.store.add_outbox(agent.name, text, title=title, ntfy_id=message.get("id"),
                                           sent_at=sent_at, session_id=session_id)
        if item is None:
            return None  # duplicate (already ingested)
        log.info("outbox message stored", extra=fields(agent=agent.name, id=item["id"], chars=len(text)))
        task = asyncio.create_task(self._deliver(agent, item))
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)
        return item

    async def _deliver(self, agent: AgentConfig, item: Dict[str, Any]) -> None:
        """Wait (bounded) for the pre-synthesized audio, then push."""
        audio_task = self.ensure_audio(item, agent)
        if audio_task is not None:
            try:
                await asyncio.wait_for(asyncio.shield(audio_task), timeout=self.config.outbox.audio_wait_seconds)
            except (asyncio.TimeoutError, Exception):
                pass  # push now; the audio route serves it once ready
        try:
            await self.push.notify_message(agent.name, item["id"], item["text"], item.get("session_id"),
                                           item.get("title"))
        except Exception:
            log.exception("outbox push failed", extra=fields(agent=agent.name, id=item["id"]))

    async def close(self) -> None:
        pending = list(self._tasks) + list(self._audio_tasks.values())
        for task in pending:
            task.cancel()
        if pending:
            await asyncio.gather(*pending, return_exceptions=True)

    def ensure_audio(self, item: Dict[str, Any], agent: Optional[AgentConfig] = None
                     ) -> Optional["asyncio.Task[Optional[str]]"]:
        if item.get("audio_path") and os.path.exists(item["audio_path"]):
            return None
        if len(item["text"]) > self.config.outbox.audio_max_chars:
            return None
        task = self._audio_tasks.get(item["id"])
        if task is None:
            agent = agent or self.config.agent(item["agent"])
            voice = (agent.voice if agent else None) or self.config.default_voice
            task = asyncio.create_task(self._synthesize(item, voice))
            self._audio_tasks[item["id"]] = task
            task.add_done_callback(lambda _t, i=item["id"]: self._audio_tasks.pop(i, None))
        return task

    async def _synthesize(self, item: Dict[str, Any], voice: str) -> Optional[str]:
        try:
            audio = await self.tts.synthesize(item["text"], voice, "mp3", BACKGROUND, max_chars=0)
        except TtsError as exc:
            log.warning("outbox audio failed", extra=fields(id=item["id"], error=str(exc)))
            return None
        path = self.audio_file(item["id"])
        os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
        tmp = path + ".part"
        with open(tmp, "wb") as fh:
            fh.write(audio.data)
        os.replace(tmp, path)
        await self.store.set_outbox_audio(item["id"], path)
        return path

    async def purge(self) -> int:
        removed = await self.store.purge_outbox(self.config.outbox.retention_days)
        for row in removed:
            if row.get("audio_path"):
                try:
                    os.unlink(row["audio_path"])
                except OSError:
                    pass
        return len(removed)


class NtfySubscriber:
    def __init__(self, client: httpx.AsyncClient, base_url: str, agent: AgentConfig, store: Store,
                 outbox: OutboxService, backoff_start: float = BACKOFF_START, backoff_max: float = BACKOFF_MAX):
        self.client = client
        self.base_url = base_url.rstrip("/")
        self.agent = agent
        self.store = store
        self.outbox = outbox
        self.backoff_start = backoff_start
        self.backoff_max = backoff_max
        self.connected = False

    async def run(self) -> None:
        backoff = self.backoff_start
        while True:
            try:
                got_data = await self._stream_once()
                if got_data:
                    backoff = self.backoff_start
            except asyncio.CancelledError:
                raise
            except httpx.HTTPError as exc:
                log.info("ntfy stream lost", extra=fields(agent=self.agent.name, error=type(exc).__name__))
            except Exception:
                log.exception("ntfy subscriber error", extra=fields(agent=self.agent.name))
            self.connected = False
            await asyncio.sleep(backoff)
            backoff = min(backoff * 2, self.backoff_max)

    async def _stream_once(self) -> bool:
        params: Dict[str, str] = {}
        cursor = await self.store.get_cursor(self.agent.name)
        if cursor:
            params["since"] = cursor
        url = f"{self.base_url}/{self.agent.ntfy_topic}/json"
        timeout = httpx.Timeout(connect=5.0, read=120.0, write=10.0, pool=5.0)  # ntfy keepalive ~45 s
        got_data = False
        async with self.client.stream("GET", url, params=params, headers=auth_header(self.agent.ntfy_token),
                                      timeout=timeout) as resp:
            if resp.status_code != 200:
                log.warning("ntfy refused subscription", extra=fields(agent=self.agent.name,
                                                                      status=resp.status_code))
                return False
            self.connected = True
            async for line in resp.aiter_lines():
                line = line.strip()
                if not line:
                    continue
                try:
                    event = json.loads(line)
                except ValueError:
                    continue
                got_data = True
                if event.get("event") != "message":
                    continue
                await self.outbox.ingest(self.agent, event)
                if event.get("id"):
                    await self.store.set_cursor(self.agent.name, str(event["id"]))
        return got_data


async def ntfy_reachable(client: httpx.AsyncClient, base_url: str) -> bool:
    try:
        resp = await client.get(f"{base_url.rstrip('/')}/v1/health", timeout=2.0)
        return resp.status_code == 200
    except httpx.HTTPError:
        return False
