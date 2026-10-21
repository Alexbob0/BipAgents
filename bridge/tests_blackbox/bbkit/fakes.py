"""The fake servers the bridge talks to, each on its own port of 127.0.0.1: Hermes' api_server, Kyutai TTS, Pocket
TTS, the person's own voice server, ntfy, APNs, FCM (with Google's OAuth token endpoint), the publisher's push relay
and a model's /v1/models. They behave like the fakes of tests/conftest.py (same fake audio, same answers), record
every request (`.requests`, a list of `Seen`) and are driven from the tests by plain attributes.

Started once per test session; `reset()` before each test puts them back to their defaults. Streams they hold open
(SSE, ntfy) end when the test sets `release`, or when its own flag in a run's script is set."""
from __future__ import annotations

import array
import asyncio
import json
import math
import struct
import threading
import time
from typing import Any, Callable, Dict, List, Optional, Tuple

from starlette.requests import Request
from starlette.responses import JSONResponse, PlainTextResponse, Response, StreamingResponse

from .common import HERMES_KEY, make_wav, pcm_for
from .servers import Seen, Served, wait_flag

CLOSE = object()   # in a run's script: end the SSE stream here (instead of staying open like Hermes does)


class Fake:
    name = "fake"

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.served: Optional[Served] = None
        self.reset()

    def reset(self) -> None:
        self.requests: List[Seen] = []
        self.release = threading.Event()
        # A hook for one test: `override(seen) -> Optional[Response]`, tried before the fake's own routes.
        self.override: Optional[Callable[[Seen], Any]] = None

    @property
    def url(self) -> str:
        assert self.served is not None
        return self.served.url

    @property
    def port(self) -> int:
        assert self.served is not None
        return self.served.port

    def start(self) -> "Fake":
        self.served = Served(self).start()
        return self

    def stop(self) -> None:
        self.release.set()
        if self.served is not None:
            self.served.stop()

    def seen(self, path_prefix: str = "", method: Optional[str] = None) -> List[Seen]:
        return [r for r in list(self.requests) if r.path.startswith(path_prefix) and (method is None or r.method == method)]

    async def __call__(self, scope, receive, send) -> None:
        if scope["type"] != "http":
            return
        request = Request(scope, receive)
        body = await request.body()
        seen = Seen(scope["method"], scope["path"], scope.get("query_string", b"").decode("latin-1"),
                    {k.decode("latin-1").lower(): v.decode("latin-1") for k, v in scope["headers"]}, body)
        with self.lock:
            self.requests.append(seen)
        try:
            response = None
            if self.override is not None:
                response = self.override(seen)
                if asyncio.iscoroutine(response):
                    response = await response
            if response is None:
                response = await self.handle(seen)
        except Exception as exc:  # noqa: BLE001 - a fake's own bug shows as an odd status, not a hang
            response = PlainTextResponse(f"fake {self.name} error: {exc!r}", status_code=599)
        await response(scope, receive, send)

    async def handle(self, seen: Seen) -> Response:  # pragma: no cover - overridden
        return PlainTextResponse("no route", status_code=404)


# -- TTS ---------------------------------------------------------------------------------------------------------

class FakeKyutai(Fake):
    """Kyutai TTS: `/health`, `/v1/audio/speech` (wav: the fake PCM in a WAV; mp3: « ID3 » + the text) and
    `/v1/audio/stream` (the fake PCM in two chunks)."""
    name = "kyutai"

    def reset(self) -> None:
        super().reset()
        self.inputs: List[Dict[str, Any]] = []
        self.delay = 0.0
        self.active = 0
        self.max_active = 0
        self.status = 200
        self.streaming = True

    async def handle(self, seen: Seen) -> Response:
        if seen.path == "/health":
            return JSONResponse({"status": "ok"})
        body = seen.json()
        with self.lock:
            self.active += 1
            self.max_active = max(self.max_active, self.active)
            self.inputs.append(body)
        try:
            if self.delay:
                await asyncio.sleep(self.delay)
        finally:
            with self.lock:
                self.active -= 1
        if self.status != 200:
            return PlainTextResponse("boom", status_code=self.status)
        if seen.path == "/v1/audio/stream":
            if not self.streaming:
                return JSONResponse({"detail": "Not Found"}, status_code=404)
            pcm = pcm_for(body["input"])

            async def chunks():
                half = len(pcm) // 2 + 1   # odd split on purpose: chunks need not be sample-aligned
                yield pcm[:half]
                await asyncio.sleep(0.01)
                yield pcm[half:]

            return StreamingResponse(chunks(), media_type="application/octet-stream")
        if body.get("response_format") == "wav":
            return Response(make_wav(pcm_for(body["input"])), media_type="audio/wav")
        return Response(b"ID3" + body["input"].encode(), media_type="audio/mpeg")


