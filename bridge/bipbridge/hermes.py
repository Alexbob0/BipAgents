"""Hermes api_server client: lenient SSE parsing of ``GET /v1/runs/{id}/events`` and approvals."""
from __future__ import annotations

import json
import logging
from dataclasses import dataclass, field
from typing import Any, AsyncIterator, Dict, List, Optional, Tuple

import httpx

from .config import AgentConfig

log = logging.getLogger("bipbridge.hermes")

DELTA_TYPES = {"message.delta", "assistant.delta", "response.output_text.delta"}
TERMINAL_TYPES = {"run.completed", "run.failed", "run.cancelled", "run.interrupted", "done", "error"}
# Events after which buffered text is a complete thought worth speaking right away.
FLUSH_TYPES = {"tool.started", "approval.request", "assistant.completed", "message.completed"}


@dataclass
class SSEEvent:
    type: str
    data: Dict[str, Any] = field(default_factory=dict)
    raw: str = ""

    @property
    def text(self) -> str:
        """Delta text for delta events (``delta``, ``text`` or ``content``; nested dicts tolerated)."""
        for key in ("delta", "text", "content"):
            value = self.data.get(key)
            if isinstance(value, str):
                return value
            if isinstance(value, dict):
                inner = value.get("content") or value.get("text")
                if isinstance(inner, str):
                    return inner
        return ""

    @property
    def final_text(self) -> str:
        """Final answer carried by a ``run.completed`` event, if any."""
        for key in ("output", "final_output", "response", "text", "content", "result"):
            value = self.data.get(key)
            if isinstance(value, str) and value.strip():
                return value
        return ""


class SSEParser:
    """Line-oriented, lenient SSE parser.

    Type comes from the JSON ``type`` (or ``event``) field, else the SSE ``event:`` line. Comment
    lines (``: keepalive``) are ignored. A bare JSON line outside SSE framing is accepted too.
    """

    def __init__(self) -> None:
        self._event_name: Optional[str] = None
        self._data: List[str] = []

    def feed_line(self, line: str) -> Optional[SSEEvent]:
        line = line.rstrip("\r\n")
        if line == "":
            return self._dispatch()
        if line.startswith(":"):
            return None
        stripped = line.strip()
        if stripped.startswith("{") and not self._data and self._event_name is None:
            return self._build(None, stripped)  # bare JSON line (NDJSON-style), tolerated
        name, sep, value = line.partition(":")
        if not sep:
            name, value = line, ""
        if value.startswith(" "):
            value = value[1:]
        if name == "event":
            self._event_name = value.strip() or None
        elif name == "data":
            self._data.append(value)
        return None  # id:, retry: and unknown fields are ignored

    def finish(self) -> Optional[SSEEvent]:
        return self._dispatch()

    def _dispatch(self) -> Optional[SSEEvent]:
        if not self._data and self._event_name is None:
            return None
        name, payload = self._event_name, "\n".join(self._data)
        self._event_name, self._data = None, []
        return self._build(name, payload)

    @staticmethod
    def _build(name: Optional[str], payload: str) -> Optional[SSEEvent]:
        if payload.strip() == "[DONE]":
            return SSEEvent("done", {}, payload)
        data: Dict[str, Any] = {}
        if payload.strip():
            try:
                parsed = json.loads(payload)
                if isinstance(parsed, dict):
                    data = parsed
                else:
                    data = {"value": parsed}
            except ValueError:
                data = {"text": payload}
        etype = data.get("type") or data.get("event") or name or "message"
        if not isinstance(etype, str):
            etype = str(etype)
        return SSEEvent(etype, data, payload)


async def iter_sse(lines: AsyncIterator[str]) -> AsyncIterator[SSEEvent]:
    parser = SSEParser()
    async for line in lines:
        event = parser.feed_line(line)
        if event is not None:
            yield event
    last = parser.finish()
    if last is not None:
        yield last


class HermesHTTPError(Exception):
    def __init__(self, status: int, body: str = ""):
        super().__init__(f"Hermes HTTP {status}")
        self.status = status
        self.body = body


class HermesClient:
    def __init__(self, client: httpx.AsyncClient):
        self.client = client

    @staticmethod
    def _headers(agent: AgentConfig, accept: str = "application/json") -> Dict[str, str]:
        return {"Authorization": f"Bearer {agent.hermes_key}", "Accept": accept}

    async def _get_json(self, agent: AgentConfig, path: str, params: Optional[Dict[str, Any]] = None) -> Any:
        resp = await self.client.get(f"{agent.hermes_url}{path}", params=params, headers=self._headers(agent),
                                     timeout=15.0)
        if resp.status_code != 200:
            raise HermesHTTPError(resp.status_code, resp.text[:500])
        return resp.json()

    async def list_sessions(self, agent: AgentConfig, limit: int = 20) -> List[Dict[str, Any]]:
        """``GET /api/sessions`` (most recent first)."""
        data = await self._get_json(agent, "/api/sessions", {"limit": limit})
        items = data.get("sessions", data.get("data", data.get("items"))) if isinstance(data, dict) else data
        return [item for item in items or [] if isinstance(item, dict)]

    async def session_messages(self, agent: AgentConfig, session_id: str) -> List[Dict[str, Any]]:
        """``GET /api/sessions/{id}/messages`` (oldest first)."""
        data = await self._get_json(agent, f"/api/sessions/{session_id}/messages", {"inline_images": "false"})
        items = data.get("messages", data.get("data", data.get("items"))) if isinstance(data, dict) else data
        return [item for item in items or [] if isinstance(item, dict)]

    async def run_events(self, agent: AgentConfig, run_id: str) -> AsyncIterator[SSEEvent]:
        """Yield events of ``GET /v1/runs/{run_id}/events`` until the server closes the stream.
        Raises :class:`HermesHTTPError` on a non-200 answer."""
        url = f"{agent.hermes_url}/v1/runs/{run_id}/events"
        timeout = httpx.Timeout(connect=5.0, read=60.0, write=10.0, pool=5.0)  # keepalives every 10 s
        async with self.client.stream("GET", url, headers=self._headers(agent, "text/event-stream"),
                                      timeout=timeout) as resp:
            if resp.status_code != 200:
                body = (await resp.aread()).decode("utf-8", "replace")[:500]
                raise HermesHTTPError(resp.status_code, body)
            async for event in iter_sse(resp.aiter_lines()):
                yield event

    async def approve(self, agent: AgentConfig, run_id: str, choice: str,
                      request_id: Optional[str] = None) -> Tuple[int, Any]:
        body: Dict[str, Any] = {"choice": choice}
        if request_id:
            body["request_id"] = request_id
        resp = await self.client.post(f"{agent.hermes_url}/v1/runs/{run_id}/approval", json=body,
                                      headers=self._headers(agent), timeout=15.0)
        try:
            payload: Any = resp.json()
        except ValueError:
            payload = {"detail": resp.text[:500]}
        return resp.status_code, payload
