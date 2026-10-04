"""FastAPI application factory: routes, services and background tasks."""
from __future__ import annotations

import asyncio
import logging
import os
import re
from datetime import datetime, timezone
from contextlib import asynccontextmanager
from typing import Any, AsyncIterator, Dict, List, Optional

import httpx
from fastapi import APIRouter, Depends, FastAPI, HTTPException, Query, Request
from fastapi.responses import FileResponse, JSONResponse, Response, StreamingResponse
from pydantic import BaseModel, Field

from . import __version__
from .apns import ApnsClient
from .auth import require_bridge_key
from .config import Config
from .files import UploadSizeLimit, handle_upload, purge_uploads
from .hermes import HermesClient
from .logs import fields, redact
from .ntfy import NtfySubscriber, OutboxService, ntfy_reachable
from .push import PushService
from .runs import RunHub
from .store import Store, iso, parse_since, utcnow
from .tts import INTERACTIVE, TtsError, TtsService
from .voice_ws import voice_endpoint

log = logging.getLogger("bipbridge.app")

RUN_ID = re.compile(r"^[A-Za-z0-9_.:\-]{1,256}$")
DEVICE_TOKEN = re.compile(r"^[0-9a-fA-F]{32,512}$")


class Services:
    """Everything that needs a running event loop (created in the lifespan)."""

    def __init__(self, config: Config, transport: Optional[httpx.AsyncBaseTransport] = None,
                 apns_transport: Optional[httpx.AsyncBaseTransport] = None):
        self.config = config
        self.http = httpx.AsyncClient(transport=transport, timeout=30.0,
                                      limits=httpx.Limits(max_connections=200, max_keepalive_connections=20))
        self.apns_http = httpx.AsyncClient(transport=apns_transport, http2=apns_transport is None, timeout=15.0)
        self.store = Store(config.outbox.db_path)
        self.tts = TtsService(self.http, config.kyutai_url, config.default_voice,
                              cache_entries=config.limits.tts_cache_entries, timeout=config.kyutai_timeout,
                              max_chars=config.limits.tts_max_chars,
                              sentence_max_chars=config.limits.sentence_max_chars)
        self.hermes = HermesClient(self.http)
        apns = None
        if config.apns.enabled:
            if os.path.exists(config.apns.p8_path):
                apns = ApnsClient(config.apns, self.apns_http)
            else:
                log.warning("APNs key file missing", extra=fields(path=config.apns.p8_path))
        self.apns = apns
        self.push = PushService(config, self.store, apns)
        self.hub = RunHub(self.hermes, self.push, watch_max_seconds=config.limits.watch_max_seconds)
        self.outbox = OutboxService(config, self.store, self.tts, self.push)
        self.subscribers: List[NtfySubscriber] = []
        self._tasks: List["asyncio.Task[Any]"] = []

    def start_background(self) -> None:
        for agent in self.config.agents.values():
            if agent.ntfy_topic:
                sub = NtfySubscriber(self.http, self.config.ntfy_url, agent, self.store, self.outbox)
                self.subscribers.append(sub)
                self._tasks.append(asyncio.create_task(sub.run()))
        self._tasks.append(asyncio.create_task(self._purge_loop()))

    async def _purge_loop(self) -> None:
        await asyncio.sleep(5)
        while True:
            try:
                for agent in self.config.agents.values():
                    if agent.upload_dir_host:
                        removed = await asyncio.to_thread(purge_uploads, agent.upload_dir_host,
                                                          self.config.limits.upload_retention_days)
                        if removed:
                            log.info("uploads purged", extra=fields(agent=agent.name, removed=removed))
                removed = await self.outbox.purge()
                if removed:
                    log.info("outbox purged", extra=fields(removed=removed))
            except Exception:
                log.exception("purge failed")
            await asyncio.sleep(self.config.limits.purge_interval_seconds)

    async def stop(self) -> None:
        for task in self._tasks:
            task.cancel()
        await asyncio.gather(*self._tasks, return_exceptions=True)
        await self.hub.close()
        await self.outbox.close()
        await self.http.aclose()
        await self.apns_http.aclose()
        self.store.close()


