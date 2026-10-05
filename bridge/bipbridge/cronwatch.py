"""Scheduled-task replies → Boîte + push, whatever the job's delivery settings.

Hermes cron jobs deliver where their ``deliver`` says (SimpleX, Telegram…), but every run also leaves a
session (``cron_<job>_<YYYYMMDD>_<HHMMSS>``). The watcher polls each agent's recent sessions and puts the
final reply of each finished cron run into the outbox — which stores it, pre-synthesizes its audio and
pushes it like an ntfy message — so nothing a job says is missed in the app.

- A reply is taken once it stopped changing between two polls (the run is over).
- ``[SILENT]`` (nothing new to report) is skipped.
- Only runs from the last ``window_hours`` count, so a first start does not replay old history;
  each session is stored once (``ntfy_id = hermes-session:<id>``), so restarts never duplicate.
"""
from __future__ import annotations

import asyncio
import logging
import re
from datetime import datetime, timedelta
from typing import Any, Dict, List, Optional, Set

from .config import AgentConfig, Config
from .hermes import HermesClient
from .logs import fields
from .ntfy import OutboxService
from .store import Store

log = logging.getLogger("bipbridge.cronwatch")

_CRON_ID = re.compile(r"^cron_(?P<job>.+?)_(?P<date>\d{8})_(?P<time>\d{6})$")
SILENT = "[SILENT]"


def is_cron_session(session: Dict[str, Any]) -> bool:
    sid = str(session.get("id") or session.get("session_id") or "")
    return sid.startswith("cron_") or str(session.get("source") or "").lower() == "cron"


def session_started(session: Dict[str, Any]) -> Optional[datetime]:
    """Local time the run started: from the id (``cron_<job>_20261005_075008``), else a timestamp field."""
    sid = str(session.get("id") or session.get("session_id") or "")
    match = _CRON_ID.match(sid)
    if match:
        try:
            return datetime.strptime(match.group("date") + match.group("time"), "%Y%m%d%H%M%S")
        except ValueError:
            pass
    for key in ("started_at", "created_at", "created"):
        value = session.get(key)
        if isinstance(value, (int, float)):
            return datetime.fromtimestamp(value)
        if isinstance(value, str):
            try:
                return datetime.fromisoformat(value.replace("Z", "+00:00")).astimezone().replace(tzinfo=None)
            except ValueError:
                continue
    return None


def job_id(session_id: str) -> Optional[str]:
    match = _CRON_ID.match(session_id)
    return match.group("job") if match else None


def job_name(title: Optional[str]) -> Optional[str]:
    """« Podcast du matin · Oct 05 07:53 » → « Podcast du matin »."""
    if not title:
        return None
    return title.split(" · ")[0].strip() or None


def message_text(message: Dict[str, Any]) -> str:
    content = message.get("content", message.get("text"))
    if isinstance(content, str):
        return content.strip()
    if isinstance(content, list):
        parts = [p.get("text", "") for p in content if isinstance(p, dict) and p.get("type") in ("text", "output_text")]
        return "".join(parts).strip()
    return ""


def final_reply(messages: List[Dict[str, Any]]) -> Optional[str]:
    """The run's last assistant text, if the conversation ends on the assistant."""
    for message in reversed(messages):
        role = str(message.get("role") or "").lower()
        if role == "assistant":
            text = message_text(message)
            if text:
                return text
            continue  # tool-call turn without text
        if role in ("tool", "function"):
            continue
        return None  # ends on the prompt: still running
    return None


class CronWatcher:
    def __init__(self, config: Config, hermes: HermesClient, outbox: OutboxService,
                 interval_seconds: float = 60.0, window_hours: float = 12.0, store: Optional[Store] = None):
        self.config = config
        self.store = store
        self.hermes = hermes
        self.outbox = outbox
        self.interval = interval_seconds
        self.window = timedelta(hours=window_hours)
        self._done: Set[str] = set()          # session ids already stored or skipped
        self._candidate: Dict[str, str] = {}  # session id -> reply seen at the previous poll

    async def run(self) -> None:
        await asyncio.sleep(10)
        while True:
            for agent in self.config.agents.values():
                try:
                    await self.poll(agent)
                except Exception as exc:  # network, Hermes restarting… try again next round
                    log.info("cron watch poll failed", extra=fields(agent=agent.name, error=type(exc).__name__))
            await asyncio.sleep(self.interval)

    async def poll(self, agent: AgentConfig, now: Optional[datetime] = None) -> int:
        """One round for one agent; returns how many replies were stored."""
        now = now or datetime.now()
        stored = 0
        for session in await self.hermes.list_sessions(agent, limit=20):
            sid = str(session.get("id") or session.get("session_id") or "")
            key = f"{agent.name}/{sid}"
            if not sid or key in self._done or not is_cron_session(session):
                continue
            started = session_started(session)
            if started is None or now - started > self.window:
                self._done.add(key)
                continue
            reply = final_reply(await self.hermes.session_messages(agent, sid))
            if reply is None:
                continue
            if reply.strip() == SILENT:
                self._done.add(key)
                continue
            if self._candidate.get(key) != reply:
                self._candidate[key] = reply  # take it once it stops changing
                continue
            self._candidate.pop(key, None)
            self._done.add(key)
            title = session.get("title") if isinstance(session.get("title"), str) else None
            notify = await self._note_job(agent, sid, title)
            item = await self.outbox.ingest(agent, {"message": reply, "title": title,
                                                    "id": f"hermes-session:{sid}", "tags": [f"session:{sid}"]},
                                            notify=notify)
            if item is not None:
                stored += 1
                log.info("cron reply stored", extra=fields(agent=agent.name, session=sid, chars=len(reply)))
        return stored

    async def _note_job(self, agent: AgentConfig, session_id: str, title: Optional[str]) -> bool:
        """Remembers the job (for the app's « Tâches planifiées » settings); False if it is muted."""
        job = job_id(session_id)
        if job is None or self.store is None:
            return True
        return await self.store.note_cron_job(agent.name, job, job_name(title))