class FakePocket(Fake):
    """Pocket TTS speaks the Kyutai API; its fake audio is the text reversed (tells engines apart)."""
    name = "pocket"

    def reset(self) -> None:
        super().reset()
        self.inputs: List[Dict[str, Any]] = []
        self.status = 200
        self.statuses: List[int] = []   # one status per synthesis request, then `status`
        self.voices = {"fr": ["colibri", "galet", "lumen", "mousse", "ours"]}

    async def handle(self, seen: Seen) -> Response:
        if seen.path == "/health":
            return JSONResponse({"status": "ok", "voices": self.voices}, status_code=200 if self.status == 200 else 503)
        body = seen.json()
        with self.lock:
            self.inputs.append(body)
            status = self.statuses.pop(0) if self.statuses else self.status
        if status != 200:
            return PlainTextResponse("pocket down", status_code=status)
        pcm = pcm_for(body["input"][::-1])
        if seen.path == "/v1/audio/stream":
            return Response(pcm, media_type="application/octet-stream")
        if body.get("response_format") == "wav":
            return Response(make_wav(pcm), media_type="audio/wav")
        return Response(b"ID3pocket" + body["input"].encode(), media_type="audio/mpeg")


def tone(seconds: float, rate: int = 44100) -> bytes:
    n = int(seconds * rate)
    return array.array("h", (int(8000 * math.sin(2 * math.pi * 440 * i / rate)) for i in range(n))).tobytes()


def wav_file(pcm: bytes, rate: int, channels: int = 1) -> bytes:
    return (b"RIFF" + struct.pack("<I", 36 + len(pcm)) + b"WAVEfmt " +
            struct.pack("<IHHIIHH", 16, 1, channels, rate, rate * channels * 2, channels * 2, 16) + b"data" +
            struct.pack("<I", len(pcm)) + pcm)


class FakeVoiceServer(Fake):
    """The person's own voice server (OpenAI's `POST /v1/audio/speech`): Drama 3 on audio.cpp streaming 44.1 kHz PCM,
    or a server giving whole WAV files. `mode` picks how it answers."""
    name = "voices"

    def reset(self) -> None:
        super().reset()
        self.inputs: List[Dict[str, Any]] = []
        # stream44k: 1 s of 44.1 kHz PCM in odd chunks; endless: 40 s for any text; wav44k / wav24k: whole files
        # (0.5 s); mp3: « ID3server »; busy: 5 s before a 24 kHz WAV; steady: 0.15 s before a 0.3 s WAV.
        self.mode = "stream44k"
        self.no_mp3 = True              # audio.cpp makes no mp3: 400
        self.voices_route = "audio"     # audio: /v1/audio/voices; kyutai: /v1/voices; none: neither
        self.voices = ["fr-naturelle-s15", "fr-mascotte-s17"]

    async def handle(self, seen: Seen) -> Response:
        if seen.path == "/v1/models":
            return JSONResponse({"data": [{"id": "voices"}]})
        if seen.path == "/v1/audio/voices":
            if self.voices_route == "audio":
                return JSONResponse({"voices": self.voices})
            return Response(status_code=404)
        if seen.path == "/v1/voices":
            if self.voices_route == "kyutai":
                return JSONResponse({"default": self.voices[0], "aliases": {v: f"{v}.wav" for v in self.voices}})
            return Response(status_code=404)
        if seen.path != "/v1/audio/speech":
            return Response(status_code=404)
        body = seen.json()
        with self.lock:
            self.inputs.append(body)
        if body.get("response_format") == "mp3":
            if self.mode == "mp3":
                return Response(b"ID3server", media_type="audio/mpeg")
            if self.no_mp3:
                return Response(status_code=400)
        if self.mode == "busy":
            await asyncio.sleep(5)
            return Response(wav_file(tone(0.5, 24000), 24000), media_type="audio/wav")
        if self.mode == "steady":
            await asyncio.sleep(0.15)
            return Response(wav_file(tone(0.3, 24000), 24000), media_type="audio/wav")
        if self.mode in ("wav44k", "wav24k"):
            rate = 44100 if self.mode == "wav44k" else 24000
            return Response(wav_file(tone(0.5, rate), rate), media_type="audio/wav")
        if self.mode == "endless":
            async def forever():
                for _ in range(40):
                    yield tone(1.0)
                    await asyncio.sleep(0)
            return StreamingResponse(forever(), media_type="application/octet-stream")
        audio = tone(1.0)

        async def chunks():
            for i in range(0, len(audio), 4411):   # odd sizes: chunks need not be sample-aligned
                yield audio[i:i + 4411]

        return StreamingResponse(chunks(), media_type="application/octet-stream")