# -- request models (typing.Optional/List: pydantic evaluates annotations on Python 3.9) ---------

MESSAGE_MAX_CHARS = 8000


class TtsRequest(BaseModel):
    text: str = Field(..., max_length=20000)
    voice: Optional[str] = Field(None, max_length=300)
    format: str = "pcm16"
    agent: Optional[str] = None


class DeviceRequest(BaseModel):
    token: str
    environment: Optional[str] = None
    agent_ids: List[str] = Field(default_factory=list)


class WatchRequest(BaseModel):
    agent: str
    run_id: str


class ApproveRequest(BaseModel):
    agent: str
    run_id: str
    choice: str = Field(..., max_length=32)
    request_id: Optional[str] = Field(None, max_length=256)


def _services(request: Request) -> Services:
    return request.app.state.services


def _agent_or_404(config: Config, name: Optional[str]):
    agent = config.agent(name)
    if agent is None:
        raise HTTPException(status_code=404, detail="unknown agent")
    return agent


def _check_run_id(run_id: str) -> str:
    if not RUN_ID.match(run_id or ""):
        raise HTTPException(status_code=400, detail="invalid run_id")
    return run_id


def _outbox_item(row: Dict[str, Any], services: Services) -> Dict[str, Any]:
    has_audio = os.path.exists(services.outbox.audio_file(row["id"]))
    return {
        "id": row["id"], "agent": row["agent"], "title": row.get("title"), "text": row["text"],
        "created_at": row["created_at"], "sent_at": row.get("sent_at"), "session_id": row.get("session_id"),
        "has_audio": has_audio, "audio_url": f"/v1/outbox/{row['id']}/audio",
    }


