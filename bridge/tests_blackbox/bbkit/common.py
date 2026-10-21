"""Constants and small helpers shared by the fakes and the tests (the same values as tests/conftest.py)."""
from __future__ import annotations

import json
import struct
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


def read_sse(resp) -> list:
    """(event, data) pairs of an SSE response (httpx streaming response) until it ends; keepalives skipped."""
    events, name = [], None
    for line in resp.iter_lines():
        if line.startswith("event: "):
            name = line[len("event: "):]
        elif line.startswith("data: "):
            events.append((name, json.loads(line[len("data: "):])))
    return events