# -- Hermes --------------------------------------------------------------------------------------------------------

def split_agent(path: str) -> Tuple[str, str]:
    """`/p/<agent>/rest` (a profile of the hub) -> (agent, /rest); anything else is the root agent ("")."""
    parts = path.split("/")
    if len(parts) >= 3 and parts[1] == "p" and parts[2]:
        return parts[2], "/" + "/".join(parts[3:])
    return "", path


class FakeHermes(Fake):
    """Hermes' api_server for several agents: the root one (`hermes_url` = this server) and the hub's profiles
    (`<url>/p/<name>`). Runs' event streams follow a script (SSE chunks, `threading.Event` gates, `CLOSE`) and stay
    open like Hermes' until `release`; runs started with `POST /v1/runs` run for `rounds` polls of their status (and
    count tokens in their session meanwhile), as the jobs' fake in tests/test_jobs.py."""
    name = "hermes"

    def reset(self) -> None:
        super().reset()
        self.keys: Dict[str, str] = {"": HERMES_KEY}    # agent -> its key (agents not listed: any key)
        self.runs: Dict[str, List[Any]] = {}
        self.run_status: Dict[str, int] = {}
        self.run_info: Dict[str, Dict[str, Any]] = {}   # GET /v1/runs/{id}, as Hermes keeps a run's status
        self.event_requests: List[Seen] = []
        self.approvals: List[Dict[str, Any]] = []
        self.approval_response: Tuple[int, Any] = (200, {"object": "hermes.run.approval_response", "resolved": 1})
        self.sessions: Dict[str, Dict[str, Dict[str, Any]]] = {}   # agent -> session id -> session
        self.messages: Dict[Tuple[str, str], List[Dict[str, Any]]] = {}
        self.created: List[Tuple[str, Dict[str, Any]]] = []
        self.chats: List[Dict[str, Any]] = []
        self.patched: List[Dict[str, Any]] = []
        self.deleted: List[str] = []
        self.chat_reply = "- Alex prépare un podcast pour jeudi.\n- Il dort mal depuis lundi."
        self.capabilities: Dict[str, int] = {}          # agent -> status (default 200)
        self.toolsets = [{"name": "web", "label": "Web", "description": "Search", "enabled": True,
                          "configured": True, "tools": ["web_search"]}]
        self.started: Dict[str, Dict[str, Any]] = {}    # runs made by POST /v1/runs
        self.rounds, self.step = 3, 40_000
        self.run_output = "Rapport : trois écoles comparées, page « Écoles »."
        self.run_usage = {"input_tokens": 100_000, "output_tokens": 5_000}
        self.forget_runs = False
        self.default_session = {"title": "Bot Chat", "message_count": 7}

    # what the tests set up
    def add_session(self, session_id: str, messages: List[Dict[str, Any]], title: str = "Discussion",
                    last_active: float = 0.0, agent: str = "", **extra: Any) -> None:
        self.sessions.setdefault(agent, {})[session_id] = {"id": session_id, "title": title,
                                                           "message_count": len(messages),
                                                           "last_active": last_active, **extra}
        self.messages[(agent, session_id)] = messages

    def _session_counters(self, agent: str, session_id: str) -> Dict[str, Any]:
        session = self.sessions.get(agent, {}).get(session_id)
        if session is None:
            return {}
        if any(r["status"] == "running" and r["agent"] == agent and r["session_id"] == session_id
               for r in self.started.values()):
            session["input_tokens"] = session.get("input_tokens", 0) + self.step
            session["output_tokens"] = session.get("output_tokens", 0) + self.step // 20
        return session

    async def handle(self, seen: Seen) -> Response:
        agent, path = split_agent(seen.path)
        parts = path.strip("/").split("/")
        if path in ("/health", "/v1/health"):   # Hermes' health needs no key
            return JSONResponse({"status": "ok"})
        expected = self.keys.get(agent)
        if expected is not None and seen.header("authorization") != f"Bearer {expected}":
            return JSONResponse({"error": "bad key"}, status_code=401)
        method = seen.method
        if path == "/v1/capabilities":
            status = self.capabilities.get(agent, 200)
            return JSONResponse({"runs": True} if status == 200 else {"error": "down"}, status_code=status)
        if path == "/v1/toolsets":
            return JSONResponse({"data": self.toolsets})
        if parts[:2] == ["v1", "runs"]:
            return await self._runs(seen, agent, parts, method)
        if parts[:2] == ["api", "sessions"]:
            return self._sessions(seen, agent, parts, method)
        return JSONResponse({"error": "no route"}, status_code=404)

    async def _runs(self, seen: Seen, agent: str, parts: List[str], method: str) -> Response:
        if len(parts) == 2 and method == "POST":
            body = seen.json() or {}
            with self.lock:
                run_id = f"r{len(self.started) + 1}"
                self.started[run_id] = {"status": "running", "polls": 0, "input": body.get("input"),
                                        "session_id": body.get("session_id"), "instructions": body.get("instructions"),
                                        "agent": agent}
            return JSONResponse({"run_id": run_id, "status": "started"}, status_code=202)
        run_id = parts[2] if len(parts) > 2 else ""
        if len(parts) == 4 and parts[3] == "events":
            self.event_requests.append(seen)
            if run_id not in self.runs:
                return JSONResponse({"error": "not found"}, status_code=self.run_status.get(run_id, 404))
            script, release = self.runs[run_id], self.release

            async def stream():
                for item in script:
                    if item is CLOSE:
                        return
                    if isinstance(item, threading.Event):
                        await wait_flag(item)
                        continue
                    if isinstance(item, (int, float)):
                        await asyncio.sleep(item)
                        continue
                    yield item.encode("utf-8")
                await wait_flag(release, 60)   # stays open like a real SSE stream until the test ends

            return StreamingResponse(stream(), media_type="text/event-stream")
        if len(parts) == 4 and parts[3] == "approval" and method == "POST":
            self.approvals.append({"agent": agent, "run_id": run_id, "body": seen.json()})
            status, payload = self.approval_response
            return JSONResponse(payload, status_code=status)
        if len(parts) == 4 and parts[3] == "stop" and method == "POST":
            run = self.started.get(run_id)
            if run is not None:
                run["status"] = "cancelled"
            return JSONResponse({"status": "stopping"})
        if len(parts) == 3 and method == "GET":
            if run_id in self.run_info:
                return JSONResponse(self.run_info[run_id])
            run = self.started.get(run_id)
            if run is None or self.forget_runs:
                return JSONResponse({"error": "not found"}, status_code=404)
            run["polls"] += 1
            if run["status"] == "running" and run["polls"] >= self.rounds:
                run["status"] = "completed"
                return JSONResponse({"status": "completed", "output": self.run_output, "usage": self.run_usage})
            return JSONResponse({"status": run["status"]})
        return JSONResponse({"error": "no route"}, status_code=404)

    def _sessions(self, seen: Seen, agent: str, parts: List[str], method: str) -> Response:
        mine = self.sessions.setdefault(agent, {})
        if len(parts) == 2 and method == "GET":
            listed = sorted(mine.values(), key=lambda s: -float(s.get("last_active") or 0))
            return JSONResponse({"sessions": listed})
        if len(parts) == 2 and method == "POST":
            body = seen.json() or {}
            with self.lock:
                self.created.append((agent, body))
                session_id = f"api_s{len(self.created)}"
            mine[session_id] = {"id": session_id, "title": body.get("title"), "message_count": 0,
                                "last_active": time.time(), "input_tokens": 0, "output_tokens": 0,
                                "cache_read_tokens": 0}
            self.messages[(agent, session_id)] = []
            return JSONResponse({"object": "hermes.session", "session": {"id": session_id, "title": body.get("title")}},
                                status_code=201)
        session_id = parts[2] if len(parts) > 2 else ""
        if len(parts) == 3 and method == "GET":
            if session_id in mine:
                return JSONResponse({"session": self._session_counters(agent, session_id)})
            return JSONResponse({"id": session_id, **self.default_session})
        if len(parts) == 3 and method == "PATCH":
            body = seen.json() or {}
            self.patched.append({"id": session_id, **body})
            if session_id in mine:
                mine[session_id].update(body)
            return JSONResponse({})
        if len(parts) == 3 and method == "DELETE":
            self.deleted.append(session_id)
            mine.pop(session_id, None)
            return JSONResponse({})
        if len(parts) == 4 and parts[3] == "messages":
            return JSONResponse({"messages": self.messages.get((agent, session_id), [])})
        if len(parts) == 4 and parts[3] == "chat" and method == "POST":
            self.chats.append({"agent": agent, "session": session_id, **(seen.json() or {})})
            return JSONResponse({"message": {"role": "assistant", "content": self.chat_reply}})
        return JSONResponse({"error": "no route"}, status_code=404)


