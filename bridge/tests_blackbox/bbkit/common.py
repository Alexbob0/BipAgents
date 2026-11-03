"""Constants and small helpers shared by the fakes and the tests (the same values as tests/conftest.py)."""
from __future__ import annotations

import json
import struct
import time
from typing import Any

BRIDGE_KEY = "test-bridge-key-0123456789abcdef0123456789"
HERMES_KEY = "hermes-wellness-key-xyz"
NTFY_TOKEN = "tk_testtoken"
AUTH = {"Authorization": f"Bearer {BRIDGE_KEY}"}
# An address on the Internet, as the bridge sees it through X-Forwarded-For from 127.0.0.1 (uvicorn's
# `proxy_headers` with `forwarded_allow_ips="127.0.0.1"`): wrong keys are only counted for such addresses.
REMOTE = "203.0.113.50"


def make_wav(pcm: bytes, rate: int = 24000, channels: int = 1, bits: int = 16, tag: int = 1) -> bytes:
    block = channels * bits // 8
    fmt = struct.pack("<HHIIHH", tag, channels, rate, rate * block, block, bits)
    return (b"RIFF" + struct.pack("<I", 4 + 8 + len(fmt) + 8 + len(pcm)) + b"WAVE"
            + b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(pcm)) + pcm)


def pcm_for(text: str) -> bytes:
    """Deterministic fake audio: one int16 sample per character."""
    return b"".join(struct.pack("<h", (ord(c) % 3000)) for c in text)


def sse(event_type: str, **data: Any) -> str:
    payload = {"event": event_type, **data}
    return f"event: {event_type}\ndata: {json.dumps(payload)}\n\n"


def read_sse(resp, timeout: float = 120.0) -> list:
    """(event, data) pairs of an SSE response (httpx streaming response) until it ends; keepalives skipped. A stream
    that never ends fails after `timeout` s: its keepalives (every 10 s) would keep the client's read timeout from ever
    firing, and the run would hang instead of failing."""
    events, name = [], None
    deadline = time.monotonic() + timeout
    for line in resp.iter_lines():
        if time.monotonic() > deadline:
            raise AssertionError(f"the event stream did not end within {timeout:g} s; read so far: {events}")
        if line.startswith("event: "):
            name = line[len("event: "):]
        elif line.startswith("data: "):
            events.append((name, json.loads(line[len("data: "):])))
    return events


def in_session(bridge, payload, session: str) -> bool:
    """A push's conversation (`c`, a thread of the bridge) holds this Hermes session (docs/conversations.md §9.3)."""
    thread = payload.get("c")
    if not thread:
        return False
    found = bridge.get(f"/v1/threads/{thread}", headers=AUTH).json()
    return any(s["session_id"] == session for s in found.get("segments", []))


def pushed_text(bridge, payload) -> str:
    """The text a push's extension shows: its journal row's (`GET /v1/notifications/{n}`)."""
    return bridge.get(f"/v1/notifications/{payload['n']}", headers=AUTH).json().get("preview")


def answered(bridge, agent: str) -> list:
    """The decisions the bridge kept for an agent's conversations (approvals_log.rs), oldest first: what the timeline
    shows as answered approvals."""
    path = bridge.data / "approvals" / f"{agent}.jsonl"
    rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()] if path.exists() else []
    return sorted([r for r in rows if r.get("choice")], key=lambda r: r.get("at") or 0)
