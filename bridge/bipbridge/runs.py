"""Run hub: one upstream ``/v1/runs/{id}/events`` subscription per (agent, run), fanned out.

Why one subscription: a Hermes run keeps a single event buffer, so two independent subscribers
(the voice WebSocket and the background approval watcher) could steal events from each other.
The hub keeps a replay backlog so a voice client that attaches after ``POST /v1/watch`` still
hears the answer from its beginning.

Followers are the voice WebSocket and the app's SSE (``GET /v1/runs/{id}/events``): while one is attached
the user sees the run. Push rule (SPEC §B3.4): on ``approval.request`` for a *watched* run with no
follower, push an ``APPROVAL`` notification (once per request); when such a run completes, push a
« reply ready » alert that opens its conversation.
"""
from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import time
from collections import deque
from typing import Any, Deque, Dict, List, Optional, Set, Tuple

import httpx

from .config import AgentConfig
from .hermes import DELTA_TYPES, TERMINAL_TYPES, HermesClient, HermesHTTPError, SSEEvent
from .logs import fields
from .push import PushService

log = logging.getLogger("bipbridge.runs")

BACKLOG_EVENTS = 5000
FINISHED_RETENTION_SECONDS = 900.0  # an app re-attaching later still replays the whole run
BACKOFF_START, BACKOFF_MAX = 0.5, 8.0
GIVE_UP_AFTER = 40  # ~5 min of consecutive connection failures ends even a watch
UNREACHABLE_AFTER = 3  # consecutive connection failures before voice listeners are told

Key = Tuple[str, str]


def _append_to_backlog(backlog: Deque[SSEEvent], event: SSEEvent) -> None:
    """Consecutive text deltas are merged in the replay backlog: a long reply is thousands of token
    deltas, which would push its beginning out of the bounded backlog (a late follower then saw the
    reply start mid-sentence). Live listeners still get every delta as it comes."""
    last = backlog[-1] if backlog else None
    delta = event.data.get("delta")
    if (last is not None and event.type in DELTA_TYPES and last.type == event.type
            and isinstance(delta, str) and isinstance(last.data.get("delta"), str)):
        data = {**last.data, "delta": last.data["delta"] + delta}
        backlog[-1] = SSEEvent(event.type, data, json.dumps(data, ensure_ascii=False))
        return
    backlog.append(event)


class Listener:
    def __init__(self, hub: "RunHub", sub: "RunSubscription", kind: str):
        self.hub = hub
        self.sub = sub
        self.kind = kind
        self.queue: "asyncio.Queue[Optional[SSEEvent]]" = asyncio.Queue()
        self.closed = False

    def __aiter__(self) -> "Listener":
        return self

    async def __anext__(self) -> SSEEvent:
        event = await self.queue.get()
        if event is None:
            raise StopAsyncIteration
        return event

    def close(self) -> None:
        if not self.closed:
            self.closed = True
            self.hub._remove_listener(self)


class RunSubscription:
    def __init__(self, agent: AgentConfig, run_id: str):
        self.agent = agent
        self.run_id = run_id
        self.backlog: Deque[SSEEvent] = deque(maxlen=BACKLOG_EVENTS)
        self.listeners: Set[Listener] = set()
        self.watched = False
        self.finished = False
        self.terminal: Optional[str] = None
        self.task: Optional["asyncio.Task[None]"] = None
        self.pushed: Set[str] = set()
        self.session_id: Optional[str] = None  # for the « reply ready » push

    @property
    def key(self) -> Key:
        return (self.agent.name, self.run_id)

    @property
    def followers(self) -> int:
        """Listeners showing the run to the user (voice WebSocket, app SSE)."""
        return sum(1 for listener in self.listeners if listener.kind in ("voice", "app"))