# -- ntfy, model --------------------------------------------------------------------------------------------------

class FakeNtfy(Fake):
    """ntfy: `/v1/health`, and `GET /<topic>/json` streaming an « open » event, the lines queued for the topic, then
    staying open until the test ends."""
    name = "ntfy"

    def reset(self) -> None:
        super().reset()
        self.lines: Dict[str, List[str]] = {}
        self.subscriptions: List[Seen] = []

    async def handle(self, seen: Seen) -> Response:
        if seen.path == "/v1/health":
            return JSONResponse({"healthy": True})
        self.subscriptions.append(seen)
        topic = seen.path.strip("/").split("/")[0]
        with self.lock:
            lines = self.lines.pop(topic, [])
        release = self.release

        async def stream():
            yield (json.dumps({"id": "open1", "event": "open", "topic": topic}) + "\n").encode()
            for line in lines:
                yield (line + "\n").encode()
            await wait_flag(release, 60)

        return StreamingResponse(stream(), media_type="application/x-ndjson")


def facts_answer(body: Dict[str, Any]) -> List[Dict[str, Any]]:
    """The mailbox scan's model (tests/test_mail_scan.py's): one line of facts per « E-mail N » it was given, by sender."""
    text = body["messages"][1]["content"]
    lines: List[Dict[str, Any]] = []
    for n, chunk in enumerate(text.split("E-mail ")[1:], start=1):
        if "Sam" in chunk:
            lines.append({"n": n, "people": [{"name": "Sam", "email": "sam@example.com", "relation": "friend"}],
                          "topics": ["repas dimanche"], "facts": ["Repas chez Sam dimanche, apporter le dessert"],
                          "commitments": [{"what": "Apporter le dessert chez Sam", "due": "2099-10-25"}]})
        elif "Banque" in chunk:
            lines.append({"n": n, "orgs": [{"name": "Banque Exemple", "kind": "bank"}], "topics": ["banque"],
                          "facts": ["Rendez-vous à la banque le 30 octobre"],
                          "commitments": [{"what": "Rendez-vous banque", "due": "2099-10-30"}]})
        else:
            lines.append({"n": n, "skip": True})
    return lines