def build_router() -> APIRouter:
    router = APIRouter(prefix="/v1", dependencies=[Depends(require_bridge_key)])

    @router.get("/agents")
    async def list_agents(request: Request) -> Dict[str, Any]:
        config = _services(request).config
        return {"agents": [{"id": a.name, "name": a.display_name, "voice": a.voice or config.default_voice,
                            "uploads": bool(a.upload_dir_host), "inbox": bool(a.ntfy_topic)}
                           for a in config.agents.values()]}

    @router.post("/tts/sentence")
    async def tts_sentence(body: TtsRequest, request: Request) -> Response:
        services = _services(request)
        agent = services.config.agent(body.agent) if body.agent else None
        if body.agent and agent is None:
            raise HTTPException(status_code=404, detail="unknown agent")
        if body.format not in ("pcm16", "opus", "wav"):
            raise HTTPException(status_code=400, detail="format must be pcm16, opus or wav")
        voice = body.voice or (agent.voice if agent else None) or services.config.default_voice
        try:
            audio = await services.tts.synthesize(body.text, voice, body.format, INTERACTIVE)
        except TtsError as exc:
            raise HTTPException(status_code=exc.status, detail=str(exc)) from exc
        headers = {"X-Sample-Rate": str(audio.sample_rate), "X-Channels": str(audio.channels),
                   "X-Cache": "hit" if audio.cached else "miss", "Cache-Control": "no-store"}
        if audio.format == "pcm16":
            headers["X-Sample-Format"] = "s16le"
        return Response(content=audio.data, media_type=audio.content_type, headers=headers)

    @router.post("/tts/message")
    async def tts_message(body: TtsRequest, request: Request) -> Response:
        """A whole reply as one audio file (voice-message mode). Kyutai gets the full text at once, so it
        batches the sentences on the GPU: far faster per second of audio than sentence by sentence."""
        services = _services(request)
        agent = services.config.agent(body.agent) if body.agent else None
        if body.agent and agent is None:
            raise HTTPException(status_code=404, detail="unknown agent")
        fmt = body.format if body.format != "pcm16" else "mp3"  # default to a compressed file here
        if fmt not in ("mp3", "opus", "wav"):
            raise HTTPException(status_code=400, detail="format must be mp3, opus or wav")
        voice = body.voice or (agent.voice if agent else None) or services.config.default_voice
        try:
            audio = await services.tts.synthesize(body.text, voice, fmt, INTERACTIVE, max_chars=MESSAGE_MAX_CHARS)
        except TtsError as exc:
            raise HTTPException(status_code=exc.status, detail=str(exc)) from exc
        return Response(content=audio.data, media_type=audio.content_type,
                        headers={"X-Cache": "hit" if audio.cached else "miss", "Cache-Control": "no-store"})

    @router.post("/tts/stream")
    async def tts_stream(body: TtsRequest, request: Request) -> Response:
        """Raw PCM16 (s16le, mono, 24 kHz) streamed as Kyutai produces it: first audio in ~0.75 s.
        Errors before the first chunk are plain HTTP errors; the body is chunked audio only."""
        services = _services(request)
        agent = services.config.agent(body.agent) if body.agent else None
        if body.agent and agent is None:
            raise HTTPException(status_code=404, detail="unknown agent")
        voice = body.voice or (agent.voice if agent else None) or services.config.default_voice
        chunks = services.tts.stream(body.text, voice, INTERACTIVE, max_chars=MESSAGE_MAX_CHARS)
        try:
            first = await chunks.__anext__()
        except StopAsyncIteration:
            first = b""
        except TtsError as exc:
            raise HTTPException(status_code=exc.status, detail=str(exc)) from exc

        async def relay() -> AsyncIterator[bytes]:
            try:
                if first:
                    yield first
                async for chunk in chunks:
                    yield chunk
            finally:
                await chunks.aclose()

        return StreamingResponse(relay(), media_type="application/octet-stream",
                                 headers={"X-Sample-Rate": "24000", "X-Channels": "1",
                                          "X-Sample-Format": "s16le", "Cache-Control": "no-store"})

    @router.post("/files")
    async def upload_file(request: Request) -> Dict[str, Any]:
        return await handle_upload(request)

    @router.post("/devices")
    async def register_device(body: DeviceRequest, request: Request) -> Dict[str, Any]:
        services = _services(request)
        token = body.token.strip().replace(" ", "").replace("<", "").replace(">", "")
        if not DEVICE_TOKEN.match(token):
            raise HTTPException(status_code=400, detail="invalid device token (hex expected)")
        environment = body.environment or services.config.apns.environment
        if environment not in ("sandbox", "production"):
            raise HTTPException(status_code=400, detail="environment must be sandbox or production")
        agent_ids = [a.strip().lower() for a in body.agent_ids if a and a.strip()]
        unknown = [a for a in agent_ids if a not in services.config.agents]
        if unknown:
            raise HTTPException(status_code=400, detail={"error": "unknown agent_ids", "agent_ids": unknown})
        device = await services.store.upsert_device(token.lower(), environment, agent_ids)
        log.info("device registered", extra=fields(token=redact(token), env=environment, agents=agent_ids))
        return device

    @router.delete("/devices/{token}", status_code=204)
    async def delete_device(token: str, request: Request) -> Response:
        services = _services(request)
        if await services.store.delete_device(token.strip().lower()):
            log.info("device removed", extra=fields(token=redact(token)))
        return Response(status_code=204)

    @router.get("/outbox")
    async def list_outbox(request: Request, since: Optional[str] = None, agent: Optional[str] = None,
                          limit: int = Query(100, ge=1, le=500)) -> Dict[str, Any]:
        services = _services(request)
        since_dt = None
        if since:
            try:
                since_dt = parse_since(since)
            except ValueError as exc:
                raise HTTPException(status_code=400, detail="invalid since (ISO 8601 expected)") from exc
        agent_name = _agent_or_404(services.config, agent).name if agent else None
        rows = await services.store.list_outbox(since_dt, agent_name, limit)
        return {"items": [_outbox_item(r, services) for r in rows], "server_time": iso(utcnow())}

    @router.get("/outbox/{item_id}")
    async def get_outbox(item_id: str, request: Request) -> Dict[str, Any]:
        services = _services(request)
        row = await services.store.get_outbox(item_id)
        if row is None:
            raise HTTPException(status_code=404, detail="not found")
        return _outbox_item(row, services)

    @router.get("/outbox/{item_id}/audio")
    async def get_outbox_audio(item_id: str, request: Request) -> Response:
        services = _services(request)
        row = await services.store.get_outbox(item_id)
        if row is None:
            raise HTTPException(status_code=404, detail="not found")
        path = services.outbox.audio_file(item_id)
        if not os.path.exists(path):
            task = services.outbox.ensure_audio(row)
            if task is None:
                raise HTTPException(status_code=404, detail="no audio for this message")
            try:
                await asyncio.wait_for(asyncio.shield(task), timeout=20.0)
            except asyncio.TimeoutError:
                return JSONResponse({"status": "pending"}, status_code=202, headers={"Retry-After": "3"})
            if not os.path.exists(path):
                raise HTTPException(status_code=503, detail="audio synthesis failed")
        return FileResponse(path, media_type="audio/mpeg", filename=f"{item_id}.mp3")

    @router.post("/watch")
    async def watch_run(body: WatchRequest, request: Request) -> Dict[str, Any]:
        services = _services(request)
        agent = _agent_or_404(services.config, body.agent)
        run_id = _check_run_id(body.run_id)
        sub = services.hub.watch(agent, run_id)
        return {"watching": True, "agent": agent.name, "run_id": run_id, "followers": sub.voice_listeners,
                "finished": sub.finished}

    @router.post("/approve")
    async def approve(body: ApproveRequest, request: Request) -> Response:
        services = _services(request)
        agent = _agent_or_404(services.config, body.agent)
        run_id = _check_run_id(body.run_id)
        try:
            status, payload = await services.hermes.approve(agent, run_id, body.choice.strip().lower(),
                                                            body.request_id)
        except httpx.HTTPError as exc:
            raise HTTPException(status_code=502, detail="Hermes unreachable") from exc
        if 200 <= status < 300:
            services.hub.forget_approval(agent.name, run_id, body.request_id)
        log.info("approval forwarded", extra=fields(agent=agent.name, run_id=run_id, status=status))
        return JSONResponse(payload, status_code=status)

    @router.get("/approvals")
    async def approvals(request: Request, agent: Optional[str] = None) -> Dict[str, Any]:
        services = _services(request)
        agent_name = _agent_or_404(services.config, agent).name if agent else None
        items = services.hub.pending_approvals(agent_name)
        for item in items:
            item["created_at"] = iso(datetime.fromtimestamp(item["created_at"], tz=timezone.utc))
        return {"items": items}

    return router


