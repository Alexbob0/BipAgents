"""FastAPI application factory: routes, services and background tasks."""
from __future__ import annotations

import asyncio
import json
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

from . import __version__, pages
from .apns import ApnsClient
from .auth import require_bridge_key
from .config import Config
from .cronwatch import CronWatcher, Presence
from .live_brief import Briefs, live_router
from .media import content_type as media_content_type, resolve as resolve_media
from .files import UploadSizeLimit, handle_upload, purge_uploads
from .hermes import HermesClient, HermesHTTPError
from .lan import hermes_proxy, lan_info
from .relay import RelayClient, relay_router
from .agents_admin import agents_admin_router, mount_all_uploads, run_hermes
from .agent_tools import AudioMaker, agent_tools_router, media_roots, purge_audio, register_all as register_agent_tools
from .place_backup import place_backup_router
from .place_import import place_import_routers
from .sleep import Sleep
from .ssh_keys import ssh_router
from .vault import Vault, vault_router
from .cloud_setup import cloud_setup_router
from .console import ConsoleStore, console_admin_router, console_files, console_router
from .guard import Guard
from .voices import relay_for, voice_for, voices_router
from .favicons import favicons_router
from .households import Households, path_for as households_path
from .peers import Links, family_router, household_admin_router, peer_router
from .desktop import Tickets, desktop_admin_router, desktop_router, static_files
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
                              sentence_max_chars=config.limits.sentence_max_chars, pocket_url=config.pocket_url,
                              voice_server=config.voice_server, words=config.pronunciations)
        self.hermes = HermesClient(self.http)
        self.briefs = Briefs(os.path.join(os.path.dirname(config.outbox.db_path), "briefs"), self.hermes, config.agents)
        self.sleep = Sleep(config, routines=self._routines, hermes_up=self._hermes_up)
        self.audio_maker = AudioMaker(config, self.tts)
        apns = None
        if config.apns.enabled:
            if os.path.exists(config.apns.p8_path):
                apns = ApnsClient(config.apns, self.apns_http)
            else:
                log.warning("APNs key file missing", extra=fields(path=config.apns.p8_path))
        self.apns = apns
        # Without its own APNs key, an install (another person's bridge) sends through the administrator's relay.
        sender: Any = apns
        if sender is None and config.push_relay_url and config.push_relay_key:
            sender = RelayClient(config.push_relay_url, config.push_relay_key, self.http)
        self.push_mode = "apns" if apns is not None else ("relay" if sender is not None else None)
        # Android: FCM with the Firebase project's key, else through the same relay as Apple's pushes.
        fcm: Any = None
        if config.fcm_service_account:
            if os.path.exists(config.fcm_service_account):
                from .fcm import FcmClient
                fcm = FcmClient(config.fcm_service_account, self.http)
            else:
                log.warning("FCM service account missing", extra=fields(path=config.fcm_service_account))
        self.fcm = fcm
        android = fcm or (RelayClient(config.push_relay_url, config.push_relay_key, self.http)
                          if config.push_relay_url and config.push_relay_key else None)
        # The publisher's relay, for the apps that registered there (any install without an Apple key of its own).
        self.push_service = RelayClient(config.push_service_url, "", self.http) if config.push_service_url else None
        self.push = PushService(config, self.store, sender, android, self.push_service)
        self.hub = RunHub(self.hermes, self.push, watch_max_seconds=config.limits.watch_max_seconds, store=self.store)
        self.outbox = OutboxService(config, self.store, self.tts, self.push)
        self.presence = Presence()
        self.vault = Vault(os.path.join(os.path.dirname(config.outbox.db_path), "vault"))
        self.links = Links(os.path.join(os.path.dirname(config.outbox.db_path), "peers.json"))
        self.households = Households(households_path(config))
        self.desktop_tickets = Tickets()
        self.desktop_control: Dict[str, bool] = {}   # desktops set up by hand: the person took over (input passes)
        self.subscribers: List[NtfySubscriber] = []
        self._tasks: List["asyncio.Task[Any]"] = []

    def start_background(self) -> None:
        for agent in self.config.agents.values():
            if agent.ntfy_topic:
                sub = NtfySubscriber(self.http, self.config.ntfy_url, agent, self.store, self.outbox)
                self.subscribers.append(sub)
                self._tasks.append(asyncio.create_task(sub.run()))
        if self.config.cron_watch:
            watcher = CronWatcher(self.config, self.hermes, self.outbox, self.config.cron_interval_seconds,
                                  self.config.cron_window_hours, store=self.store, hub=self.hub, push=self.push,
                                  presence=self.presence)
            self._tasks.append(asyncio.create_task(watcher.run()))
        self._tasks.append(asyncio.create_task(self._purge_loop()))
        self._tasks.append(asyncio.create_task(self.briefs.run()))
        if self.sleep.enabled:
            self._tasks.append(asyncio.create_task(self.sleep.run_forever()))
        self._tasks.append(asyncio.create_task(self.hub.resume(self.config.agents)))
        if self.config.hermes.enabled:
            self._tasks.append(asyncio.create_task(mount_all_uploads(self.config)))
            self._tasks.append(asyncio.create_task(register_agent_tools(self.config, run_hermes)))

    async def _routines(self) -> List[str]:
        """When each scheduled task last ran (sleep.py: a place with routines stays up)."""
        return [str(row["last_seen"]) for row in await self.store.cron_jobs() if row.get("last_seen")]

    async def _hermes_up(self) -> bool:
        agent = next(iter(self.config.agents.values()), None)
        if agent is None:
            return True
        try:
            await self.hermes.capabilities(agent)
            return True
        except Exception:
            return False

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
                removed = await asyncio.to_thread(purge_audio, self.config, self.config.outbox.retention_days)
                if removed:
                    log.info("agent audio purged", extra=fields(removed=removed))
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