class FakeModel(Fake):
    """A model's API: `/v1/models` (for `[model] health_url`) and `/v1/chat/completions` (the mailbox scan)."""
    name = "model"

    def reset(self) -> None:
        super().reset()
        self.status = 200
        self.chats: List[Dict[str, Any]] = []

    async def handle(self, seen: Seen) -> Response:
        if seen.path.endswith("/chat/completions"):
            body = seen.json()
            self.chats.append({"body": body, "authorization": seen.header("authorization")})
            content = "\n".join(json.dumps(line, ensure_ascii=False) for line in facts_answer(body))
            return JSONResponse({"choices": [{"message": {"content": content}}],
                                 "usage": {"prompt_tokens": 1000, "completion_tokens": 100,
                                           "prompt_tokens_details": {"cached_tokens": 400}}},
                                status_code=self.status)
        return JSONResponse({"data": [{"id": "glm"}]}, status_code=self.status)


# -- push ----------------------------------------------------------------------------------------------------------

class FakeApns(Fake):
    """APNs on `/production/3/device/<token>` and `/sandbox/3/device/<token>` (the bridge's `[apns] base_url` and
    `sandbox_url` point there). Plain HTTP/1.1: real APNs is HTTP/2 over TLS, which both bridges negotiate by ALPN."""
    name = "apns"

    def reset(self) -> None:
        super().reset()
        self.responses: Dict[str, Tuple[int, Optional[str]]] = {}   # token -> (status, reason)
        self.script: List[Tuple[int, Optional[str]]] = []           # next answers, whatever the token

    def pushes(self) -> List[Seen]:
        return [r for r in list(self.requests) if "/3/device/" in r.path]

    def payloads(self) -> List[Dict[str, Any]]:
        return [r.json() for r in self.pushes()]

    @staticmethod
    def environment(seen: Seen) -> str:
        return seen.path.split("/")[1]

    @staticmethod
    def token(seen: Seen) -> str:
        return seen.path.rsplit("/", 1)[-1]

    async def handle(self, seen: Seen) -> Response:
        token = self.token(seen)
        with self.lock:
            status, reason = self.script.pop(0) if self.script else self.responses.get(token, (200, None))
        if status == 200:
            return Response(status_code=200, headers={"apns-id": "abc"})
        return JSONResponse({"reason": reason}, status_code=status)


