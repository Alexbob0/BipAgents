"""``WS /v1/voice``: follow a Hermes run and stream synthesized sentences (SPEC §A5, §C2).

Client -> server (text frames, JSON):
  {"type":"follow","agent":"wellness","run_id":"run_…","voice":"5476","format":"pcm16","from_seq":0}
  {"type":"cancel"}
  {"type":"ping"}
Server -> client:
  {"type":"following","run_id","agent","format","sample_rate","channels"}
  {"type":"sentence","seq":n,"text":"…","format","sample_rate","channels","bytes":len}  + 1 binary frame
  {"type":"done","run_id","reason":"completed|failed|cancelled|interrupted|error|stream_closed"}
  {"type":"error","code","message"[,"seq"]}
  {"type":"pong"}
"""
from __future__ import annotations

import asyncio
import json
import logging
from typing import Any, Dict, List, Optional, Tuple

from starlette.websockets import WebSocket, WebSocketDisconnect, WebSocketState

from .auth import websocket_authorized
from .hermes import DELTA_TYPES, FLUSH_TYPES, TERMINAL_TYPES
from .logs import fields
from .textproc import SentenceSplitter
from .tts import FORMATS, INTERACTIVE, Audio, TtsError

log = logging.getLogger("bipbridge.voice")

TERMINAL_REASONS = {
    "run.completed": "completed", "run.failed": "failed", "run.cancelled": "cancelled",
    "run.interrupted": "interrupted", "done": "completed", "error": "failed",
}