class CronJobPrefs(BaseModel):
    notify: bool


class TtsRequest(BaseModel):
    text: str = Field(..., max_length=20000)
    voice: Optional[str] = Field(None, max_length=300)
    format: str = "pcm16"
    agent: Optional[str] = None
    # What it is for (`live`, `replies`, `reading`): with no `voice`, the agent's voice for that use.
    use: Optional[str] = Field(None, max_length=20)


class DeviceRequest(BaseModel):
    token: str
    environment: Optional[str] = None
    agent_ids: List[str] = Field(default_factory=list)


class WatchRequest(BaseModel):
    agent: str
    run_id: str
    session_id: Optional[str] = None


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


SESSION_ID = re.compile(r"^[A-Za-z0-9_.:-]{1,200}$")
SSE_KEEPALIVE_SECONDS = 10.0


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
        """This install's agents. With `url` (their Hermes on the tailnet) and `key`, it is all the app needs to add
        them from one QR code (the install's: the bridge's address and key), and to pick up agents created later."""
        config = _services(request).config
        from .agents_admin import unclaimed_profiles
        return {"bridge": {"url": config.public_url, "lan": lan_info(config)},
                "agents": [{"id": a.name, "name": a.display_name, "voice": a.voice or config.default_voice,
                            "server_voice": a.server_voice, "category": a.category, "color": a.color,
                            "uploads": bool(a.upload_dir_host), "inbox": bool(a.ntfy_topic),
                            "url": a.public_url,
                            # Never to a web console: it reaches Hermes through the bridge, which adds the key.
                            "key": a.hermes_key if a.public_url and not getattr(request.state, "console", False) else None}
                           for a in config.agents.values()],
                # Profiles created on the server that the app could connect (POST /v1/agents/claim).
                "unclaimed": unclaimed_profiles(config)}

    @router.post("/tts/sentence")
    async def tts_sentence(body: TtsRequest, request: Request) -> Response:
        services = _services(request)
        agent = services.config.agent(body.agent) if body.agent else None
        if body.agent and agent is None:
            raise HTTPException(status_code=404, detail="unknown agent")
        if body.format not in ("pcm16", "opus", "wav"):
            raise HTTPException(status_code=400, detail="format must be pcm16, opus or wav")
        voice = body.voice or voice_for(services.config, agent, body.use)
        try:
            audio = await services.tts.synthesize(body.text, voice, body.format, INTERACTIVE, relay=relay_for(agent))
        except TtsError as exc:
            raise HTTPException(status_code=exc.status, detail=str(exc)) from exc
        headers = {"X-Sample-Rate": str(audio.sample_rate), "X-Channels": str(audio.channels),
                   "X-Cache": "hit" if audio.cached else "miss", "Cache-Control": "no-store"}
        if audio.format == "pcm16":
            headers["X-Sample-Format"] = "s16le"
        return Response(content=audio.data, media_type=audio.content_type, headers=headers)

    @router.post("/tts/prepare")
    async def tts_prepare(body: TtsRequest, request: Request) -> Dict[str, Any]:
        """The text a Bip says, prepared as this bridge would for Pocket (numbers in words, respellings, pauses), for a
        phone that speaks with the Bips' voices itself (voices/onnx/). `local: false`: not a Bip's voice, the phone
        asks for the audio as before."""
        services = _services(request)
        agent = services.config.agent(body.agent) if body.agent else None
        if body.agent and agent is None:
            raise HTTPException(status_code=404, detail="unknown agent")
        voice = body.voice or voice_for(services.config, agent, body.use)
        try:
            prepared = services.tts.for_phone(body.text, voice, max_chars=MESSAGE_MAX_CHARS)
        except TtsError as exc:
            raise HTTPException(status_code=exc.status, detail=str(exc)) from exc
        if prepared is None:
            return {"local": False, "voice": voice}
        text, language, bip = prepared
        return {"local": True, "voice": voice, "text": text, "language": language, "bip": bip}

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
        voice = body.voice or voice_for(services.config, agent, body.use)
        try:
            audio = await services.tts.synthesize(body.text, voice, fmt, INTERACTIVE, max_chars=MESSAGE_MAX_CHARS,
                                                  relay=relay_for(agent))
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
        voice = body.voice or voice_for(services.config, agent, body.use)
        chunks = services.tts.stream(body.text, voice, INTERACTIVE, max_chars=MESSAGE_MAX_CHARS, relay=relay_for(agent))
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
        environment = body.environment or services.config.apns.environment
        if environment == "relay":   # a relay token from the publisher's relay: kept as it is (case-sensitive)
            from .relay import RELAY_TOKEN
            if services.push_service is None:
                raise HTTPException(status_code=409, detail="this bridge has no push relay set up")
            if not RELAY_TOKEN.match(token):
                raise HTTPException(status_code=400, detail="invalid relay token")
        elif environment == "fcm":   # the Android app (Firebase Cloud Messaging); its tokens are case-sensitive
            from .fcm import FCM_TOKEN
            if not FCM_TOKEN.match(token):
                raise HTTPException(status_code=400, detail="invalid FCM token")
        else:
            if not DEVICE_TOKEN.match(token):
                raise HTTPException(status_code=400, detail="invalid device token (hex expected)")
            if environment not in ("sandbox", "production"):
                raise HTTPException(status_code=400, detail="environment must be sandbox, production or fcm")
            token = token.lower()
        agent_ids = [a.strip().lower() for a in body.agent_ids if a and a.strip()]
        unknown = [a for a in agent_ids if a not in services.config.agents]
        if unknown:
            raise HTTPException(status_code=400, detail={"error": "unknown agent_ids", "agent_ids": unknown})
        device = await services.store.upsert_device(token, environment, agent_ids)
        log.info("device registered", extra=fields(token=redact(token), env=environment, agents=agent_ids))
        return device

    @router.delete("/devices/{token}", status_code=204)
    async def delete_device(token: str, request: Request) -> Response:
        services = _services(request)
        token = token.strip()
        if await services.store.delete_device(token) or await services.store.delete_device(token.lower()):
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

    @router.get("/media")
    async def get_media(request: Request, path: str = Query(..., max_length=1024)) -> Response:
        """A file an agent pointed to with « MEDIA:<path> » (only under `[media] roots`)."""
        host = resolve_media(path, media_roots(_services(request).config))
        if host is None:
            raise HTTPException(status_code=404, detail="not found")
        headers = {"X-Content-Type-Options": "nosniff"}
        if host.lower().endswith(".html"):
            # A page an agent made: never a document of this origin, even opened by hand; a new version each time.
            headers.update({"Content-Security-Policy": f"sandbox allow-scripts; {pages.POLICY}", "Cache-Control": "no-store"})
        return FileResponse(host, media_type=media_content_type(host), filename=os.path.basename(host), headers=headers)

    @router.get("/pages")
    async def list_pages(request: Request, agent: str = Query(...)) -> Dict[str, Any]:
        """The pages an agent made (each one's `path` is for `/v1/media`), the last changed first."""
        config = _services(request).config
        agent_cfg = _agent_or_404(config, agent)
        found = await asyncio.to_thread(pages.list_pages, config, agent_cfg.name)
        return {"pages": [{k: p.get(k) for k in ("page", "path", "title", "created", "updated", "version", "bytes")}
                          for p in found]}

    @router.delete("/pages/{page}")
    async def delete_page(page: str, request: Request, agent: str = Query(...)) -> Dict[str, Any]:
        config = _services(request).config
        agent_cfg = _agent_or_404(config, agent)
        if not await asyncio.to_thread(pages.delete, config, agent_cfg.name, page):
            raise HTTPException(status_code=404, detail="not found")
        log.info("agent page deleted", extra=fields(agent=agent_cfg.name, page=page))
        return {"deleted": page}

    @router.get("/sessions/{session_id}/state")
    async def session_state(session_id: str, request: Request, agent: str = Query(...)) -> Dict[str, Any]:
        """Polled by an open conversation: its message count (to show turns the agent takes on its own),
        and « the user is looking at it » (no push for what is on screen)."""
        services = _services(request)
        agent_cfg = _agent_or_404(services.config, agent)
        if not SESSION_ID.match(session_id):
            raise HTTPException(status_code=400, detail="invalid session_id")
        services.presence.touch(agent_cfg.name, session_id)
        try:
            session = await services.hermes.session(agent_cfg, session_id)
        except HermesHTTPError as exc:
            raise HTTPException(status_code=exc.status if exc.status in (401, 403, 404) else 502, detail="Hermes refused") from exc
        except httpx.HTTPError as exc:
            raise HTTPException(status_code=502, detail="Hermes unreachable") from exc
        count = session.get("message_count", session.get("messages"))
        return {"session_id": session_id, "message_count": count if isinstance(count, int) else None,
                "updated_at": session.get("updated_at") or session.get("last_active")}

    @router.get("/status")
    async def status(request: Request) -> Dict[str, Any]:
        """Is each agent's Hermes up, and does the model answer? Lets the app tell « server down » from « model
        down » (its machine off, an expired API key)."""
        services = _services(request)
        config = services.config

        async def hermes_ok(agent) -> Dict[str, Any]:
            try:
                await asyncio.wait_for(services.hermes.capabilities(agent), timeout=5)
                return {"ok": True}
            except Exception as exc:  # unreachable, refused, timeout
                return {"ok": False, "error": type(exc).__name__}

        async def model_ok() -> Optional[Dict[str, Any]]:
            if not config.model_health_url:
                return None
            headers = {"Authorization": f"Bearer {config.model_health_key}"} if config.model_health_key else {}
            try:
                resp = await services.http.get(config.model_health_url, headers=headers, timeout=5.0)
                return {"ok": resp.status_code < 400, "status": resp.status_code}
            except httpx.HTTPError as exc:
                return {"ok": False, "error": type(exc).__name__}

        names = list(config.agents)
        results = await asyncio.gather(*(hermes_ok(config.agents[n]) for n in names), model_ok())
        return {"agents": dict(zip(names, results[:-1])), "model": results[-1], "protected": config.hermes.isolated}

    @router.get("/pairing")
    async def pairing(request: Request) -> Dict[str, Any]:
        """The LAN door as it is now (address, certificate): the app refreshes it while the tailnet works."""
        return {"lan": lan_info(_services(request).config)}

    @router.get("/replies/{reply_id}")
    async def get_reply(reply_id: str, request: Request) -> Dict[str, Any]:
        """The text of a reply / question push, for the Notification Service Extension (kept 24 h)."""
        text = _services(request).push.text(reply_id)
        if text is None:
            raise HTTPException(status_code=404, detail="not found")
        return {"id": reply_id, "text": text}

    @router.get("/cron-jobs")
    async def cron_jobs(request: Request, agent: Optional[str] = None) -> Dict[str, Any]:
        """Scheduled tasks seen by the cron watcher, with whether their replies are pushed."""
        services = _services(request)
        name = _agent_or_404(services.config, agent).name if agent else None
        return {"jobs": await services.store.cron_jobs(name)}

    @router.put("/cron-jobs/{agent}/{job}")
    async def set_cron_job(agent: str, job: str, body: CronJobPrefs, request: Request) -> Dict[str, Any]:
        services = _services(request)
        name = _agent_or_404(services.config, agent).name
        if not await services.store.set_cron_notify(name, job, body.notify):
            raise HTTPException(status_code=404, detail="unknown job")
        return {"agent": name, "job": job, "notify": body.notify}

    @router.post("/watch")
    async def watch_run(body: WatchRequest, request: Request) -> Dict[str, Any]:
        services = _services(request)
        agent = _agent_or_404(services.config, body.agent)
        run_id = _check_run_id(body.run_id)
        session_id = body.session_id if body.session_id and SESSION_ID.match(body.session_id) else None
        sub = services.hub.watch(agent, run_id, session_id)
        return {"watching": True, "agent": agent.name, "run_id": run_id, "followers": sub.followers,
                "finished": sub.finished}

    @router.get("/runs/{run_id}/events")
    async def run_events(run_id: str, request: Request, agent: str = Query(...),
                         session_id: Optional[str] = Query(None, max_length=200)) -> StreamingResponse:
        """The app follows a run through the bridge (SSE): the bridge holds the single Hermes subscription,
        replays the run from its start (re-attaching after a disconnect is exact), and knows whether
        someone is watching, so approvals and « reply ready » are pushed only when nobody is."""
        services = _services(request)
        agent_cfg = _agent_or_404(services.config, agent)
        run_id = _check_run_id(run_id)
        if session_id is not None and not SESSION_ID.match(session_id):
            raise HTTPException(status_code=400, detail="invalid session_id")
        listener = services.hub.subscribe(agent_cfg, run_id, kind="app", watch=True)
        if session_id:
            listener.sub.session_id = session_id

        async def events() -> AsyncIterator[bytes]:
            try:
                while True:
                    try:
                        event = await asyncio.wait_for(listener.queue.get(), timeout=SSE_KEEPALIVE_SECONDS)
                    except asyncio.TimeoutError:
                        if await request.is_disconnected():
                            return
                        yield b": keepalive\n\n"
                        continue
                    if event is None:
                        return
                    data = (event.raw or json.dumps(event.data, ensure_ascii=False)).replace("\n", "\ndata: ")
                    yield f"event: {event.type}\ndata: {data}\n\n".encode("utf-8")
            finally:
                listener.close()

        return StreamingResponse(events(), media_type="text/event-stream",
                                 headers={"Cache-Control": "no-store", "X-Accel-Buffering": "no"})

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
        from .agents_admin import heal_souls, heal_thinking
        heal_thinking(config)
        heal_souls(config)
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
    app.state.guard = Guard()   # wrong keys counted by address (guard.py)
    app.state.console = ConsoleStore(os.path.join(os.path.dirname(config.outbox.db_path), "console.json"))
    app.add_middleware(UploadSizeLimit, limit_getter=lambda: config.limits.upload_max_bytes)

    @app.get("/health")
    async def health(request: Request) -> Dict[str, Any]:
        services = _services(request)
        pocket, server = services.tts.pocket, services.tts.server
        kyutai_ok, ntfy_ok, pocket_ok, server_ok = await asyncio.gather(
            services.tts.reachable(), ntfy_reachable(services.http, config.ntfy_url),
            services.tts.reachable(pocket) if pocket else asyncio.sleep(0, result=None),
            services.tts.reachable(server) if server else asyncio.sleep(0, result=None))
        # « voice_server »: the person's own voice model (null: none; false: set up, down for now). « drama »: the same
        # for the apps that offered Drama 3's voices by name, only when that server is Drama 3.
        drama = config.voice_server is not None and config.voice_server.model.startswith("drama")
        return {"ok": True, "version": __version__, "kyutai": kyutai_ok, "pocket": pocket_ok,
                "voice_server": server_ok, "drama": server_ok if drama else None, "ntfy": ntfy_ok,
                "apns": services.apns is not None, "push": services.push_mode,
                # The app registers with the publisher's relay and sends the relay token (environment « relay »).
                "push_relay": services.push_service is not None, "fcm": services.fcm is not None,
                "tts_queue": services.tts.scheduler.depth}

    app.include_router(build_router())
    app.include_router(relay_router())
    app.include_router(agents_admin_router(), prefix="/v1", dependencies=[Depends(require_bridge_key)])
    app.include_router(vault_router(), prefix="/v1", dependencies=[Depends(require_bridge_key)])
    app.include_router(place_backup_router(), prefix="/v1", dependencies=[Depends(require_bridge_key)])
    import_admin, import_open = place_import_routers()
    app.include_router(import_admin, prefix="/v1", dependencies=[Depends(require_bridge_key)])
    app.include_router(import_open, prefix="/v1")   # a computer sending its agents: the app's one-time code instead
    app.include_router(ssh_router(), prefix="/v1", dependencies=[Depends(require_bridge_key)])
    app.include_router(voices_router(), prefix="/v1", dependencies=[Depends(require_bridge_key)])
    app.include_router(live_router(), prefix="/v1", dependencies=[Depends(require_bridge_key)])
    app.include_router(favicons_router(), prefix="/v1", dependencies=[Depends(require_bridge_key)])
    if not config.cloud:   # on the Internet (a server the app created), no web console: one door less
        app.include_router(console_admin_router(), prefix="/v1", dependencies=[Depends(require_bridge_key)])
        app.include_router(console_router())   # the web console's page and login (its session then stands for the key)
        app.mount("/console/static", console_files(), name="console")
    # A server the app created, until the app claims it (its own proofs, no bridge key yet).
    app.include_router(cloud_setup_router(), prefix="/v1")
    # The agents' Hermes through the bridge: on the LAN door, and for agents in their own space (their gateway listens
    # on a local port only). Hermes checks each agent's own key.
    app.include_router(hermes_proxy())
    app.include_router(peer_router())   # agents' own tokens, checked against the links the person allowed
    app.include_router(agent_tools_router())   # the agents' MCP tools (create_audio), each agent with its own token
    app.include_router(family_router())   # the linked households' bridges, with the key each was given
    app.include_router(household_admin_router(), prefix="/v1", dependencies=[Depends(require_bridge_key)])
    app.include_router(desktop_admin_router(), prefix="/v1", dependencies=[Depends(require_bridge_key)])
    app.mount("/desktop/static/novnc", static_files(), name="novnc")
    app.include_router(desktop_router())   # the page and its stream: a short-lived ticket from the route above
    app.add_api_websocket_route("/v1/voice", voice_endpoint)
    return app
