"""Kerfy Studio's relay (relay/, the push relay's family rendezvous) as the black-box tests run it: the real binary on
127.0.0.1, its sealing key fixed so that it can be stopped and started again with its mailboxes still valid, and a
proxy in front of it playing a relay that misbehaves (it changes what passes, keeps the calls to send them again).

The binary: `RELAY_CMD`, else `bipagents-relay` in `$CARGO_TARGET_DIR` (or relay/target) release then debug, else
built once here (`cargo build --release --locked` in relay/)."""
from __future__ import annotations

import base64
import os
import shlex
import signal
import subprocess
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import httpx
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import Response, StreamingResponse
from starlette.routing import Route

from .servers import Served, free_port

RELAY_DIR = Path(__file__).resolve().parents[3] / "relay"


def relay_command() -> List[str]:
    raw = os.environ.get("RELAY_CMD", "").strip()
    if raw:
        return shlex.split(raw)
    target = os.environ.get("CARGO_TARGET_DIR", "").strip()
    folder = Path(target) if target else RELAY_DIR / "target"
    for profile in ("release", "debug"):
        built = folder / profile / "bipagents-relay"
        if built.is_file():
            return [str(built)]
    done = subprocess.run(["cargo", "build", "--release", "--locked"], cwd=str(RELAY_DIR), capture_output=True,
                          text=True, timeout=1800)
    built = folder / "release" / "bipagents-relay"
    if done.returncode != 0 or not built.is_file():
        raise RuntimeError(f"no relay to test: set RELAY_CMD or build relay/ ({done.stderr[-2000:]})")
    return [str(built)]


class KerfyRelay:
    """The relay process: `url`, `start()`, `stop()` (same port and key when started again)."""

    def __init__(self, root: Path):
        self.root = root
        self.port = free_port()
        self.key = base64.b64encode(os.urandom(32)).decode()
        self.log_path = root / "kerfy-relay.log"
        self.process: Optional[subprocess.Popen] = None
        self.command = relay_command()

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def start(self) -> "KerfyRelay":
        env = {"PATH": "/usr/bin:/bin", "RELAY_SEAL_KEYS": self.key, "RELAY_LISTEN": f"127.0.0.1:{self.port}",
               "RELAY_R1_UNTIL": "0", "RUST_LOG": "info"}
        log = open(self.log_path, "ab")
        self.process = subprocess.Popen(self.command, env=env, stdin=subprocess.DEVNULL, stdout=log,
                                        stderr=subprocess.STDOUT, start_new_session=True)
        log.close()
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(f"the relay exited: {self.log_tail()}")
            try:
                if httpx.get(f"{self.url}/health", timeout=1).status_code == 200:
                    return self
            except httpx.HTTPError:
                pass
            time.sleep(0.05)
        raise RuntimeError(f"the relay did not start: {self.log_tail()}")

    def stop(self) -> None:
        if self.process is None or self.process.poll() is not None:
            return
        self.process.send_signal(signal.SIGTERM)
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(self.process.pid, signal.SIGKILL)
            self.process.wait(timeout=5)

    def log_tail(self, lines: int = 40) -> str:
        try:
            return "\n".join(self.log_path.read_text(errors="replace").splitlines()[-lines:])
        except OSError:
            return ""

    def health(self) -> Dict[str, Any]:
        return httpx.get(f"{self.url}/health", timeout=5).json()


def flip(body: bytes, at: int) -> bytes:
    """One bit changed at `at` (or the middle when the body is shorter)."""
    if not body:
        return body
    at = at if at < len(body) else len(body) // 2
    return body[:at] + bytes([body[at] ^ 0x01]) + body[at + 1:]


class RelayProxy:
    """A relay that misbehaves, in front of the real one: what the bridges' calls carry kept (`calls`: path, headers,
    body), and on demand one bit changed in the next call a mailbox takes (`flip_next`: its token) or in the next
    answer it sends (`flip_answer`). The bridges are pointed at `url`."""

    def __init__(self, upstream: str):
        self.upstream = upstream
        self.calls: List[Tuple[str, Dict[str, str], bytes]] = []
        self.flip_next: Optional[str] = None
        self.flip_answer: Optional[str] = None
        self.served = Served(Starlette(routes=[Route("/{path:path}", self.handle,
                                                      methods=["GET", "POST", "PUT", "DELETE"])])).start()

    @property
    def url(self) -> str:
        return self.served.url

    def stop(self) -> None:
        self.served.stop()

    async def handle(self, request: Request) -> Response:
        path = request.url.path
        body = await request.body()
        headers = {k: v for k, v in request.headers.items() if k.lower() not in ("host", "content-length")}
        if path.startswith("/v1/family/call/"):
            self.calls.append((path, dict(headers), body))
        token = headers.get("authorization", "").removeprefix("Bearer ").strip()
        if path.startswith("/v1/family/answer/") and self.flip_answer and token == self.flip_answer:
            self.flip_answer = None
            body = flip(body, 12)
        client = httpx.AsyncClient(timeout=None)
        upstream = await client.send(client.build_request(request.method, self.upstream + path,
                                                          params=request.query_params, headers=headers,
                                                          content=body), stream=True)
        out_headers = {k: v for k, v in upstream.headers.items()
                       if k.lower() not in ("content-length", "transfer-encoding", "connection")}
        if path == "/v1/family/next" and upstream.status_code == 200 and self.flip_next and token == self.flip_next:
            self.flip_next = None
            taken = await upstream.aread()
            await upstream.aclose()
            await client.aclose()
            return Response(flip(taken, 40), status_code=200, headers=out_headers)

        async def stream():
            try:
                async for chunk in upstream.aiter_raw():
                    yield chunk
            finally:
                await upstream.aclose()
                await client.aclose()

        return StreamingResponse(stream(), status_code=upstream.status_code, headers=out_headers)


def send_again(relay_url: str, path: str, headers: Dict[str, str], body: bytes,
               token: Optional[str] = None) -> httpx.Response:
    """A call kept by the proxy, sent to the relay again (with another mailbox's token when given), as a relay that
    keeps what passes could."""
    sent = {k: v for k, v in headers.items() if k.lower() in ("x-bip-kind", "content-type", "authorization")}
    if token is not None:
        sent["authorization"] = f"Bearer {token}"
    return httpx.post(relay_url + path, headers=sent, content=body, timeout=40)
