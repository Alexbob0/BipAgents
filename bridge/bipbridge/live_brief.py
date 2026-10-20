"""A Live call continues a conversation from a short brief of it instead of its whole history (100k tokens and more:
a minute of reading for a local model, and a cache broken for the written conversation). The brief:

- the key points, which the agent itself keeps up to date in the background: in a throwaway session (hidden, deleted
  at once), from its previous points and the messages since, once a conversation has had 6 new messages and been quiet
  a minute (its turn is over, the model server's cache is still warm); one at a time, never for Live, scheduled-task
  or throwaway sessions;
- the last exchanges, word for word, read when the brief is asked for: always up to date;
- the calls of the last day that continued it.

The apps give it to the call's own short session as instructions (Hermes adds them to the end of its system prompt),
the same text at every turn of the call so that the model server's cache holds.

    GET  /v1/live/brief?agent=&session=   {session, text, points_at}   (without a session: the agent's latest one)
    POST /v1/live/calls                   {agent, session, live_session}   a call that just ended

Messages and points stay on this server (data folder « briefs », one file per conversation)."""
from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import time
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel, Field

from .config import AgentConfig
from .cronwatch import is_cron_session
from .hermes import HermesClient, HermesHTTPError
from .logs import fields

log = logging.getLogger("bipbridge.live_brief")

RECENT_MESSAGES = 8
MESSAGE_CHARS = 700
RECENT_CHARS = 6000
POINTS_EVERY = 6            # new messages before the key points are written again
MIN_MESSAGES = 10           # shorter conversations: the last exchanges say it all
QUIET_SECONDS = 60          # the turn is over
LOOP_SECONDS = 120
CALLS_KEPT = 3
CALL_HOURS = 24
CALL_CHARS = 2500
POINTS_TITLE = "BipAgents · notes"
LIVE_TITLE = "Live"
# What the apps put in a message for the agent only (a call's opening, a call's recap): never shown to the person.
HIDDEN = re.compile(r"<!--bip:.*?-->", re.S)

POINTS_REQUEST = """<!--bip:notes-->[Request from the BipAgents app, not from the person. Use no tool and save nothing to \
memory: only answer with the text asked for.]
You keep short notes so that a voice call can continue a conversation with the person. Here are your previous notes, \
then the messages since.

Previous notes:
{points}

Messages since:
{messages}

Write the updated notes: what is being discussed, the decisions taken, what you promised or still have to do, the \
names, dates and facts the person gave, their mood if it matters. At most 12 short lines, most important first, in \
the language of the conversation. Only the notes."""


def visible(text: str) -> str:
    return HIDDEN.sub("", text).strip()


def _content(message: Dict[str, Any]) -> str:
    content = message.get("content")
    if isinstance(content, list):
        content = " ".join(str(part.get("text") or "") for part in content if isinstance(part, dict))
    return visible(content) if isinstance(content, str) else ""


def exchanges(messages: List[Dict[str, Any]]) -> List[Tuple[str, str]]:
    """(role, text) of what the person and the agent said, tools and machinery left out."""
    out: List[Tuple[str, str]] = []
    for message in messages:
        role = message.get("role")
        if role not in ("user", "assistant") or message.get("display_kind") == "hidden":
            continue
        text = _content(message)
        if text:
            out.append((role, text))
    return out


def transcript(pairs: List[Tuple[str, str]], limit: int = MESSAGE_CHARS) -> str:
    def line(role: str, text: str) -> str:
        short = text if len(text) <= limit else text[:limit].rsplit(" ", 1)[0] + "…"
        return ("- The person: " if role == "user" else "- You: ") + " ".join(short.split())
    return "\n".join(line(role, text) for role, text in pairs)


def _seconds(value: Any) -> Optional[float]:
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, str):
        try:
            return datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp()
        except ValueError:
            return None
    return None


