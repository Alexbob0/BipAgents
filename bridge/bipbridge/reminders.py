"""The person's iPhone Reminders, for the agents they choose (decided 2026-10-20). No server reaches them (Apple has no
API for them), so the iPhone app shares them: it sends its lists and reminders here when it opens (and when woken by
a silent push), and applies what the agents asked for, through EventKit. An agent reads the last copy and asks for
a reminder to be added or ticked off; the request waits here until the iPhone has done it.

    reminders_list       the reminders as the iPhone last sent them (not done, and done in the last 30 days)
    reminders_add        a new reminder (a title, a date, a list), done by the iPhone at its next sync
    reminders_complete   a reminder ticked off, the same way

App routes (the bridge key):

    GET  /v1/reminders                 {agents, at, lists: n, items: n}
    PUT  /v1/reminders                 {lists: [{id, title}], items: [{id, list, title, notes, due, completed}]}
    PUT  /v1/reminders/agents          {agents}
    GET  /v1/reminders/queue           the agents' requests waiting for the iPhone
    POST /v1/reminders/queue/{id}      {done: bool, error?}

    data_dir/reminders.json   (chmod 600: the copy, the agents allowed, the requests)
"""
from __future__ import annotations

import asyncio
import json
import os
import secrets
import threading
import time
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel, Field

from .config import Config

LOCK = threading.Lock()
KEPT_DONE = 200
QUEUE_DAYS = 14

REMINDERS_LIST = {
    "name": "reminders_list",
    "description": ("The person's iPhone Reminders as their phone last shared them: open ones, and those done in the "
                    "last 30 days. Optional: a list's name, or include_done."),
    "inputSchema": {"type": "object", "properties": {"list": {"type": "string"}, "include_done": {"type": "boolean"}}},
}
REMINDERS_ADD = {
    "name": "reminders_add",
    "description": ("Add a reminder to the person's iPhone Reminders (it appears when their iPhone next syncs, usually "
                    "within minutes). due: an ISO date or date-time, optional; list: a list's name, optional."),
    "inputSchema": {"type": "object", "properties": {"title": {"type": "string"}, "due": {"type": "string"},
                                                     "list": {"type": "string"}, "notes": {"type": "string"}},
                    "required": ["title"]},
}
REMINDERS_COMPLETE = {
    "name": "reminders_complete",
    "description": "Tick off one of the person's reminders (its id from reminders_list).",
    "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]},
}
REMINDER_TOOLS = [REMINDERS_LIST, REMINDERS_ADD, REMINDERS_COMPLETE]
REMINDER_TOOL_NAMES = {t["name"] for t in REMINDER_TOOLS}


def _path(config: Config) -> Path:
    return Path(config.outbox.db_path).parent / "reminders.json"


def read(config: Config) -> Dict[str, Any]:
    try:
        data = json.loads(_path(config).read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def write(config: Config, data: Dict[str, Any]) -> None:
    path = _path(config)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".tmp")
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        json.dump(data, fh)
    os.replace(temporary, path)


def _text(text: str, error: bool = False) -> Dict[str, Any]:
    return {"content": [{"type": "text", "text": text}], "isError": error}