class FakeFcm(Fake):
    """FCM HTTP v1 (`/v1/projects/<id>/messages:send`) and Google's OAuth token endpoint (`/token`)."""
    name = "fcm"

    def reset(self) -> None:
        super().reset()
        self.dead: set = set()

    def sends(self) -> List[Seen]:
        return [r for r in list(self.requests) if r.path.endswith("/messages:send")]

    async def handle(self, seen: Seen) -> Response:
        if seen.path == "/token":
            return JSONResponse({"access_token": "ya29.test", "expires_in": 3600, "token_type": "Bearer"})
        if seen.path.endswith("/messages:send"):
            if seen.header("authorization") != "Bearer ya29.test":
                return JSONResponse({"error": {"status": "UNAUTHENTICATED"}}, status_code=401)
            message = seen.json()["message"]
            if message["token"] in self.dead:
                return JSONResponse({"error": {"status": "NOT_FOUND", "details": [{"errorCode": "UNREGISTERED"}]}},
                                    status_code=404)
            return JSONResponse({"name": f"projects/x/messages/{len(self.sends())}"})
        return Response(status_code=404)


class FakePushService(Fake):
    """The publisher's push relay (relay/): `POST /v1/relay/push`; a token ending in « dead » is gone (410)."""
    name = "push-service"

    async def handle(self, seen: Seen) -> Response:
        if seen.path != "/v1/relay/push":
            return Response(status_code=404)
        body = seen.json()
        dead = str(body.get("token", "")).endswith("dead")
        return JSONResponse({"status": 410 if dead else 200, "reason": "Unregistered" if dead else None,
                             "apns_id": None if dead else "relay-1"})


class FakeAdminRelay(Fake):
    """Another person's bridge's relay (`[push] relay_url`): answers like `/v1/relay/push` of an admin bridge (APNs'
    410 for a token ending in « dead », a 403 for one ending in « blocked »)."""
    name = "admin-relay"

    async def handle(self, seen: Seen) -> Response:
        if seen.path != "/v1/relay/push":
            return Response(status_code=404)
        token = str(seen.json().get("token", ""))
        if token.endswith("blocked"):
            return JSONResponse({"detail": "this device was blocked for abuse"}, status_code=403)
        if token.endswith("dead"):
            return JSONResponse({"status": 410, "reason": "Unregistered", "apns_id": None})
        return JSONResponse({"status": 200, "reason": None, "apns_id": "relayed"})


class Fakes:
    """Every fake, started once."""

    def __init__(self) -> None:
        from .servers import Dropper
        self.hermes = FakeHermes()
        self.kyutai = FakeKyutai()
        self.pocket = FakePocket()
        self.voices = FakeVoiceServer()
        self.ntfy = FakeNtfy()
        self.model = FakeModel()
        self.apns = FakeApns()
        self.fcm = FakeFcm()
        self.push_service = FakePushService()
        self.admin_relay = FakeAdminRelay()
        self.all: List[Fake] = [self.hermes, self.kyutai, self.pocket, self.voices, self.ntfy, self.model, self.apns,
                                self.fcm, self.push_service, self.admin_relay]
        self.down = Dropper()
        from .imap import FakeImap
        self.imap = FakeImap()

    def start(self) -> "Fakes":
        for fake in self.all:
            fake.start()
        self.down.start()
        return self

    def reset(self) -> None:
        for fake in self.all:
            fake.reset()
        self.down.reset()
        self.imap.reset()

    def release(self) -> None:
        for fake in self.all:
            fake.release.set()

    def stop(self) -> None:
        for fake in self.all:
            fake.stop()
        self.down.stop()
        self.imap.stop()
