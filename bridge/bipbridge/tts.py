"""TTS client (Kyutai 1.6B, optional Kyutai Pocket TTS): FIFO scheduler per engine, WAV -> PCM16
conversion, small LRU cache.

Kyutai (``context/kyutai_server.py``) serializes synthesis behind a global lock and returns whole
files. The bridge mirrors that with its own queue so requests reach Kyutai strictly in arrival
order (FIFO), interactive requests (voice sentences, ``/v1/tts/sentence``) ahead of background ones
(outbox pre-synthesis), and a cancelled waiter simply leaves the queue.
"""
from __future__ import annotations

import array
import asyncio
import heapq
import itertools
import logging
import struct
import sys
from collections import OrderedDict
from contextlib import asynccontextmanager
from dataclasses import dataclass
from typing import AsyncIterator, List, Optional, Tuple

import httpx

from .logs import fields
from .textproc import normalize_for_speech

log = logging.getLogger("bipbridge.tts")

INTERACTIVE = 0
BACKGROUND = 1

FORMATS = {
    # client format -> (Kyutai response_format, content type)
    "pcm16": ("wav", "application/octet-stream"),
    "wav": ("wav", "audio/wav"),
    "opus": ("opus", "audio/ogg"),
    "mp3": ("mp3", "audio/mpeg"),
}


class TtsError(Exception):
    def __init__(self, message: str, status: int = 502):
        super().__init__(message)
        self.status = status


class FifoScheduler:
    """One holder at a time; waiters served by (priority, arrival order)."""

    def __init__(self) -> None:
        self._busy = False
        self._waiters: List[Tuple[int, int, "asyncio.Future[None]"]] = []
        self._counter = itertools.count()

    @property
    def depth(self) -> int:
        return sum(1 for _, _, f in self._waiters if not f.done()) + (1 if self._busy else 0)

    @asynccontextmanager
    async def slot(self, priority: int = INTERACTIVE) -> AsyncIterator[None]:
        if not self._busy and not any(not f.done() for _, _, f in self._waiters):
            self._busy = True
        else:
            fut: "asyncio.Future[None]" = asyncio.get_running_loop().create_future()
            heapq.heappush(self._waiters, (priority, next(self._counter), fut))
            try:
                await fut
            except asyncio.CancelledError:
                if fut.done() and not fut.cancelled():
                    self._release()  # slot was handed to us just before cancellation
                raise
        try:
            yield
        finally:
            self._release()

    def _release(self) -> None:
        while self._waiters:
            _, _, fut = heapq.heappop(self._waiters)
            if not fut.done():
                fut.set_result(None)  # hand over; stays busy
                return
        self._busy = False


def wav_to_pcm16(data: bytes) -> Tuple[bytes, int, int]:
    """Parse a RIFF/WAVE file (PCM 16-bit, PCM 24/32-bit or IEEE float 32) into little-endian
    int16 PCM. Returns ``(pcm, sample_rate, channels)``."""
    if len(data) < 12 or data[:4] != b"RIFF" or data[8:12] != b"WAVE":
        raise TtsError("Kyutai returned something that is not a WAV file")
    pos = 12
    fmt = None
    pcm = b""
    while pos + 8 <= len(data):
        cid = data[pos:pos + 4]
        size = struct.unpack("<I", data[pos + 4:pos + 8])[0]
        body_start = pos + 8
        body_end = min(len(data), body_start + size) if size != 0xFFFFFFFF else len(data)
        if cid == b"fmt ":
            tag, channels, rate, _, _, bits = struct.unpack("<HHIIHH", data[body_start:body_start + 16])
            if tag == 0xFFFE and size >= 26:  # WAVE_FORMAT_EXTENSIBLE: real tag in sub-format GUID
                tag = struct.unpack("<H", data[body_start + 24:body_start + 26])[0]
            fmt = (tag, channels, rate, bits)
        elif cid == b"data":
            pcm = data[body_start:body_end]
            break
        pos = body_start + size + (size & 1)
    if fmt is None:
        raise TtsError("WAV without fmt chunk")
    tag, channels, rate, bits = fmt
    if tag == 1 and bits == 16:
        out = pcm[: len(pcm) - (len(pcm) % 2)]
    elif tag == 3 and bits == 32:
        floats = array.array("f")
        floats.frombytes(pcm[: len(pcm) - (len(pcm) % 4)])
        if sys.byteorder == "big":  # pragma: no cover
            floats.byteswap()
        ints = array.array("h", (int(max(-1.0, min(1.0, x)) * 32767) for x in floats))
        out = ints.tobytes()
    elif tag == 1 and bits in (24, 32):
        step = bits // 8
        n = len(pcm) // step
        out = b"".join(pcm[i * step + step - 2:i * step + step] for i in range(n))
    else:
        raise TtsError(f"unsupported WAV encoding (tag={tag}, bits={bits})")
    if sys.byteorder == "big":  # pragma: no cover
        swapped = array.array("h", out)
        swapped.byteswap()
        out = swapped.tobytes()
    return out, rate, channels


