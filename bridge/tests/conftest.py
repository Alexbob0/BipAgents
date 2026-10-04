"""Shared fakes: Kyutai, Hermes (runs SSE + approval), ntfy and APNs behind httpx.MockTransport.

Nothing here talks to the network. Fakes may be driven from the TestClient thread, so waits use
``threading.Event`` polled with short ``asyncio.sleep`` calls (safe across event loops)."""
from __future__ import annotations

import asyncio
import json
import os
import struct
import sys
import threading
import time
from typing import Any, Dict, List, Optional

import httpx
import pytest
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import ec

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from bipbridge.config import parse_config  # noqa: E402

BRIDGE_KEY = "test-bridge-key-0123456789abcdef0123456789"
HERMES_KEY = "hermes-wellness-key-xyz"
NTFY_TOKEN = "tk_testtoken"
AUTH = {"Authorization": f"Bearer {BRIDGE_KEY}"}
KYUTAI = "http://kyutai.test"
NTFY = "http://ntfy.test"
HERMES = "http://hermes-wellness.test"


def make_wav(pcm: bytes, rate: int = 24000, channels: int = 1, bits: int = 16, tag: int = 1) -> bytes:
    block = channels * bits // 8
    fmt = struct.pack("<HHIIHH", tag, channels, rate, rate * block, block, bits)
    return (b"RIFF" + struct.pack("<I", 4 + 8 + len(fmt) + 8 + len(pcm)) + b"WAVE"
            + b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(pcm)) + pcm)


def pcm_for(text: str) -> bytes:
    """Deterministic fake audio: one int16 sample per character."""
    return b"".join(struct.pack("<h", (ord(c) % 3000)) for c in text)


async def wait_flag(flag: threading.Event, timeout: float = 10.0) -> None:
    deadline = time.monotonic() + timeout
    while not flag.is_set() and time.monotonic() < deadline:
        await asyncio.sleep(0.01)


def wait_until(predicate, timeout: float = 5.0, interval: float = 0.02) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(interval)
    return predicate()


class FakeBackend:
    """Kyutai + Hermes + ntfy on one MockTransport."""

    def __init__(self) -> None:
        self.kyutai_inputs: List[Dict[str, Any]] = []
        self.kyutai_delay = 0.0
        self.kyutai_active = 0
        self.kyutai_max_active = 0
        self.kyutai_status = 200
        self.kyutai_lock = threading.Lock()
        # run_id -> list of SSE chunks (str) or threading.Event (wait until set)
        self.runs: Dict[str, List[Any]] = {}
        self.run_status: Dict[str, int] = {}
        self.run_requests: List[httpx.Request] = []
        self.approvals: List[Dict[str, Any]] = []
        self.approval_response = (200, {"object": "hermes.run.approval_response", "resolved": 1})
        self.ntfy_lines: Dict[str, List[str]] = {}
        self.ntfy_requests: List[httpx.Request] = []
        self.release = threading.Event()  # set at teardown to end hanging streams

    def transport(self) -> httpx.MockTransport:
        return httpx.MockTransport(self.handle)

    async def handle(self, request: httpx.Request) -> httpx.Response:
        url = request.url
        base = f"{url.scheme}://{url.host}"
        if base == KYUTAI:
            return await self._kyutai(request)
        if base == HERMES:
            return self._hermes(request)
        if base == NTFY:
            return self._ntfy(request)
        return httpx.Response(599, text="unexpected host")

    async def _kyutai(self, request: httpx.Request) -> httpx.Response:
        if request.url.path == "/health":
            return httpx.Response(200, json={"status": "ok"})
        body = json.loads(request.content)
        with self.kyutai_lock:
            self.kyutai_active += 1
            self.kyutai_max_active = max(self.kyutai_max_active, self.kyutai_active)
            self.kyutai_inputs.append(body)
        try:
            if self.kyutai_delay:
                await asyncio.sleep(self.kyutai_delay)
        finally:
            with self.kyutai_lock:
                self.kyutai_active -= 1
        if self.kyutai_status != 200:
            return httpx.Response(self.kyutai_status, text="boom")
        fmt = body["response_format"]
        if fmt == "wav":
            return httpx.Response(200, content=make_wav(pcm_for(body["input"])),
                                  headers={"content-type": "audio/wav"})
        return httpx.Response(200, content=b"ID3" + body["input"].encode(), headers={"content-type": "audio/mpeg"})

    def _hermes(self, request: httpx.Request) -> httpx.Response:
        parts = request.url.path.strip("/").split("/")
        if request.headers.get("authorization") != f"Bearer {HERMES_KEY}":
            return httpx.Response(401, json={"error": "bad key"})
        if len(parts) == 4 and parts[:2] == ["v1", "runs"] and parts[3] == "events":
            run_id = parts[2]
            self.run_requests.append(request)
            if run_id.startswith("down"):
                raise httpx.ConnectError("connection refused", request=request)
            if run_id not in self.runs:
                return httpx.Response(self.run_status.get(run_id, 404), json={"error": "not found"})
            script = self.runs[run_id]

            async def stream():
                for item in script:
                    if isinstance(item, threading.Event):
                        await wait_flag(item)
                        continue
                    yield item.encode("utf-8")
                # stay open like a real SSE stream until the test releases it
                await wait_flag(self.release, 30)

            return httpx.Response(200, content=stream(), headers={"content-type": "text/event-stream"})
        if len(parts) == 4 and parts[:2] == ["v1", "runs"] and parts[3] == "approval":
            self.approvals.append({"run_id": parts[2], "body": json.loads(request.content)})
            status, payload = self.approval_response
            return httpx.Response(status, json=payload)
        return httpx.Response(404, json={"error": "no route"})

    def _ntfy(self, request: httpx.Request) -> httpx.Response:
        if request.url.path == "/v1/health":
            return httpx.Response(200, json={"healthy": True})
        self.ntfy_requests.append(request)
        topic = request.url.path.strip("/").split("/")[0]
        lines = self.ntfy_lines.pop(topic, [])

        async def stream():
            yield (json.dumps({"id": "open1", "event": "open", "topic": topic}) + "\n").encode()
            for line in lines:
                yield (line + "\n").encode()
            await wait_flag(self.release, 30)

        return httpx.Response(200, content=stream())


