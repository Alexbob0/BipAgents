"""Kyutai Pocket TTS server for BipAgents: the Bips' voices, on CPU, in French, English, Spanish and German.

Speaks the Kyutai-style API the bridge uses (``bridge/bipbridge/tts.py``):

- ``GET  /health``           -> ``{"status": "ok", "languages": [...], "voices": {...}}``
- ``POST /v1/audio/speech``  ``{"input", "voice", "response_format": "wav"|"mp3"}`` -> the whole file
- ``POST /v1/audio/stream``  ``{"input", "voice"}`` -> raw PCM16 mono 24 kHz, chunk by chunk (~80 ms each)

``voice`` is ``loutre`` (French) or ``<code>/loutre`` (``en``, ``es``, ``de``): the voice state
``<voices>/<french|english|spanish|german>/loutre.safetensors`` with that language's model. Unknown voice: 404.

Run: ``python pocket_server.py --voices ../voices --languages fr,en,es,de`` (listens on 127.0.0.1:8098).
"""
from __future__ import annotations

import argparse
import asyncio
import io
import logging
import re
import threading
import time
import wave
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, Iterator, Optional, Tuple

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


class StreamRequest(BaseModel):
    input: str
    voice: str
    format: str = "pcm16"


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

    def stream(self, voice: str, text: str, stop: threading.Event) -> Iterator[bytes]:
        """PCM16 chunks as Pocket decodes them; ``stop`` ends the generation early."""
        model, state = self.resolve(voice)
        with self.lock:
            for chunk in model.generate_audio_stream(state, text, stop=stop):
                if stop.is_set():
                    break
                yield to_pcm16(chunk)

    def synthesize(self, voice: str, text: str) -> bytes:
        return b"".join(self.stream(voice, text, threading.Event()))


def to_pcm16(chunk) -> bytes:
    """A float tensor in [-1, 1] -> little-endian int16 bytes."""
    import torch

    return (chunk.detach().clamp(-1, 1) * 32767).to(torch.int16).cpu().numpy().tobytes()


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
        pcm = await run_in_threadpool(engine.synthesize, body.voice, body.input)
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
                for chunk in engine.stream(body.voice, body.input, stop):
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