@dataclass
class Audio:
    data: bytes
    content_type: str
    format: str
    sample_rate: int = 24000
    channels: int = 1
    cached: bool = False


POCKET_PREFIX = "pocket:"


@dataclass
class Engine:
    """One TTS server speaking the Kyutai API (``/v1/audio/speech``, ``/v1/audio/stream``), with its own queue."""
    name: str
    base_url: str
    scheduler: FifoScheduler


class TtsService:
    """Kyutai TTS 1.6B (GPU) plus, optionally, Kyutai Pocket TTS (CPU) for the Bips' voices.

    A voice named ``pocket:<name>`` (e.g. ``pocket:loutre``) goes to Pocket; any other voice to Kyutai. Each
    engine has its own FIFO queue (they run on different hardware). When Pocket is not configured, down or
    failing before its first audio, the request falls back to Kyutai's default voice (that audio is not cached).
    """

    def __init__(self, client: httpx.AsyncClient, base_url: str, default_voice: str,
                 cache_entries: int = 256, timeout: float = 120.0, max_chars: int = 1000,
                 sentence_max_chars: int = 180, pocket_url: Optional[str] = None):
        self.client = client
        self.base_url = base_url.rstrip("/")
        self.default_voice = default_voice
        self.cache_entries = max(0, cache_entries)
        self.timeout = timeout
        self.max_chars = max_chars
        self.sentence_max_chars = sentence_max_chars
        self.scheduler = FifoScheduler()
        self.kyutai = Engine("kyutai", self.base_url, self.scheduler)
        self.pocket = Engine("pocket", pocket_url.rstrip("/"), FifoScheduler()) if pocket_url else None
        self._cache: "OrderedDict[Tuple[str, str, str], Audio]" = OrderedDict()
        self.calls = 0  # engine requests actually made (diagnostics / tests)

    def _route(self, voice: str) -> Tuple[Engine, str]:
        if voice.startswith(POCKET_PREFIX):
            if self.pocket is not None:
                return self.pocket, voice[len(POCKET_PREFIX):]
            log.warning("pocket voice requested but pocket is not configured", extra=fields(voice=voice))
            return self.kyutai, self.default_voice
        return self.kyutai, voice

    def _cache_get(self, key: Tuple[str, str, str]) -> Optional[Audio]:
        audio = self._cache.get(key)
        if audio is not None:
            self._cache.move_to_end(key)
        return audio

    def _cache_put(self, key: Tuple[str, str, str], audio: Audio) -> None:
        if self.cache_entries <= 0:
            return
        self._cache[key] = audio
        self._cache.move_to_end(key)
        while len(self._cache) > self.cache_entries:
            self._cache.popitem(last=False)

    def _prepare(self, text: str, normalized: bool, max_chars: Optional[int]) -> str:
        clean = text.strip() if normalized else normalize_for_speech(text, self.sentence_max_chars)
        if not clean:
            raise TtsError("empty text after normalization", status=400)
        limit = max_chars if max_chars is not None else self.max_chars
        if limit and len(clean) > limit:
            raise TtsError(f"text too long ({len(clean)} > {limit} chars)", status=413)
        return clean

    async def synthesize(self, text: str, voice: Optional[str] = None, fmt: str = "pcm16",
                         priority: int = INTERACTIVE, normalized: bool = False,
                         max_chars: Optional[int] = None) -> Audio:
        if fmt not in FORMATS:
            raise TtsError(f"unsupported format: {fmt}", status=400)
        clean = self._prepare(text, normalized, max_chars)
        voice = voice or self.default_voice
        key = (voice, fmt, clean)
        hit = self._cache_get(key)
        if hit is not None:
            return Audio(hit.data, hit.content_type, hit.format, hit.sample_rate, hit.channels, cached=True)
        engine, engine_voice = self._route(voice)
        try:
            async with engine.scheduler.slot(priority):
                hit = self._cache_get(key)  # an identical request may have just finished
                if hit is not None:
                    return Audio(hit.data, hit.content_type, hit.format, hit.sample_rate, hit.channels, cached=True)
                audio = await self._call_engine(engine, clean, engine_voice, fmt)
        except TtsError as exc:
            if engine is self.kyutai or exc.status < 500:
                raise
            log.warning("pocket failed, falling back to kyutai", extra=fields(error=str(exc)))
            async with self.kyutai.scheduler.slot(priority):
                return await self._call_engine(self.kyutai, clean, self.default_voice, fmt)
        self._cache_put(key, audio)
        return audio

    async def stream(self, text: str, voice: Optional[str] = None, priority: int = INTERACTIVE,
                     max_chars: Optional[int] = None) -> AsyncIterator[bytes]:
        """PCM16 mono 24 kHz chunks as the engine produces them (``POST /v1/audio/stream``): the first
        audio arrives after ~0.75 s with Kyutai, ~0.1 s with Pocket, instead of after the whole text.
        Validation errors and an engine failure before the first chunk raise ``TtsError``; an engine
        without the streaming route (404/405) falls back to one whole-file synthesis. Only a complete
        stream is cached."""
        clean = self._prepare(text, False, max_chars)
        voice = voice or self.default_voice
        key = (voice, "pcm16", clean)
        hit = self._cache_get(key)
        if hit is not None:
            yield hit.data
            return
        engine, engine_voice = self._route(voice)
        received = bytearray()
        try:
            async for chunk in self._stream_engine(engine, clean, engine_voice, priority):
                received.extend(chunk)
                yield chunk
        except TtsError as exc:
            if received:
                return  # cut mid-stream: the client keeps what it already played, nothing is cached
            if engine is self.kyutai or exc.status < 500:
                raise
            log.warning("pocket stream failed, falling back to kyutai", extra=fields(error=str(exc)))
            async for chunk in self._stream_engine(self.kyutai, clean, self.default_voice, priority):
                yield chunk
            return
        if received:
            self._cache_put(key, Audio(bytes(received), FORMATS["pcm16"][1], "pcm16"))

    async def _stream_engine(self, engine: Engine, text: str, voice: str, priority: int) -> AsyncIterator[bytes]:
        async with engine.scheduler.slot(priority):
            self.calls += 1
            body = {"input": text, "voice": voice, "format": "pcm16"}
            sent = 0
            whole_file = False
            try:
                async with self.client.stream("POST", f"{engine.base_url}/v1/audio/stream", json=body,
                                              timeout=self.timeout) as resp:
                    if resp.status_code in (404, 405):
                        whole_file = True
                    elif resp.status_code != 200:
                        log.warning("tts stream error", extra=fields(engine=engine.name, status=resp.status_code))
                        raise TtsError(f"{engine.name} returned HTTP {resp.status_code}")
                    else:
                        async for chunk in resp.aiter_bytes():
                            if chunk:
                                sent += len(chunk)
                                yield chunk
            except httpx.HTTPError as exc:
                log.warning("tts stream failed", extra=fields(engine=engine.name, error=type(exc).__name__, sent=sent))
                raise TtsError(f"{engine.name} unreachable" if not sent else f"{engine.name} stream cut") from exc
            if whole_file:
                self.calls -= 1  # counted again by the whole-file call
                audio = await self._call_engine(engine, text, voice, "pcm16")
                yield audio.data

    async def _call_engine(self, engine: Engine, text: str, voice: str, fmt: str) -> Audio:
        engine_fmt, content_type = FORMATS[fmt]
        self.calls += 1
        try:
            resp = await self.client.post(
                f"{engine.base_url}/v1/audio/speech",
                json={"model": f"{engine.name}-tts", "input": text, "voice": voice, "response_format": engine_fmt},
                timeout=self.timeout,
            )
        except httpx.HTTPError as exc:
            log.warning("tts request failed", extra=fields(engine=engine.name, error=type(exc).__name__))
            raise TtsError(f"{engine.name} unreachable") from exc
        if resp.status_code != 200:
            log.warning("tts error", extra=fields(engine=engine.name, status=resp.status_code))
            raise TtsError(f"{engine.name} returned HTTP {resp.status_code}")
        body = resp.content
        if fmt == "pcm16":
            pcm, rate, channels = wav_to_pcm16(body)
            return Audio(pcm, content_type, fmt, rate, channels)
        return Audio(body, content_type, fmt)

    async def reachable(self, engine: Optional[Engine] = None) -> bool:
        engine = engine or self.kyutai
        try:
            resp = await self.client.get(f"{engine.base_url}/health", timeout=2.0)
            return resp.status_code == 200
        except httpx.HTTPError:
            return False
