"""TTS client (Kyutai 1.6B, optional Kyutai Pocket TTS, optional voice server of the person's own: Fish Audio Drama 3,
Kokoro…): FIFO scheduler per engine, WAV -> PCM16 conversion, resampling to 24 kHz, small LRU cache.

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

from .config import VoiceServerConfig
from .logs import fields
from . import pronunciations, speech_intl
from .numbers_fr import prepare_for_synthesis
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
SERVER_PREFIX = "server:"
DRAMA_PREFIX = "drama:"  # a voice server's voice, as an earlier app named the Drama 3 ones (`drama:fr-mascotte-s17`)
OUTPUT_RATE = 24000      # what the app plays (and Kyutai, Pocket produce)
CHARS_PER_SECOND = 15.0
SLOW_SERVER_SECONDS = 30.0   # after a missed deadline, how long the Bips speak instead of the voice server


def voice_language(voice: str) -> str:
    """``pocket:en/colibri`` -> ``en``, ``server:en/af_bella`` -> ``en``, ``drama:en-naturelle-s47`` -> ``en``; French for
    ``pocket:colibri`` and Kyutai voices."""
    for prefix in (POCKET_PREFIX, SERVER_PREFIX):
        if voice.startswith(prefix):
            lang, sep, _ = voice[len(prefix):].partition("/")
            if sep and lang in speech_intl.LANGUAGES:
                return lang
    if voice.startswith(DRAMA_PREFIX):
        lang = voice[len(DRAMA_PREFIX):].partition("-")[0]
        if lang in speech_intl.LANGUAGES:
            return lang
    return "fr"


def is_server_voice(voice: str) -> bool:
    return voice.startswith(SERVER_PREFIX) or voice.startswith(DRAMA_PREFIX)


def server_voice_id(voice: str) -> str:
    """The id the voice server knows: ``server:fr/af_bella`` -> ``af_bella``, ``drama:fr-mascotte-s17`` as it is."""
    if voice.startswith(DRAMA_PREFIX):
        return voice[len(DRAMA_PREFIX):]
    rest = voice[len(SERVER_PREFIX):]
    lang, sep, name = rest.partition("/")
    return name if sep and lang in speech_intl.LANGUAGES else rest


def server_fallback(voice: str) -> str:
    """The Pocket voice standing in for a voice server's when it is down: a lively Bip for a mascot, a calm one
    otherwise, in the voice's language."""
    lang = voice_language(voice)
    name = "colibri" if "mascotte" in voice or "mascot" in voice else "galet"
    return f"{POCKET_PREFIX}{name}" if lang == "fr" else f"{POCKET_PREFIX}{lang}/{name}"


class Resampler:
    """PCM16 mono from one rate to another, chunk by chunk, keeping its filter state across chunks (no click at the
    joins): soxr when installed, else linear interpolation."""

    def __init__(self, source: int, target: int):
        self.source, self.target = source, target
        self._carry = b""
        try:
            import numpy
            import soxr
            self._np = numpy
            self._soxr = soxr.ResampleStream(source, target, 1, dtype="int16", quality="HQ")
        except ImportError:  # pragma: no cover - soxr is a requirement; kept working without it
            log.warning("soxr missing: linear resampling")
            self._soxr = None
            self._position = 0.0
            self._previous: Optional[int] = None

    def feed(self, data: bytes, last: bool = False) -> bytes:
        data = self._carry + data
        whole = len(data) - (len(data) % 2)
        data, self._carry = data[:whole], data[whole:]
        if self._soxr is not None:
            samples = self._np.frombuffer(data, dtype="<i2")
            return self._soxr.resample_chunk(samples, last=last).astype("<i2").tobytes()
        return self._linear(data)

    def _linear(self, data: bytes) -> bytes:  # pragma: no cover - fallback only
        samples = array.array("h", data)
        if sys.byteorder == "big":
            samples.byteswap()
        source = ([self._previous] if self._previous is not None else []) + list(samples)
        step = self.source / self.target
        out = array.array("h")
        while self._position + 1 < len(source):
            i = int(self._position)
            frac = self._position - i
            out.append(int(source[i] * (1 - frac) + source[i + 1] * frac))
            self._position += step
        if source:
            self._position -= len(source) - 1
            self._previous = source[-1]
        if sys.byteorder == "big":
            out.byteswap()
        return out.tobytes()