def sse(event_type: str, **data: Any) -> str:
    payload = {"event": event_type, **data}
    return f"event: {event_type}\ndata: {json.dumps(payload)}\n\n"


class FakeApns:
    def __init__(self) -> None:
        self.requests: List[httpx.Request] = []
        self.responses: Dict[str, Any] = {}  # token -> (status, reason)

    def transport(self) -> httpx.MockTransport:
        return httpx.MockTransport(self.handle)

    def handle(self, request: httpx.Request) -> httpx.Response:
        self.requests.append(request)
        token = request.url.path.rsplit("/", 1)[-1]
        status, reason = self.responses.get(token, (200, None))
        if status == 200:
            return httpx.Response(200, headers={"apns-id": "abc"})
        return httpx.Response(status, json={"reason": reason})

    def payloads(self) -> List[Dict[str, Any]]:
        return [json.loads(r.content) for r in self.requests]


@pytest.fixture
def p8_path(tmp_path) -> str:
    key = ec.generate_private_key(ec.SECP256R1())
    pem = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                            serialization.NoEncryption())
    path = tmp_path / "AuthKey_TESTKEY123.p8"
    path.write_bytes(pem)
    return str(path)


@pytest.fixture
def config_dict(tmp_path, p8_path) -> Dict[str, Any]:
    return {
        "bridge_key": BRIDGE_KEY,
        "data_dir": str(tmp_path / "data"),
        "kyutai": {"url": KYUTAI, "default_voice": "5476"},
        "ntfy": {"url": NTFY},
        "apns": {"team_id": "TEAM123456", "key_id": "TESTKEY123", "p8_path": p8_path,
                 "bundle_id": "io.github.bipagents", "environment": "sandbox"},
        "outbox": {"audio_wait_seconds": 2},
        "limits": {"upload_max_mb": 1, "purge_interval_hours": 1000},
        "agents": {
            "Wellness": {
                "display_name": "Wellness", "hermes_url": HERMES, "hermes_key": HERMES_KEY,
                "ntfy_topic": "hermes-wellness-out", "ntfy_token": NTFY_TOKEN,
                "upload_dir_host": str(tmp_path / "uploads" / "wellness"),
                "upload_dir_container": "/home/hermes/.hermes/profiles/wellness/uploads",
                "voice": "4193",
            },
        },
    }


@pytest.fixture
def config(config_dict):
    return parse_config(config_dict)


@pytest.fixture
def backend():
    fake = FakeBackend()
    yield fake
    fake.release.set()


@pytest.fixture
def apns():
    return FakeApns()


@pytest.fixture
def client(config, backend, apns):
    from fastapi.testclient import TestClient

    from bipbridge.app import create_app

    app = create_app(config, transport=backend.transport(), apns_transport=apns.transport())
    with TestClient(app) as test_client:
        test_client.backend = backend  # type: ignore[attr-defined]
        test_client.apns = apns  # type: ignore[attr-defined]
        yield test_client
        backend.release.set()
