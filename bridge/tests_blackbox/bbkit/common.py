"""Constants and small helpers shared by the fakes and the tests (the same values as tests/conftest.py)."""
from __future__ import annotations

import contextlib
import json
import struct
import threading
import re
import time
import uuid
from typing import Any

import httpx

BRIDGE_KEY = "test-bridge-key-0123456789abcdef0123456789"
HERMES_KEY = "hermes-wellness-key-xyz"
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


@contextlib.contextmanager
def on_screen(bridge, agent: str = "wellness", thread: str = "main"):
    """The app shows that conversation: its event stream open with `thread=` (presence: nothing pushed for it)."""
    stop = threading.Event()

    def watch():
        with httpx.Client(base_url=bridge.url, headers=AUTH, timeout=30) as client:
            with client.stream("GET", "/v1/threads/events", params={"agent": agent, "thread": thread}) as resp:
                for _ in resp.iter_lines():
                    if stop.is_set():
                        return

    threading.Thread(target=watch, daemon=True).start()
    time.sleep(0.5)
    try:
        yield
    finally:
        stop.set()


def app_turn(bridge, fakes, text: str, session: str, agent: str = "wellness", hermes_agent: str = "") -> str:
    """A turn the app starts in the conversation holding `session` (its one sending route,
    `POST /v1/threads/{id}/messages`): the bridge starts it on Hermes and follows it (the run hub). The session is
    made in the fake Hermes when it has none of that name. The run's id."""
    if session not in fakes.hermes.sessions.get(hermes_agent, {}):
        fakes.hermes.add_session(session, [], agent=hermes_agent)
    thread = bridge.get("/v1/threads/resolve", headers=AUTH, params={"agent": agent, "session_id": session})
    assert thread.status_code == 200, thread.text
    sent = bridge.post(f"/v1/threads/{thread.json()['thread']['id']}/messages", headers=AUTH,
                       json={"agent": agent, "client_id": str(uuid.uuid4()), "text": text})
    assert sent.status_code == 200 and sent.json()["state"] == "started", sent.text
    return sent.json()["run_id"]


def follow_run(bridge, run_id: str, agent: str = "wellness") -> list:
    """The run's events as an app follows them (`GET /v1/runs/{id}/events`), until its end."""
    with bridge.stream("GET", f"/v1/runs/{run_id}/events", params={"agent": agent}, headers=AUTH) as resp:
        assert resp.status_code == 200, resp.read()
        return read_sse(resp)


def words_of(run_input) -> Any:
    """What the person wrote, without the notes the bridge puts before it (`<!--bip:now …-->`, threads/send.rs)."""
    return re.sub(r"^(?:<!--.*?-->\s*)+", "", run_input, flags=re.S) if isinstance(run_input, str) else run_input


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


def boite_items(bridge, agent=None) -> list:
    """The Boîte as the apps read it: the journal's `boite` rows, oldest first, each as its message."""
    params = {"limit": 500, **({"agent": agent} if agent else {})}
    rows = bridge.get("/v1/notifications", headers=AUTH, params=params).json()["notifications"]
    return [{"id": r["ref"], "agent": r["agent"], "session_id": r["session_id"], "created_at": r["created_at"],
             **r["boite"]} for r in reversed(rows) if r["type"] == "boite" and "boite" in r]


def boite_message(bridge, fakes, text: str, name: str = "Le point", notify: bool = True) -> dict:
    """A message in the Boîte as the bridge's own producers put one there: a routine's report (made by the app, run
    now, its turn answering `text`; notify False: a muted routine makes none now, see routines.rs). Returns the Boîte's item once stored."""
    from .servers import wait_until
    made = bridge.post("/v1/agents/wellness/routines", headers=AUTH,
                       json={"name": name, "schedule": "every day at 7:00", "prompt": "Fais le point."})
    assert made.status_code == 200, made.text
    if not notify:
        muted = bridge.put(f"/v1/cron-jobs/wellness/{made.json()['job']['id']}", headers=AUTH, json={"notify": False})
        assert muted.status_code == 200, muted.text
    before = {i["id"] for i in boite_items(bridge)}
    fakes.hermes.run_output = text
    assert bridge.post(f"/v1/routines/{made.json()['job']['id']}/run", headers=AUTH).status_code == 200

    def new():
        return [i for i in boite_items(bridge) if i["id"] not in before]

    assert wait_until(new, timeout=20)
    return new()[0]