def pcm16_to_wav(pcm: bytes, rate: int, channels: int = 1) -> bytes:
    """A WAV file around PCM16 data."""
    return (b"RIFF" + struct.pack("<I", 36 + len(pcm)) + b"WAVEfmt " +
            struct.pack("<IHHIIHH", 16, 1, channels, rate, rate * channels * 2, channels * 2, 16) +
            b"data" + struct.pack("<I", len(pcm)) + pcm)


def mono(pcm: bytes, channels: int) -> bytes:
    """The first channel of interleaved PCM16."""
    samples = array.array("h", pcm[: len(pcm) - len(pcm) % (2 * channels)])
    return samples[::channels].tobytes()


def voice_names(data: object) -> List[str]:
    """The voice ids in a server's answer: `{"voices": ["a", …]}` (audio.cpp, Kokoro), `{"voices": [{"id": …}]}`,
    Kyutai's `{"aliases": {name: path}}`, or a bare list."""
    if isinstance(data, dict):
        data = data.get("voices", data.get("aliases", data.get("data")))
    if isinstance(data, dict):
        data = list(data)
    if not isinstance(data, list):
        return []
    names = [item if isinstance(item, str) else (item.get("id") or item.get("name") or item.get("voice_id"))
             for item in data if isinstance(item, (str, dict))]
    return [str(name) for name in names if name]


POCKET_ATTEMPTS = 2
POCKET_RETRY_DELAY = 0.5  # seconds


@dataclass
class Engine:
    """One TTS server with its own queue: the Kyutai API (``/v1/audio/speech``, ``/v1/audio/stream``), or OpenAI's
    ``/v1/audio/speech`` for the person's own voice server (``server``)."""
    name: str
    base_url: str
    scheduler: FifoScheduler
    health_path: str = "/health"


