"""Kyutai Pocket TTS server for BipAgents: the Bips' voices, on CPU, in French, English, Spanish and German.

Speaks the Kyutai-style API the bridge uses (``bridge/bipbridge/tts.py``):

- ``GET  /health``           -> ``{"status": "ok", "languages": [...], "voices": {...}}``
- ``POST /v1/audio/speech``  ``{"input", "voice", "response_format": "wav"|"mp3"}`` -> the whole file
- ``POST /v1/audio/stream``  ``{"input", "voice"}`` -> raw PCM16 mono 24 kHz, chunk by chunk (~80 ms each)

``voice`` is ``loutre`` (French) or ``<code>/loutre`` (``en``, ``es``, ``de``): the voice state
``<voices>/<french|english|spanish|german>/loutre.safetensors`` with that language's model. Unknown voice: 404.

Phrasing: the model is trained on single sentences and, left alone, packs several into one generation (rushed,
with erratic pauses). Here each sentence is generated on its own (very short ones join their neighbour), the
model's own leading/trailing silence is trimmed, each sentence gets a moderate level adjustment, and the pause
after it follows the punctuation and line breaks (paragraphs, list items). ``pace: "live"`` shortens the pauses.

Run: ``python pocket_server.py --voices ../voices --languages fr,en,es,de`` (listens on 127.0.0.1:8098).
"""
from __future__ import annotations

import argparse
import asyncio
import io
import logging
import random
import re
import threading
import time
import wave
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Dict, Iterable, Iterator, List, Optional, Tuple

from pydantic import BaseModel

log = logging.getLogger("pocket_server")

LANGUAGE_DIRS = {"fr": "french", "en": "english", "es": "spanish", "de": "german"}
VOICE_NAME = re.compile(r"^[a-z0-9_-]{1,40}$")


class VoiceNotFound(Exception):
    pass


class SpeechRequest(BaseModel):
    input: str
    voice: str
    response_format: str = "wav"
    model: Optional[str] = None
    pace: str = "read"


class StreamRequest(BaseModel):
    input: str
    voice: str
    format: str = "pcm16"
    pace: str = "read"


# Pause after a sentence (seconds), by its final punctuation; "para" after a line (paragraph, list item).
PAUSES = {
    "read": {".": 0.32, "!": 0.36, "?": 0.40, "…": 0.55, "para": 0.62, "": 0.30},
    "live": {".": 0.22, "!": 0.24, "?": 0.26, "…": 0.38, "para": 0.40, "": 0.20},
}
SHORT_WORDS = 3  # "Oui." "C'est noté." are generated with their neighbour, not alone
_SENTENCE = re.compile(r".+?(?:[.!?…]+[\"'»”)\]]*(?=\s|$)|$)")
_TRAILING = re.compile(r"[\"'»”)\]\s]+$")


def phrases(text: str) -> List[Tuple[str, str]]:
    """``[(sentence, pause kind), …]``: one generation per sentence; the last of each line pauses as a paragraph."""
    out: List[Tuple[str, str]] = []
    for line in (l.strip() for l in text.split("\n")):
        if not line:
            continue
        merged: List[str] = []
        for part in (p.strip() for p in _SENTENCE.findall(line)):
            if not part:
                continue
            if merged and len(merged[-1].split()) <= SHORT_WORDS:
                merged[-1] += " " + part
            else:
                merged.append(part)
        if len(merged) > 1 and len(merged[-1].split()) <= SHORT_WORDS:
            last = merged.pop()
            merged[-1] += " " + last
        for i, sentence in enumerate(merged):
            end = _TRAILING.sub("", sentence)[-1:]
            out.append((sentence, "para" if i == len(merged) - 1 else (end if end in ".!?…" else "")))
    return out


