"""Kyutai TTS client: global FIFO scheduler, WAV -> PCM16 conversion, small LRU cache.

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


class TtsService:
    def __init__(self, client: httpx.AsyncClient, base_url: str, default_voice: str,
                 cache_entries: int = 256, timeout: float = 120.0, max_chars: int = 1000,
                 sentence_max_chars: int = 180):
        self.client = client
        self.base_url = base_url.rstrip("/")
        self.default_voice = default_voice
        self.cache_entries = max(0, cache_entries)
        self.timeout = timeout
        self.max_chars = max_chars
        self.sentence_max_chars = sentence_max_chars
        self.scheduler = FifoScheduler()
        self._cache: "OrderedDict[Tuple[str, str, str], Audio]" = OrderedDict()
        self.calls = 0  # Kyutai requests actually made (diagnostics / tests)

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

    async def synthesize(self, text: str, voice: Optional[str] = None, fmt: str = "pcm16",
                         priority: int = INTERACTIVE, normalized: bool = False,
                         max_chars: Optional[int] = None) -> Audio:
        if fmt not in FORMATS:
            raise TtsError(f"unsupported format: {fmt}", status=400)
        clean = text.strip() if normalized else normalize_for_speech(text, self.sentence_max_chars)
        if not clean:
            raise TtsError("empty text after normalization", status=400)
        limit = max_chars if max_chars is not None else self.max_chars
        if limit and len(clean) > limit:
            raise TtsError(f"text too long ({len(clean)} > {limit} chars)", status=413)
        voice = voice or self.default_voice
        key = (voice, fmt, clean)
        hit = self._cache_get(key)
        if hit is not None:
            return Audio(hit.data, hit.content_type, hit.format, hit.sample_rate, hit.channels, cached=True)
        async with self.scheduler.slot(priority):
            hit = self._cache_get(key)  # an identical request may have just finished
            if hit is not None:
                return Audio(hit.data, hit.content_type, hit.format, hit.sample_rate, hit.channels, cached=True)
            audio = await self._call_kyutai(clean, voice, fmt)
        self._cache_put(key, audio)
        return audio

    async def stream(self, text: str, voice: Optional[str] = None, priority: int = INTERACTIVE,
                     max_chars: Optional[int] = None) -> AsyncIterator[bytes]:
        """PCM16 mono 24 kHz chunks as Kyutai produces them (``POST /v1/audio/stream``): the first
        audio arrives after ~0.75 s instead of after the whole text. Validation errors and a Kyutai
        failure before the first chunk raise ``TtsError``; an older Kyutai without the streaming
        route (404/405) falls back to one whole-file synthesis. A complete stream is cached."""
        clean = normalize_for_speech(text, self.sentence_max_chars)
        if not clean:
            raise TtsError("empty text after normalization", status=400)
        limit = max_chars if max_chars is not None else self.max_chars
        if limit and len(clean) > limit:
            raise TtsError(f"text too long ({len(clean)} > {limit} chars)", status=413)
        voice = voice or self.default_voice
        key = (voice, "pcm16", clean)
        hit = self._cache_get(key)
        if hit is not None:
            yield hit.data
            return
        async with self.scheduler.slot(priority):
            self.calls += 1
            url = f"{self.base_url}/v1/audio/stream"
            body = {"input": clean, "voice": voice, "format": "pcm16"}
            received = bytearray()
            try:
                async with self.client.stream("POST", url, json=body, timeout=self.timeout) as resp:
                    if resp.status_code in (404, 405):
                        audio = None
                    elif resp.status_code != 200:
                        log.warning("kyutai stream error", extra=fields(status=resp.status_code))
                        raise TtsError(f"Kyutai returned HTTP {resp.status_code}")
                    else:
                        audio = True
                        async for chunk in resp.aiter_bytes():
                            if chunk:
                                received.extend(chunk)
                                yield chunk
            except httpx.HTTPError as exc:
                log.warning("kyutai stream failed", extra=fields(error=type(exc).__name__, sent=len(received)))
                if not received:
                    raise TtsError("Kyutai unreachable") from exc
                return  # mid-stream cut: the client keeps what it already played
            if audio is None:
                self.calls -= 1  # counted again by the whole-file call
                fallback = await self._call_kyutai(clean, voice, "pcm16")
                self._cache_put(key, fallback)
                yield fallback.data
                return
        if received:
            self._cache_put(key, Audio(bytes(received), FORMATS["pcm16"][1], "pcm16"))

    async def _call_kyutai(self, text: str, voice: str, fmt: str) -> Audio:
        kyutai_fmt, content_type = FORMATS[fmt]
        self.calls += 1
        try:
            resp = await self.client.post(
                f"{self.base_url}/v1/audio/speech",
                json={"model": "kyutai-tts", "input": text, "voice": voice, "response_format": kyutai_fmt},
                timeout=self.timeout,
            )
        except httpx.HTTPError as exc:
            log.warning("kyutai request failed", extra=fields(error=type(exc).__name__))
            raise TtsError("Kyutai unreachable") from exc
        if resp.status_code != 200:
            log.warning("kyutai error", extra=fields(status=resp.status_code))
            raise TtsError(f"Kyutai returned HTTP {resp.status_code}")
        body = resp.content
        if fmt == "pcm16":
            pcm, rate, channels = wav_to_pcm16(body)
            return Audio(pcm, content_type, fmt, rate, channels)
        return Audio(body, content_type, fmt)

    async def reachable(self) -> bool:
        try:
            resp = await self.client.get(f"{self.base_url}/health", timeout=2.0)
            return resp.status_code == 200
        except httpx.HTTPError:
            return False