class RunHub:
    def __init__(self, hermes: HermesClient, push: Optional[PushService], watch_max_seconds: int = 7200,
                 retention_seconds: float = FINISHED_RETENTION_SECONDS):
        self.hermes = hermes
        self.push = push
        self.watch_max_seconds = watch_max_seconds
        self.retention_seconds = retention_seconds
        self._subs: Dict[Key, RunSubscription] = {}
        self._pending: Dict[Tuple[str, str, str], Dict[str, Any]] = {}
        self._tasks: Set["asyncio.Task[Any]"] = set()

    # -- public API ------------------------------------------------------------------------------

    def subscribe(self, agent: AgentConfig, run_id: str, kind: str = "voice", watch: bool = False) -> Listener:
        sub = self._get_or_start(agent, run_id)
        if watch:
            sub.watched = True
        listener = Listener(self, sub, kind)
        for event in sub.backlog:
            listener.queue.put_nowait(event)
        if sub.finished:
            listener.queue.put_nowait(None)
        sub.listeners.add(listener)
        return listener

    def watch(self, agent: AgentConfig, run_id: str) -> RunSubscription:
        sub = self._get_or_start(agent, run_id)
        sub.watched = True
        return sub

    def subscription(self, agent: str, run_id: str) -> Optional[RunSubscription]:
        return self._subs.get((agent, run_id))

    def pending_approvals(self, agent: Optional[str] = None) -> List[Dict[str, Any]]:
        items = [dict(v) for v in self._pending.values() if agent is None or v["agent"] == agent]
        return sorted(items, key=lambda item: item["created_at"])

    def forget_approval(self, agent: str, run_id: str, request_id: Optional[str] = None) -> None:
        for key in list(self._pending):
            if key[0] == agent and key[1] == run_id and (not request_id or self._pending[key].get("request_id") in (None, request_id)):
                del self._pending[key]

    async def close(self) -> None:
        for sub in list(self._subs.values()):
            if sub.task is not None:
                sub.task.cancel()
        tasks = [s.task for s in self._subs.values() if s.task is not None] + list(self._tasks)
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
        self._subs.clear()

    # -- internals -------------------------------------------------------------------------------

    def _get_or_start(self, agent: AgentConfig, run_id: str) -> RunSubscription:
        key = (agent.name, run_id)
        sub = self._subs.get(key)
        if sub is None:
            sub = RunSubscription(agent, run_id)
            self._subs[key] = sub
            sub.task = asyncio.create_task(self._upstream(sub))
        return sub

    def _remove_listener(self, listener: Listener) -> None:
        sub = listener.sub
        sub.listeners.discard(listener)
        if not sub.listeners and not sub.watched and not sub.finished:
            if sub.task is not None:
                sub.task.cancel()
            if self._subs.get(sub.key) is sub:
                del self._subs[sub.key]

    def _spawn(self, coro: Any) -> None:
        task = asyncio.create_task(coro)
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)

    async def _upstream(self, sub: RunSubscription) -> None:
        try:
            await asyncio.wait_for(self._upstream_loop(sub), timeout=self.watch_max_seconds)
        except asyncio.TimeoutError:
            await self._dispatch(sub, SSEEvent("bridge.error", {"type": "bridge.error", "code": "watch_timeout"}))
        except asyncio.CancelledError:
            pass
        except Exception:  # pragma: no cover - defensive
            log.exception("run subscription crashed", extra=fields(agent=sub.agent.name, run_id=sub.run_id))
        finally:
            sub.finished = True
            for listener in list(sub.listeners):
                listener.queue.put_nowait(None)
            try:
                loop = asyncio.get_running_loop()
                loop.call_later(self.retention_seconds, self._drop, sub)
            except RuntimeError:  # pragma: no cover - loop closing
                pass

    def _drop(self, sub: RunSubscription) -> None:
        if self._subs.get(sub.key) is sub:
            del self._subs[sub.key]

    async def _upstream_loop(self, sub: RunSubscription) -> None:
        backoff = BACKOFF_START
        failures = 0
        while True:
            try:
                async for event in self.hermes.run_events(sub.agent, sub.run_id):
                    backoff = BACKOFF_START
                    failures = 0
                    await self._dispatch(sub, event)
                    if event.type in TERMINAL_TYPES:
                        return
            except HermesHTTPError as exc:
                log.warning("run events refused", extra=fields(agent=sub.agent.name, run_id=sub.run_id,
                                                               status=exc.status))
                if exc.status in (400, 401, 403, 404, 410):
                    code = "run_not_found" if exc.status in (404, 410) else "hermes_refused"
                    await self._dispatch(sub, SSEEvent("bridge.error", {"type": "bridge.error", "code": code,
                                                                        "status": exc.status}))
                    return
            except httpx.HTTPError as exc:
                failures += 1
                log.info("run events stream lost", extra=fields(agent=sub.agent.name, run_id=sub.run_id,
                                                                error=type(exc).__name__, failures=failures))
                if failures == UNREACHABLE_AFTER:
                    # Tell live listeners (not stored in the backlog: the watch may still recover).
                    notice = SSEEvent("bridge.error", {"type": "bridge.error", "code": "hermes_unreachable"})
                    for listener in list(sub.listeners):
                        listener.queue.put_nowait(notice)
                    if not sub.watched:
                        return
                if failures >= GIVE_UP_AFTER:
                    return
            if not sub.listeners and not sub.watched:
                return
            await asyncio.sleep(backoff)
            backoff = min(backoff * 2, BACKOFF_MAX)

    async def _dispatch(self, sub: RunSubscription, event: SSEEvent) -> None:
        _append_to_backlog(sub.backlog, event)
        for listener in list(sub.listeners):
            listener.queue.put_nowait(event)
        if event.type == "approval.request":
            self._on_approval(sub, event)
        elif event.type == "approval.responded":
            self.forget_approval(sub.agent.name, sub.run_id, event.data.get("request_id"))
        elif event.type in TERMINAL_TYPES:
            sub.terminal = event.type
            self.forget_approval(sub.agent.name, sub.run_id)
            if sub.watched and sub.followers == 0 and self.push is not None:
                output = event.data.get("output") if isinstance(event.data.get("output"), str) else None
                if event.type == "run.completed" and output:
                    log.info("reply push", extra=fields(agent=sub.agent.name, run_id=sub.run_id))
                    self._spawn(self.push.notify_reply(sub.agent.name, sub.run_id, output, sub.session_id))
                else:
                    self._spawn(self.push.notify_silent(sub.agent.name, "run_finished", run_id=sub.run_id,
                                                        status=event.type))

    def _on_approval(self, sub: RunSubscription, event: SSEEvent) -> None:
        data = event.data
        request_id = data.get("request_id") if isinstance(data.get("request_id"), str) else None
        command = data.get("command") if isinstance(data.get("command"), str) else None
        dedupe = request_id or hashlib.sha256((command or event.raw).encode("utf-8")).hexdigest()[:16]
        choices = data.get("choices") if isinstance(data.get("choices"), list) else ["once", "deny"]
        self._pending[(sub.agent.name, sub.run_id, dedupe)] = {
            "agent": sub.agent.name, "run_id": sub.run_id, "request_id": request_id, "command": command,
            "description": data.get("description") if isinstance(data.get("description"), str) else None,
            "choices": choices, "created_at": time.time(),
        }
        if not sub.watched or sub.followers > 0 or dedupe in sub.pushed or self.push is None:
            log.info("approval seen, no push", extra=fields(agent=sub.agent.name, run_id=sub.run_id,
                                                            watched=sub.watched, followers=sub.followers))
            return
        sub.pushed.add(dedupe)
        log.info("approval push", extra=fields(agent=sub.agent.name, run_id=sub.run_id))
        self._spawn(self.push.notify_approval(sub.agent.name, sub.run_id, request_id, choices, command))