def shape(chunks: Iterable, rate: int, lead_threshold: float = 0.006, tail_ratio: float = 0.06,
          lead_pad: float = 0.05, tail_pad: float = 0.08, fade: float = 0.02, target: float = 0.075,
          max_gain: float = 1.6, min_gain: float = 0.7, gain_window: float = 0.2) -> Iterator:
    """One sentence's float audio chunks, streamed: leading silence dropped, trailing silence held back and cut at
    the end, and one gain for the sentence, set on its first ``gain_window`` seconds of voice (loud and quiet
    sentences meet halfway without flattening their inner dynamics).

    The end of a sentence is judged relative to its own level (``tail_ratio`` of its voiced RMS): light, high
    voices end softly, and a fixed threshold clipped their last syllable. ``tail_pad`` is kept after the voice,
    with a short fade-out, so the cut never clicks."""
    import numpy as np

    frame = max(1, int(rate * 0.01))
    lead_margin, tail_margin, fade_len = int(rate * lead_pad), int(rate * tail_pad), int(rate * fade)
    started = False
    gain: Optional[float] = None
    tail_threshold = lead_threshold
    pending = np.zeros(0, dtype=np.float32)  # not yet emitted: waiting for the gain, or possibly trailing silence

    def energy(x):
        n = len(x) // frame
        if n == 0:
            return np.zeros(0, dtype=np.float32)
        return np.sqrt(np.mean(x[: n * frame].reshape(n, frame) ** 2, axis=1))

    def last_voiced_end(x) -> Optional[int]:
        voiced = energy(x) > tail_threshold
        return (len(voiced) - int(np.argmax(voiced[::-1]))) * frame if voiced.any() else None

    def set_gain(x) -> None:
        nonlocal gain, tail_threshold
        e = energy(x)
        voiced = e[e > lead_threshold]
        rms = float(np.sqrt(np.mean(voiced ** 2))) if len(voiced) else target
        gain = min(max_gain, max(min_gain, target / (rms or target)))
        tail_threshold = max(0.003, tail_ratio * rms)

    for chunk in chunks:
        pending = np.concatenate([pending, np.asarray(chunk, dtype=np.float32).reshape(-1)])
        if not started:
            voiced = energy(pending) > lead_threshold
            if not voiced.any():
                pending = pending[-lead_margin:] if lead_margin else pending[:0]
                continue
            pending = pending[max(0, int(np.argmax(voiced)) * frame - lead_margin):]
            started = True
        if gain is None:
            if (energy(pending) > lead_threshold).sum() * frame < rate * gain_window:
                continue
            set_gain(pending)
        end = last_voiced_end(pending)
        if end is not None:
            yield pending[:end] * gain
            pending = pending[end:]
    if not started:
        return
    if gain is None:  # a sentence shorter than the gain window
        set_gain(pending)
        end = last_voiced_end(pending)
        if end is None:
            end = len(pending)
        tail = pending[: end + tail_margin] * gain
    else:
        tail = pending[:tail_margin] * gain
    if fade_len and len(tail) > fade_len:
        tail = tail.copy()
        tail[-fade_len:] *= np.linspace(1, 0, fade_len, dtype=np.float32)
    yield tail


def phrased(text: str, generate: Callable[[str], Iterable], rate: int, pace: str = "read",
            stop: Optional[threading.Event] = None, jitter: float = 0.1) -> Iterator:
    """The whole text as float audio chunks: each sentence generated and shaped, then its pause (the last one
    too, shorter: it is the gap before the next request's sentence in Live)."""
    import numpy as np

    pauses = PAUSES.get(pace, PAUSES["read"])
    items = phrases(text)
    for index, (sentence, kind) in enumerate(items):
        if stop is not None and stop.is_set():
            return
        yield from shape(generate(sentence), rate)
        if index == len(items) - 1:
            # After the last one too, by its punctuation: in Live the app sends one sentence per request and
            # plays them back to back, so this is the breath between them.
            kind = _TRAILING.sub("", sentence)[-1:]
        seconds = pauses.get(kind, pauses[""]) * random.uniform(1 - jitter, 1 + jitter)
        yield np.zeros(int(rate * seconds), dtype=np.float32)


@dataclass
class Engine:
    """The loaded models and voice states. Pocket generation is not thread-safe and uses the CPU fully, so
    one generation runs at a time (the bridge queues its requests anyway)."""
    voices_dir: Path
    models: Dict[str, object] = field(default_factory=dict)  # code -> TTSModel
    states: Dict[Tuple[str, str], object] = field(default_factory=dict)
    lock: threading.Lock = field(default_factory=threading.Lock)

    @classmethod
    def load(cls, voices_dir: Path, languages: list[str], quantize: bool = False) -> "Engine":
        from pocket_tts import TTSModel  # heavy import, kept out of the tests

        engine = cls(voices_dir)
        for code in languages:
            started = time.monotonic()
            engine.models[code] = TTSModel.load_model(language=LANGUAGE_DIRS[code], quantize=quantize)
            log.info("loaded %s model in %.1f s", LANGUAGE_DIRS[code], time.monotonic() - started)
        return engine

    @property
    def sample_rate(self) -> int:
        model = next(iter(self.models.values()), None)
        return getattr(model, "sample_rate", 24000)

    def available_voices(self) -> Dict[str, list[str]]:
        return {code: sorted(p.stem for p in (self.voices_dir / LANGUAGE_DIRS[code]).glob("*.safetensors"))
                for code in self.models}

    def resolve(self, voice: str) -> Tuple[object, object]:
        """``"en/loutre"`` -> (English model, loutre's English voice state)."""
        code, sep, name = voice.partition("/")
        if not sep:
            code, name = "fr", voice
        model = self.models.get(code)
        if model is None or not VOICE_NAME.match(name):
            raise VoiceNotFound(voice)
        key = (code, name)
        if key not in self.states:
            path = self.voices_dir / LANGUAGE_DIRS[code] / f"{name}.safetensors"
            if not path.is_file():
                raise VoiceNotFound(voice)
            self.states[key] = model.get_state_for_audio_prompt(path)
        return model, self.states[key]

    def stream(self, voice: str, text: str, stop: threading.Event, pace: str = "read") -> Iterator[bytes]:
        """PCM16 chunks as Pocket decodes them, phrased sentence by sentence; ``stop`` ends the generation early."""
        model, state = self.resolve(voice)

        def generate(sentence: str) -> Iterator:
            # Pocket's own end-of-speech margin (its frames_after_eos guess): forcing it short clipped endings.
            for chunk in model.generate_audio_stream(state, sentence, stop=stop):
                yield chunk.detach().cpu().numpy()

        with self.lock:
            for audio in phrased(text, generate, self.sample_rate, pace, stop):
                if stop.is_set():
                    break
                if len(audio):
                    yield to_pcm16(audio)

    def synthesize(self, voice: str, text: str, pace: str = "read") -> bytes:
        return b"".join(self.stream(voice, text, threading.Event(), pace))