class VoiceSession:
    def __init__(self, websocket: WebSocket, services: Any):
        self.ws = websocket
        self.services = services
        self.config = services.config
        self._send_lock = asyncio.Lock()
        self._task: Optional["asyncio.Task[None]"] = None
        self._synth_tasks: List["asyncio.Task[Audio]"] = []

    async def send_json(self, payload: Dict[str, Any]) -> None:
        async with self._send_lock:
            if self.ws.application_state == WebSocketState.CONNECTED:
                await self.ws.send_text(json.dumps(payload, ensure_ascii=False))

    async def send_sentence(self, header: Dict[str, Any], audio: bytes) -> None:
        async with self._send_lock:  # keep header and its binary frame adjacent
            if self.ws.application_state == WebSocketState.CONNECTED:
                await self.ws.send_text(json.dumps(header, ensure_ascii=False))
                await self.ws.send_bytes(audio)

    async def handle(self, message: Dict[str, Any]) -> None:
        mtype = message.get("type")
        if mtype == "follow":
            await self.follow(message)
        elif mtype == "cancel":
            if await self.stop():
                await self.send_json({"type": "done", "run_id": message.get("run_id"), "reason": "cancelled"})
        elif mtype == "ping":
            await self.send_json({"type": "pong"})
        else:
            await self.send_json({"type": "error", "code": "unknown_message", "message": str(mtype)})

    async def follow(self, message: Dict[str, Any]) -> None:
        agent = self.config.agent(message.get("agent"))
        run_id = str(message.get("run_id") or "").strip()
        fmt = str(message.get("format") or "pcm16")
        if agent is None:
            await self.send_json({"type": "error", "code": "unknown_agent", "message": "agent inconnu"})
            return
        if not run_id or len(run_id) > 256 or "/" in run_id:
            await self.send_json({"type": "error", "code": "invalid_run_id", "message": "run_id invalide"})
            return
        if fmt not in FORMATS or fmt == "mp3":
            await self.send_json({"type": "error", "code": "invalid_format", "message": fmt})
            return
        try:
            from_seq = max(0, int(message.get("from_seq") or 0))
        except (TypeError, ValueError):
            from_seq = 0
        await self.stop()
        voice = str(message.get("voice") or agent.voice or self.config.default_voice)
        await self.send_json({"type": "following", "run_id": run_id, "agent": agent.name, "format": fmt,
                              "sample_rate": 24000, "channels": 1})
        log.info("voice follow", extra=fields(agent=agent.name, run_id=run_id, format=fmt))
        self._task = asyncio.create_task(self._run(agent, run_id, voice, fmt, from_seq))

    async def stop(self) -> bool:
        """Cancel the current follow (SSE subscription + queued synthesis). True if one was active."""
        task, self._task = self._task, None
        for synth in self._synth_tasks:
            synth.cancel()
        self._synth_tasks = []
        if task is None or task.done():
            return False
        task.cancel()
        try:
            await task
        except (asyncio.CancelledError, Exception):
            pass
        return True

    async def _run(self, agent: Any, run_id: str, voice: str, fmt: str, from_seq: int) -> None:
        hub = self.services.hub
        tts = self.services.tts
        listener = hub.subscribe(agent, run_id, kind="voice", watch=self.config.watch_followed_runs)
        splitter = SentenceSplitter(self.config.limits.sentence_max_chars)
        queue: "asyncio.Queue[Optional[Tuple[int, str, Optional[asyncio.Task[Audio]]]]]" = asyncio.Queue()
        state = {"seq": 0, "spoken": 0}

        def enqueue(sentences: List[str]) -> None:
            for sentence in sentences:
                seq = state["seq"]
                state["seq"] += 1
                state["spoken"] += 1
                if seq < from_seq:
                    continue  # already heard before a reconnection
                # Start synthesis now: it queues in the global FIFO, so Kyutai stays busy while
                # earlier sentences are being sent.
                task = asyncio.create_task(tts.synthesize(sentence, voice, fmt, INTERACTIVE, normalized=True,
                                                          max_chars=0))
                self._synth_tasks.append(task)
                queue.put_nowait((seq, sentence, task))

        consumer = asyncio.create_task(self._consume(queue, fmt))
        reason = "stream_closed"
        try:
            async for event in listener:
                if event.type in DELTA_TYPES:
                    enqueue(splitter.feed(event.text))
                elif event.type in FLUSH_TYPES:
                    enqueue(splitter.flush())
                elif event.type == "bridge.error":
                    code = str(event.data.get("code") or "hermes_error")
                    await self.send_json({"type": "error", "code": code, "message": "flux Hermes indisponible"})
                    reason = "error"
                    break
                elif event.type in TERMINAL_TYPES:
                    reason = TERMINAL_REASONS.get(event.type, "completed")
                    enqueue(splitter.flush())
                    if state["spoken"] == 0 and event.type == "run.completed" and event.final_text:
                        enqueue(splitter.feed(event.final_text) + splitter.flush())
                    break
            enqueue(splitter.flush())
            queue.put_nowait(None)
            await consumer
            await self.send_json({"type": "done", "run_id": run_id, "reason": reason})
            log.info("voice done", extra=fields(agent=agent.name, run_id=run_id, reason=reason,
                                                sentences=state["seq"]))
        except asyncio.CancelledError:
            consumer.cancel()
            raise
        except Exception:
            consumer.cancel()
            log.exception("voice follow failed", extra=fields(agent=agent.name, run_id=run_id))
            await self.send_json({"type": "error", "code": "internal", "message": "erreur du bridge"})
        finally:
            listener.close()

    async def _consume(self, queue: "asyncio.Queue[Any]", fmt: str) -> None:
        while True:
            item = await queue.get()
            if item is None:
                return
            seq, sentence, task = item
            try:
                audio = await task
            except asyncio.CancelledError:
                raise
            except TtsError as exc:
                log.warning("sentence synthesis failed", extra=fields(seq=seq, error=str(exc)))
                await self.send_json({"type": "error", "code": "tts_failed", "seq": seq, "message": str(exc)})
                continue
            finally:
                if task in self._synth_tasks:
                    self._synth_tasks.remove(task)
            header = {"type": "sentence", "seq": seq, "text": sentence, "format": fmt,
                      "sample_rate": audio.sample_rate, "channels": audio.channels, "bytes": len(audio.data)}
            await self.send_sentence(header, audio.data)

    async def close(self) -> None:
        await self.stop()


async def voice_endpoint(websocket: WebSocket) -> None:
    if not websocket_authorized(websocket):
        await websocket.close(code=1008)
        return
    await websocket.accept()
    session = VoiceSession(websocket, websocket.app.state.services)
    try:
        while True:
            raw = await websocket.receive()
            if raw.get("type") == "websocket.disconnect":
                break
            text = raw.get("text")
            if text is None:
                await session.send_json({"type": "error", "code": "binary_not_supported", "message": "JSON attendu"})
                continue
            try:
                message = json.loads(text)
                if not isinstance(message, dict):
                    raise ValueError
            except ValueError:
                await session.send_json({"type": "error", "code": "invalid_json", "message": "JSON attendu"})
                continue
            await session.handle(message)
    except WebSocketDisconnect:
        pass
    finally:
        await session.close()