def create_app(config: Config, *, transport: Optional[httpx.AsyncBaseTransport] = None,
               apns_transport: Optional[httpx.AsyncBaseTransport] = None,
               start_background: bool = True) -> FastAPI:
    """``transport``/``apns_transport`` let tests replace Kyutai/Hermes/ntfy and APNs."""

    @asynccontextmanager
    async def lifespan(app: FastAPI) -> AsyncIterator[None]:
        services = Services(config, transport, apns_transport)
        app.state.services = services
        if start_background:
            services.start_background()
        log.info("bridge started", extra=fields(version=__version__, agents=sorted(config.agents),
                                                apns=services.apns is not None))
        try:
            yield
        finally:
            await services.stop()

    app = FastAPI(title="BipAgents bridge", version=__version__, lifespan=lifespan,
                  docs_url=None, redoc_url=None, openapi_url=None)
    app.state.config = config
    app.add_middleware(UploadSizeLimit, limit_getter=lambda: config.limits.upload_max_bytes)

    @app.get("/health")
    async def health(request: Request) -> Dict[str, Any]:
        services = _services(request)
        kyutai_ok, ntfy_ok = await asyncio.gather(services.tts.reachable(),
                                                  ntfy_reachable(services.http, config.ntfy_url))
        return {"ok": True, "version": __version__, "kyutai": kyutai_ok, "ntfy": ntfy_ok,
                "apns": services.apns is not None, "tts_queue": services.tts.scheduler.depth}

    app.include_router(build_router())
    app.add_api_websocket_route("/v1/voice", voice_endpoint)
    return app