def to_pcm16(audio) -> bytes:
    """Float samples in [-1, 1] -> little-endian int16 bytes."""
    import numpy as np

    return (np.clip(np.asarray(audio, dtype=np.float32), -1, 1) * 32767).astype("<i2").tobytes()


def wav_bytes(pcm: bytes, rate: int) -> bytes:
    out = io.BytesIO()
    with wave.open(out, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(rate)
        wav.writeframes(pcm)
    return out.getvalue()


def mp3_bytes(pcm: bytes, rate: int) -> bytes:
    import lameenc

    encoder = lameenc.Encoder()
    encoder.set_bit_rate(64)
    encoder.set_in_sample_rate(rate)
    encoder.set_channels(1)
    encoder.set_quality(2)
    return bytes(encoder.encode(pcm) + encoder.flush())


def create_app(engine: Engine):
    from fastapi import FastAPI, HTTPException
    from fastapi.responses import Response, StreamingResponse
    from starlette.concurrency import run_in_threadpool

    app = FastAPI(title="BipAgents Pocket TTS", docs_url=None, redoc_url=None)

    def check(text: str, voice: str) -> None:
        if not text.strip():
            raise HTTPException(400, "empty input")
        try:
            engine.resolve(voice)
        except VoiceNotFound:
            raise HTTPException(404, f"unknown voice: {voice}") from None

    @app.get("/health")
    async def health() -> dict:
        return {"status": "ok", "languages": list(engine.models), "voices": engine.available_voices()}

    @app.post("/v1/audio/speech")
    async def speech(body: SpeechRequest) -> Response:
        check(body.input, body.voice)
        if body.response_format not in ("wav", "mp3"):
            raise HTTPException(415, f"unsupported format: {body.response_format}")
        pcm = await run_in_threadpool(engine.synthesize, body.voice, body.input, body.pace)
        if body.response_format == "mp3":
            return Response(await run_in_threadpool(mp3_bytes, pcm, engine.sample_rate), media_type="audio/mpeg")
        return Response(wav_bytes(pcm, engine.sample_rate), media_type="audio/wav")

    @app.post("/v1/audio/stream")
    async def stream(body: StreamRequest) -> StreamingResponse:
        check(body.input, body.voice)
        stop = threading.Event()
        loop = asyncio.get_running_loop()
        queue: asyncio.Queue = asyncio.Queue()

        def produce() -> None:
            try:
                for chunk in engine.stream(body.voice, body.input, stop, body.pace):
                    loop.call_soon_threadsafe(queue.put_nowait, chunk)
            except Exception as exc:  # reported by ending the stream early
                log.warning("generation failed: %s", exc)
            finally:
                loop.call_soon_threadsafe(queue.put_nowait, None)

        threading.Thread(target=produce, daemon=True).start()

        async def chunks():
            try:
                while (chunk := await queue.get()) is not None:
                    yield chunk
            finally:
                stop.set()  # client gone (barge-in, cancelled reply): stop generating

        return StreamingResponse(chunks(), media_type="application/octet-stream")

    return app


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--voices", type=Path, default=Path(__file__).resolve().parent.parent / "voices",
                        help="folder holding french/, english/, spanish/, german/ (default: the repo's voices/)")
    parser.add_argument("--languages", default="fr,en,es,de", help="comma-separated: fr, en, es, de")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8098)
    parser.add_argument("--quantize", action="store_true", help="int8 weights: less memory, faster on x86 CPUs")
    parser.add_argument("--threads", type=int, default=0,
                        help="CPU threads for generation (default: PyTorch's choice); 2 leaves room for Hermes on a 4 vCPU VPS")
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s: %(message)s")
    languages = [code.strip() for code in args.languages.split(",") if code.strip()]
    unknown = [code for code in languages if code not in LANGUAGE_DIRS]
    if unknown:
        parser.error(f"unknown language(s): {', '.join(unknown)} (use {', '.join(LANGUAGE_DIRS)})")
    if args.threads > 0:
        import torch

        torch.set_num_threads(args.threads)
    engine = Engine.load(args.voices, languages, quantize=args.quantize)
    log.info("voices: %s", engine.available_voices())

    import uvicorn

    uvicorn.run(create_app(engine), host=args.host, port=args.port, log_level="warning")


if __name__ == "__main__":
    main()