def _age(seconds: float) -> str:
    minutes = int(seconds // 60)
    return f"{minutes} min" if minutes < 120 else f"{minutes // 60} h" if minutes < 2880 else f"{minutes // 1440} days"


def call(config: Config, agent: str, name: str, arguments: Dict[str, Any], wake: Optional[Callable[[], Any]] = None,
         now: Optional[float] = None) -> Dict[str, Any]:
    now = now or time.time()
    with LOCK:
        data = read(config)
        if agent not in data.get("agents", []):
            return _text("The person hasn't shared their iPhone Reminders with you: they do it in the BipAgents app "
                         "(Settings › Rappels).", error=True)
        lists = {l["id"]: l["title"] for l in data.get("lists", [])}
        if name == REMINDERS_LIST["name"]:
            wanted = str(arguments.get("list") or "").lower()
            done = bool(arguments.get("include_done"))
            rows = [i for i in data.get("items", []) if (done or not i.get("completed"))
                    and (not wanted or lists.get(i.get("list"), "").lower() == wanted)]
            if not rows:
                return _text("No reminder." + (f" (as of {_age(now - data['at'])} ago)" if data.get("at") else ""))
            lines = [f"- id {i['id']} · {lists.get(i.get('list'), '?')} · {i['title']}" + (f" · due {i['due']}" if i.get("due") else "")
                     + (" · done" if i.get("completed") else "") + (f"\n  {i['notes'][:200]}" if i.get("notes") else "")
                     for i in rows[:200]]
            return _text(f"As the iPhone shared them {_age(now - float(data.get('at') or now))} ago:\n" + "\n".join(lines))
        queue = data.setdefault("queue", [])
        if name == REMINDERS_ADD["name"]:
            title = str(arguments.get("title") or "").strip()
            if not title:
                return _text("a title is needed", error=True)
            op = {"id": secrets.token_hex(6), "op": "add", "agent": agent, "created": now, "state": "pending",
                  "args": {"title": title[:300], "due": str(arguments.get("due") or "")[:40],
                           "list": str(arguments.get("list") or "")[:120], "notes": str(arguments.get("notes") or "")[:2000]}}
        elif name == REMINDERS_COMPLETE["name"]:
            reminder = str(arguments.get("id") or "")
            if not any(i["id"] == reminder for i in data.get("items", [])):
                return _text("unknown reminder id (see reminders_list)", error=True)
            op = {"id": secrets.token_hex(6), "op": "complete", "agent": agent, "created": now, "state": "pending",
                  "args": {"id": reminder}}
        else:
            return _text(f"unknown tool: {name}", error=True)
        queue.append(op)
        write(config, data)
    if wake is not None:
        wake()
    return _text("Asked: the person's iPhone does it at its next sync (usually within minutes, or when the app opens).")


# -- the app's routes --------------------------------------------------------------------------------------------------

class Item(BaseModel):
    id: str = Field(..., max_length=200)
    list: str = Field(default="", max_length=200)
    title: str = Field(default="", max_length=1000)
    notes: str = Field(default="", max_length=4000)
    due: str = Field(default="", max_length=40)
    completed: bool = False


class ReminderList(BaseModel):
    id: str = Field(..., max_length=200)
    title: str = Field(default="", max_length=200)


class Snapshot(BaseModel):
    lists: List[ReminderList] = Field(default_factory=list, max_length=200)
    items: List[Item] = Field(default_factory=list, max_length=5000)


class Agents(BaseModel):
    agents: List[str] = Field(default_factory=list, max_length=50)


class Done(BaseModel):
    done: bool
    error: Optional[str] = Field(default=None, max_length=300)


def reminders_router() -> APIRouter:
    router = APIRouter()

    def config_of(request: Request) -> Config:
        return request.app.state.services.config

    @router.get("/reminders")
    async def status(request: Request) -> Dict[str, Any]:
        data = await asyncio.to_thread(read, config_of(request))
        return {"agents": data.get("agents", []), "at": data.get("at"), "lists": len(data.get("lists", [])),
                "items": len(data.get("items", [])),
                "pending": sum(1 for op in data.get("queue", []) if op.get("state") == "pending")}

    @router.put("/reminders")
    async def snapshot(body: Snapshot, request: Request) -> Dict[str, Any]:
        config = config_of(request)

        def save() -> int:
            with LOCK:
                data = read(config)
                done = [i.model_dump() for i in body.items if i.completed][:KEPT_DONE]
                data.update(lists=[l.model_dump() for l in body.lists],
                            items=[i.model_dump() for i in body.items if not i.completed] + done, at=time.time())
                cutoff = time.time() - QUEUE_DAYS * 86400
                data["queue"] = [op for op in data.get("queue", []) if op.get("state") == "pending" or op["created"] > cutoff]
                write(config, data)
                return len(data["items"])

        return {"items": await asyncio.to_thread(save)}

    @router.put("/reminders/agents")
    async def agents(body: Agents, request: Request) -> Dict[str, Any]:
        config = config_of(request)
        if any(config.agent(a) is None for a in body.agents):
            raise HTTPException(status_code=400, detail="unknown agent")

        def save() -> List[str]:
            with LOCK:
                data = read(config)
                data["agents"] = [config.agent(a).name for a in body.agents]
                write(config, data)
                return data["agents"]

        return {"agents": await asyncio.to_thread(save)}

    @router.get("/reminders/queue")
    async def queue(request: Request) -> Dict[str, Any]:
        data = await asyncio.to_thread(read, config_of(request))
        return {"queue": [{k: op[k] for k in ("id", "op", "args", "agent", "created")} for op in data.get("queue", [])
                          if op.get("state") == "pending"]}

    @router.post("/reminders/queue/{op_id}")
    async def done(op_id: str, body: Done, request: Request) -> Dict[str, Any]:
        config = config_of(request)

        def save() -> bool:
            with LOCK:
                data = read(config)
                for op in data.get("queue", []):
                    if op["id"] == op_id:
                        op.update(state="done" if body.done else "failed", error=body.error, finished=time.time())
                        write(config, data)
                        return True
                return False

        if not await asyncio.to_thread(save):
            raise HTTPException(status_code=404, detail="no such request")
        return {"ok": True}

    return router