class TtsService:
    """Kyutai TTS 1.6B (GPU) plus, optionally, Kyutai Pocket TTS (CPU) for the Bips' voices.

    A voice named ``pocket:<name>`` (e.g. ``pocket:colibri``) goes to Pocket in French, ``pocket:<lang>/<name>``
    (``en``, ``es``, ``de``) to Pocket's model for that language; ``server:<lang>/<id>`` (or an earlier app's
    ``drama:<id>``) to the person's voice server; any other voice to Kyutai. Each engine has its own FIFO queue (they
    run on different hardware, and each generates one thing at a time). When Pocket is not configured, down or
    failing before its first audio, a French request falls back to Kyutai's default voice (that audio is not
    cached); another language fails instead, so the app reads it with the iPhone's voice in the right language.
    When the voice server is not configured or fails before its first audio, a Pocket Bip of the same language stands
    in.
    """

    def __init__(self, client: httpx.AsyncClient, base_url: Optional[str], default_voice: str,
                 cache_entries: int = 256, timeout: float = 120.0, max_chars: int = 1000,
                 sentence_max_chars: int = 180, pocket_url: Optional[str] = None,
                 voice_server: Optional[VoiceServerConfig] = None, words: Optional[dict] = None):
        self.client = client
        self.base_url = (base_url or "").rstrip("/")
        self.default_voice = default_voice
        self.cache_entries = max(0, cache_entries)
        self.timeout = timeout
        self.max_chars = max_chars
        self.sentence_max_chars = sentence_max_chars
        self.scheduler = FifoScheduler()
        # None: Pocket only (no GPU); then the default voice is a Pocket one and nothing falls back to Kyutai.
        self.kyutai = Engine("kyutai", self.base_url, self.scheduler) if base_url else None
        self.pocket = Engine("pocket", pocket_url.rstrip("/"), FifoScheduler()) if pocket_url else None
        self.voice_server = voice_server
        self.server = Engine("server", voice_server.url, FifoScheduler(), "/v1/models") if voice_server else None
        self.words = pronunciations.Lexicon(words or {})
        self._server_voices: Optional[Tuple[float, List[str]]] = None
        self._slow_until = 0.0
        self._cache: "OrderedDict[Tuple[str, str, str], Audio]" = OrderedDict()
        self.calls = 0  # engine requests actually made (diagnostics / tests)

    def _route(self, voice: str) -> Tuple[Engine, str]:
        if is_server_voice(voice):
            if self.server is not None:
                return self.server, server_voice_id(voice)
            return self._route(server_fallback(voice))
        if voice.startswith(POCKET_PREFIX):
            if self.pocket is not None:
                return self.pocket, voice[len(POCKET_PREFIX):]
            log.warning("pocket voice requested but pocket is not configured", extra=fields(voice=voice))
            if voice_language(voice) != "fr" or self.kyutai is None:
                raise TtsError("pocket is not configured", status=503)
            return self.kyutai, self.default_voice
        if self.kyutai is None:
            if not self.default_voice.startswith(POCKET_PREFIX):
                raise TtsError("kyutai is not configured", status=503)
            return self._route(self.default_voice)  # a Kyutai voice asked for, no Kyutai here: the default Bip
        return self.kyutai, voice

    def _paragraphs(self, voice: str) -> bool:
        """Pocket phrases the text itself (a pause per sentence, longer between lines): keep the line breaks."""
        return voice.startswith(POCKET_PREFIX) and self.pocket is not None

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

    def _prepare(self, text: str, normalized: bool, max_chars: Optional[int], language: str = "fr",
                 paragraphs: bool = False) -> str:
        clean = text.strip() if normalized else normalize_for_speech(text, self.sentence_max_chars, paragraphs)
        clean = self.words.apply(clean, language)   # « Garmin » as it is said, before numbers and CamelCase
        # numbers in words, CamelCase split, parentheses as pauses
        clean = prepare_for_synthesis(clean) if language == "fr" else speech_intl.prepare_for_synthesis(clean, language)
        if not clean:
            raise TtsError("empty text after normalization", status=400)
        limit = max_chars if max_chars is not None else self.max_chars
        if limit and len(clean) > limit:
            raise TtsError(f"text too long ({len(clean)} > {limit} chars)", status=413)
        return clean

    def _server_deadline(self, text: str, priority: int) -> Optional[float]:
        """Seconds an interactive request waits for the voice server's first sound once it is sent (not its turn in
        the queue: the earlier sentences of a Live are not the server's slowness); background ones (the outbox's
        reading) wait as long as it takes."""
        if priority != INTERACTIVE or self.voice_server is None:
            return None
        streaming = bool(self.voice_server.stream_rate)
        return self.voice_server.first_audio_seconds + (0 if streaming else len(text) / CHARS_PER_SECOND)

    def _server_slow(self) -> bool:
        """It just missed a deadline: for a while, interactive requests go to the Bips at once (one voice throughout a
        Live, not one sentence each)."""
        return asyncio.get_running_loop().time() < self._slow_until

    def _mark_server_slow(self, chars: int) -> None:
        self._slow_until = asyncio.get_running_loop().time() + SLOW_SERVER_SECONDS
        log.warning("voice server too slow, the Bips speak for a while", extra=fields(chars=chars))

    async def synthesize(self, text: str, voice: Optional[str] = None, fmt: str = "pcm16",
                         priority: int = INTERACTIVE, normalized: bool = False,
                         max_chars: Optional[int] = None, relay: Optional[str] = None) -> Audio:
        """`relay`: the Bip speaking instead of a voice server's voice when the server fails or is too slow (else a
        Bip of the same language)."""
        if fmt not in FORMATS:
            raise TtsError(f"unsupported format: {fmt}", status=400)
        voice = voice or self.default_voice
        language = voice_language(voice)
        clean = self._prepare(text, normalized, max_chars, language, paragraphs=self._paragraphs(voice))
        key = (voice, fmt, clean)
        hit = self._cache_get(key)
        if hit is not None:
            return Audio(hit.data, hit.content_type, hit.format, hit.sample_rate, hit.channels, cached=True)
        engine, engine_voice = self._route(voice)
        if engine is self.server:
            standin = relay or server_fallback(voice)
            deadline = self._server_deadline(clean, priority)
            if deadline is not None and self._server_slow():
                return await self.synthesize(text, standin, fmt, priority, normalized, max_chars)
            try:
                async with engine.scheduler.slot(priority):
                    if deadline is not None and self._server_slow():   # missed by an earlier one while this waited
                        raise asyncio.TimeoutError
                    audio = await asyncio.wait_for(self._call_server(clean, engine_voice, fmt), deadline)
            except asyncio.TimeoutError:
                if not self._server_slow():
                    self._mark_server_slow(len(clean))
                return await self.synthesize(text, standin, fmt, priority, normalized, max_chars)
            except TtsError as exc:
                if exc.status < 500 and fmt in ("pcm16", "wav"):
                    raise
                log.warning("voice server failed, using a pocket voice", extra=fields(error=str(exc), format=fmt))
                return await self.synthesize(text, standin, fmt, priority, normalized, max_chars)
            self._cache_put(key, audio)
            return audio
        attempts = POCKET_ATTEMPTS if engine is self.pocket else 1
        for attempt in range(1, attempts + 1):
            try:
                async with engine.scheduler.slot(priority):
                    hit = self._cache_get(key)  # an identical request may have just finished
                    if hit is not None:
                        return Audio(hit.data, hit.content_type, hit.format, hit.sample_rate, hit.channels, cached=True)
                    audio = await self._call_engine(engine, clean, engine_voice, fmt)
                break
            except TtsError as exc:
                if engine is self.pocket and language != "fr" and exc.status < 500:
                    # e.g. this language's voices are not installed: 503 so the app reads it with the iPhone's voice
                    raise TtsError(f"pocket {language} voice unavailable: {exc}", status=503) from exc
                if engine is self.kyutai or exc.status < 500:
                    raise
                if attempt < attempts:
                    log.info("pocket failed, retrying", extra=fields(error=str(exc)))
                    await asyncio.sleep(POCKET_RETRY_DELAY)
                    continue
                if language != "fr" or self.kyutai is None:
                    raise
                log.warning("pocket failed, falling back to kyutai", extra=fields(error=str(exc), text=clean[:60]))
                async with self.kyutai.scheduler.slot(priority):
                    return await self._call_engine(self.kyutai, clean, self.default_voice, fmt)
        self._cache_put(key, audio)
        return audio

    async def stream(self, text: str, voice: Optional[str] = None, priority: int = INTERACTIVE,
                     max_chars: Optional[int] = None, relay: Optional[str] = None) -> AsyncIterator[bytes]:
        """PCM16 mono 24 kHz chunks as the engine produces them (``POST /v1/audio/stream``): the first
        audio arrives after ~0.75 s with Kyutai, ~0.1 s with Pocket, instead of after the whole text.
        Validation errors and an engine failure before the first chunk raise ``TtsError``; an engine
        without the streaming route (404/405) falls back to one whole-file synthesis. Only a complete
        stream is cached."""
        voice = voice or self.default_voice
        language = voice_language(voice)
        clean = self._prepare(text, False, max_chars, language, paragraphs=self._paragraphs(voice))
        key = (voice, "pcm16", clean)
        hit = self._cache_get(key)
        if hit is not None:
            yield hit.data
            return
        engine, engine_voice = self._route(voice)
        received = bytearray()
        if engine is self.server:
            standin = relay or server_fallback(voice)
            deadline = self._server_deadline(clean, priority)
            try:
                if deadline is not None and self._server_slow():
                    raise asyncio.TimeoutError
                async for chunk in self._stream_server(clean, engine_voice, priority, deadline):
                    received.extend(chunk)
                    yield chunk
            except asyncio.TimeoutError:
                if received:
                    return
                if not self._server_slow():
                    self._mark_server_slow(len(clean))
                async for chunk in self.stream(text, standin, priority, max_chars):
                    yield chunk
                return
            except TtsError as exc:
                if received:
                    return  # cut mid-stream: the client keeps what it already played, nothing is cached
                if exc.status < 500:
                    raise
                log.warning("voice server stream failed, using a pocket voice", extra=fields(error=str(exc)))
                async for chunk in self.stream(text, standin, priority, max_chars):
                    yield chunk
                return
            if received:
                self._cache_put(key, Audio(bytes(received), FORMATS["pcm16"][1], "pcm16"))
            return
        attempts = POCKET_ATTEMPTS if engine is self.pocket else 1
        for attempt in range(1, attempts + 1):
            try:
                async for chunk in self._stream_engine(engine, clean, engine_voice, priority):
                    received.extend(chunk)
                    yield chunk
                break
            except TtsError as exc:
                if received:
                    return  # cut mid-stream: the client keeps what it already played, nothing is cached
                if engine is self.pocket and language != "fr" and exc.status < 500:
                    raise TtsError(f"pocket {language} voice unavailable: {exc}", status=503) from exc
                if engine is self.kyutai or exc.status < 500:
                    raise
                if attempt < attempts:
                    # Usually busy finishing a cancelled request (one generation at a time): try once more.
                    log.info("pocket stream failed, retrying", extra=fields(error=str(exc)))
                    await asyncio.sleep(POCKET_RETRY_DELAY)
                    continue
                if language != "fr" or self.kyutai is None:
                    raise
                log.warning("pocket stream failed, falling back to kyutai", extra=fields(error=str(exc), text=clean[:60]))
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

    def _server_body(self, text: str, voice: str, streaming: bool) -> dict:
        assert self.voice_server is not None
        body = {"model": self.voice_server.model, "input": text, "voice": voice,
                "response_format": "pcm" if streaming else "wav"}
        if streaming:
            body["stream_format"] = "audio"
        return body

    @staticmethod
    def _take_limit(text: str) -> int:
        """Most 24 kHz bytes a take should reach: some models (Drama 3) read sentences again after the last one, so a
        take running past twice its expected length (15 characters a second) is cut."""
        expected = len(text) / CHARS_PER_SECOND
        return int(max(2 * expected, expected + 4) * OUTPUT_RATE * 2)

    @staticmethod
    def _server_error(status: int) -> TtsError:
        return TtsError(f"voice server returned HTTP {status}", status=400 if status in (400, 404, 422) else 502)

    async def _stream_server(self, text: str, voice: str, priority: int,
                             deadline: Optional[float] = None) -> AsyncIterator[bytes]:
        """The voice server's raw PCM as it comes, resampled to 24 kHz; a server that does not stream gives its whole
        WAV file at once. Closing the stream (the client gone, a take too long, no first sound within `deadline`
        seconds once sent: `asyncio.TimeoutError`) stops the generation on the server."""
        assert self.server is not None and self.voice_server is not None
        rate = self.voice_server.stream_rate
        if not rate:
            async with self.server.scheduler.slot(priority):
                if deadline is not None and self._server_slow():
                    raise asyncio.TimeoutError
                audio = await asyncio.wait_for(self._call_server(text, voice, "pcm16"), deadline)
            yield audio.data
            return
        async with self.server.scheduler.slot(priority):
            if deadline is not None and self._server_slow():
                raise asyncio.TimeoutError
            self.calls += 1
            resampler = Resampler(rate, OUTPUT_RATE)
            limit = self._take_limit(text)
            sent = 0
            try:
                async with self.client.stream("POST", f"{self.server.base_url}/v1/audio/speech",
                                              json=self._server_body(text, voice, True), timeout=self.timeout) as resp:
                    if resp.status_code != 200:
                        log.warning("tts stream error", extra=fields(engine="server", status=resp.status_code))
                        raise self._server_error(resp.status_code)
                    pieces = resp.aiter_bytes()
                    waiting = deadline   # for the first piece only
                    while True:
                        try:
                            chunk = await asyncio.wait_for(pieces.__anext__(), waiting)
                        except StopAsyncIteration:
                            break
                        waiting = None
                        out = resampler.feed(chunk)
                        if out:
                            sent += len(out)
                            yield out
                        if sent > limit:
                            log.warning("voice server take too long, cut",
                                        extra=fields(chars=len(text), seconds=sent / 2 / OUTPUT_RATE))
                            return
                    tail = resampler.feed(b"", last=True)
                    if tail:
                        yield tail
            except httpx.HTTPError as exc:
                log.warning("tts stream failed", extra=fields(engine="server", error=type(exc).__name__, sent=sent))
                raise TtsError("voice server unreachable" if not sent else "voice server stream cut") from exc

    async def _call_server(self, text: str, voice: str, fmt: str) -> Audio:
        """WAV from the server, as PCM16 at 24 kHz; mp3 or opus asked as such (a server without them fails and a Bip
        stands in: no encoder here)."""
        assert self.server is not None
        self.calls += 1
        body = self._server_body(text, voice, False)
        if fmt in ("mp3", "opus"):
            body["response_format"] = fmt
        try:
            resp = await self.client.post(f"{self.server.base_url}/v1/audio/speech", json=body, timeout=self.timeout)
        except httpx.HTTPError as exc:
            log.warning("tts request failed", extra=fields(engine="server", error=type(exc).__name__))
            raise TtsError("voice server unreachable") from exc
        if resp.status_code != 200:
            log.warning("tts error", extra=fields(engine="server", status=resp.status_code))
            raise self._server_error(resp.status_code)
        if fmt in ("mp3", "opus"):
            return Audio(resp.content, FORMATS[fmt][1], fmt)
        pcm, rate, channels = wav_to_pcm16(resp.content)
        if channels != 1:
            pcm = mono(pcm, channels)
        if rate != OUTPUT_RATE:
            pcm = Resampler(rate, OUTPUT_RATE).feed(pcm, last=True)
        pcm = pcm[: self._take_limit(text)]
        if fmt == "wav":
            return Audio(pcm16_to_wav(pcm, OUTPUT_RATE), FORMATS["wav"][1], "wav", OUTPUT_RATE, 1)
        return Audio(pcm, FORMATS["pcm16"][1], "pcm16", OUTPUT_RATE, 1)

    async def server_voices(self, max_age: float = 300.0) -> List[str]:
        """The voice server's voices, in its order: `GET /v1/audio/voices` (audio.cpp, Kokoro), Kyutai's
        `GET /v1/voices`, else the ones its config lists. Kept a few minutes."""
        if self.voice_server is None:
            return []
        now = asyncio.get_running_loop().time()
        if self._server_voices and now - self._server_voices[0] < max_age:
            return self._server_voices[1]
        found: List[str] = []
        for path in ("/v1/audio/voices", "/v1/voices"):
            try:
                resp = await self.client.get(f"{self.voice_server.url}{path}", timeout=5.0)
                data = resp.json() if resp.status_code == 200 else None
            except (httpx.HTTPError, ValueError):
                continue
            found = voice_names(data)
            if found:
                break
        found = found or list(self.voice_server.voices)
        if found:
            self._server_voices = (now, found)
        return found

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

    async def reachable(self, engine: Optional[Engine] = None) -> Optional[bool]:
        """Whether the engine (Kyutai by default) answers ``/health``; None when it is not configured."""
        engine = engine or self.kyutai
        if engine is None:
            return None
        try:
            resp = await self.client.get(f"{engine.base_url}{engine.health_path}", timeout=2.0)
            return resp.status_code == 200
        except httpx.HTTPError:
            return False