def is_conversation(session: Dict[str, Any]) -> bool:
    """A conversation with the person: not a call, a scheduled task, a throwaway notes session or a sub-agent."""
    title = str(session.get("title") or "")
    return not (is_cron_session(session) or title.startswith(LIVE_TITLE) or title.startswith(POINTS_TITLE)
                or session.get("is_internal_child") or session.get("hidden"))


class Briefs:
    def __init__(self, folder: str, hermes: HermesClient, agents: Dict[str, AgentConfig]):
        self.folder = Path(folder)
        self.hermes = hermes
        self.agents = agents
        self._writing = asyncio.Lock()

    def _path(self, agent: str, session: str) -> Path:
        safe = re.sub(r"[^\w.-]", "_", session)[:120]
        return self.folder / re.sub(r"[^\w.-]", "_", agent) / f"{safe}.json"

    def load(self, agent: str, session: str) -> Dict[str, Any]:
        try:
            data = json.loads(self._path(agent, session).read_text(encoding="utf-8"))
            return data if isinstance(data, dict) else {}
        except (OSError, ValueError):
            return {}

    def save(self, agent: str, session: str, data: Dict[str, Any]) -> None:
        path = self._path(agent, session)
        path.parent.mkdir(parents=True, exist_ok=True)
        os.chmod(path.parent, 0o700)
        temporary = path.with_suffix(".tmp")
        temporary.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        os.replace(temporary, path)

    async def latest(self, agent: AgentConfig) -> Optional[str]:
        """The agent's most recent conversation."""
        for session in await self.hermes.list_sessions(agent, limit=10):
            if is_conversation(session) and isinstance(session.get("id"), str):
                return session["id"]
        return None

    async def brief(self, agent: AgentConfig, session: str) -> Dict[str, Any]:
        state = self.load(agent.name, session)
        messages = exchanges(await self.hermes.session_messages(agent, session))
        recent: List[Tuple[str, str]] = []
        size = 0
        for role, text in reversed(messages[-RECENT_MESSAGES:]):
            size += min(len(text), MESSAGE_CHARS)
            if recent and size > RECENT_CHARS:
                break
            recent.insert(0, (role, text))
        parts = ["This is a voice call with the person. It continues the conversation summed up below: speak as in that "
                 "conversation, briefly, as one does on the phone."]
        if state.get("points"):
            parts.append("Key points of the conversation so far (your own notes):\n" + str(state["points"]).strip())
        if recent:
            parts.append("Its last exchanges:\n" + transcript(recent))
        calls = await self._calls(agent, state)
        if calls:
            parts.append(calls)
        return {"session": session, "text": "\n\n".join(parts), "points_at": state.get("points_at")}

    async def _calls(self, agent: AgentConfig, state: Dict[str, Any]) -> str:
        out: List[str] = []
        for call in state.get("calls", [])[-CALLS_KEPT:]:
            at = call.get("at") or 0
            if time.time() - at > CALL_HOURS * 3600:
                continue
            try:
                said = exchanges(await self.hermes.session_messages(agent, call["live_session"]))
            except (HermesHTTPError, KeyError, OSError):
                continue
            text = transcript(said, limit=400)[:CALL_CHARS]
            if text:
                out.append(f"Voice call of {datetime.fromtimestamp(at).strftime('%d/%m %H:%M')}:\n{text}")
        return ("Earlier voice calls that continued it:\n" + "\n".join(out)) if out else ""

    def note_call(self, agent: str, session: str, live_session: str) -> None:
        state = self.load(agent, session)
        calls = [call for call in state.get("calls", []) if call.get("live_session") != live_session]
        calls.append({"live_session": live_session, "at": time.time()})
        state["calls"] = calls[-CALLS_KEPT:]
        self.save(agent, session, state)

    def due(self, agent: str, session: Dict[str, Any], now: float) -> bool:
        count = session.get("message_count")
        last = _seconds(session.get("last_active"))
        if not isinstance(count, int) or count < MIN_MESSAGES or not is_conversation(session):
            return False
        if last is not None and now - last < QUIET_SECONDS:
            return False
        done = self.load(agent, str(session.get("id"))).get("count")
        return not isinstance(done, int) or count - done >= POINTS_EVERY or count < done

    async def refresh(self, agent: AgentConfig, session: str, count: Optional[int] = None) -> bool:
        """The key points written again by the agent, from its previous ones and the messages since."""
        async with self._writing:
            state = self.load(agent.name, session)
            messages = await self.hermes.session_messages(agent, session)
            done = state.get("upto") if isinstance(state.get("upto"), int) else 0
            # Compressed by Hermes since (fewer messages than covered): start again from the last ones.
            since = messages[done:] if 0 < done <= len(messages) else messages[-40:]
            text = transcript(exchanges(since))
            if not text:
                return False
            request = POINTS_REQUEST.format(points=str(state.get("points") or "(none yet)").strip(), messages=text)
            scratch = await self.hermes.create_session(agent, POINTS_TITLE)
            try:
                await self.hermes.update_session(agent, scratch, hidden=True)
            except HermesHTTPError:
                pass   # an older Hermes: the session shows a few seconds, then it's deleted
            try:
                points = visible(await self.hermes.session_chat(
                    agent, scratch, request, {"model_options": {"reasoning": {"enabled": False}}}))
            finally:
                try:
                    await self.hermes.delete_session(agent, scratch)
                except HermesHTTPError as exc:
                    log.warning("notes session not deleted", extra=fields(agent=agent.name, status=exc.status))
            if not points:
                return False
            state = self.load(agent.name, session)   # a call may have been noted meanwhile
            state.update({"points": points[:4000], "points_at": time.time(), "upto": len(messages),
                          "count": count if count is not None else len(messages)})
            self.save(agent.name, session, state)
            log.info("live brief points", extra=fields(agent=agent.name, chars=len(points)))
            return True

    async def run(self) -> None:
        """Every two minutes, at most one conversation's key points written again (the one waiting the longest)."""
        await asyncio.sleep(30)
        while True:
            try:
                await self.tick()
            except Exception as exc:  # noqa: BLE001 - the loop must keep running
                log.warning("live brief loop", extra=fields(error=type(exc).__name__))
            await asyncio.sleep(LOOP_SECONDS)

    async def tick(self, now: Optional[float] = None) -> bool:
        now = time.time() if now is None else now
        for agent in self.agents.values():
            try:
                sessions = await self.hermes.list_sessions(agent, limit=10)
            except Exception:  # noqa: BLE001 - an agent down must not stop the others
                continue
            for session in sessions:
                if not self.due(agent.name, session, now):
                    continue
                count = session.get("message_count")
                try:
                    return await self.refresh(agent, str(session["id"]), count if isinstance(count, int) else None)
                except (HermesHTTPError, OSError) as exc:
                    log.warning("live brief points failed", extra=fields(agent=agent.name, error=type(exc).__name__))
                    return False
        return False


class CallEnded(BaseModel):
    agent: str = Field(max_length=200)
    session: str = Field(max_length=300)
    live_session: str = Field(max_length=300)


def live_router() -> APIRouter:
    router = APIRouter()

    def setup(request: Request, name: str) -> Tuple[Briefs, AgentConfig]:
        services = request.app.state.services
        agent = services.config.agent(name)
        if agent is None:
            raise HTTPException(status_code=404, detail="unknown agent")
        return services.briefs, agent

    @router.get("/live/brief")
    async def live_brief(request: Request, agent: str, session: Optional[str] = None) -> Dict[str, Any]:
        briefs, config = setup(request, agent)
        try:
            chosen = session or await briefs.latest(config)
            if chosen is None:
                return {"session": None, "text": "", "points_at": None}
            return await briefs.brief(config, chosen)
        except HermesHTTPError as exc:
            raise HTTPException(status_code=502 if exc.status >= 500 else exc.status, detail="hermes") from exc

    @router.post("/live/calls")
    async def live_call(body: CallEnded, request: Request) -> Dict[str, Any]:
        briefs, agent = setup(request, body.agent)
        briefs.note_call(agent.name, body.session, body.live_session)
        return {"ok": True}

    return router
