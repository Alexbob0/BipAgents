"""Kyutai Pocket TTS server for BipAgents: the Bips' voices, on CPU, in French, English, Spanish and German.

Speaks the Kyutai-style API the bridge uses (``bridge/bipbridge/tts.py``):

- ``GET  /health``           -> ``{"status": "ok", "engine": "4.0.1", "languages": [...], "voices": {...}}`` (``engine``:
  the Pocket version speaking, pocket-tts' own or the packages' lowest: the bridge's ``[pocket] engine`` follows it)
- ``POST /v1/audio/speech``  ``{"input", "voice", "response_format": "wav"|"mp3"}`` -> the whole file
- ``POST /v1/audio/stream``  ``{"input", "voice"}`` -> raw PCM16 mono 24 kHz, chunk by chunk (~80 ms each)

``voice`` is ``colibri`` (French) or ``<code>/colibri`` (``en``, ``es``, ``de``): the voice state
``<voices>/<french|english|spanish|german>/colibri.safetensors`` with that language's model. Unknown voice: 404.

Phrasing: the model is trained on single sentences and, left alone, packs several into one generation (rushed,
with erratic pauses). Here each sentence is generated on its own (very short ones join their neighbour), the
model's own leading/trailing silence is trimmed, each sentence gets a moderate level adjustment, and the pause
after it follows the punctuation and line breaks (paragraphs, list items). ``pace: "live"`` shortens the pauses.

Memory: a language's model takes ~450 MB. Only the first language (``--keep``) is loaded at start; the others load
on their first sentence (a few seconds once) and are unloaded after ``--idle-unload`` seconds without speaking.

``--onnx <folder>``: the phones' voice packages instead (voices/onnx/build.py: ``index.json``, one folder per language
and the shared decoder): the same Bips, 8-bit, ~250 MB a language while it speaks, with ONNX Runtime and numpy only
(no PyTorch, 5.5 GB less on disk). With ``--keep none --idle-unload 120``, a server's podcasts and audio files load
the model only while they are made.

Run: ``python pocket_server.py --voices ../voices --languages fr,en,es,de`` (listens on 127.0.0.1:8098), or
``python pocket_server.py --onnx ~/.local/share/bipagents/voices --keep none --idle-unload 120``.
"""
from __future__ import annotations

import argparse
import asyncio
import io
import json
import logging
import random
import re
import sys
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


def trails_off(sentence: str) -> bool:
    return _TRAILING.sub("", sentence).endswith(("…", "..."))


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
        # « aujourd'hui… »: the voice trails off, keep more of it than after a plain full stop.
        yield from shape(generate(sentence), rate, tail_pad=0.2 if trails_off(sentence) else 0.08)
        if index == len(items) - 1:
            # After the last one too, by its punctuation: in Live the app sends one sentence per request and
            # plays them back to back, so this is the breath between them.
            kind = _TRAILING.sub("", sentence)[-1:]
        seconds = pauses.get(kind, pauses[""]) * random.uniform(1 - jitter, 1 + jitter)
        yield np.zeros(int(rate * seconds), dtype=np.float32)


@dataclass
class Engine:
    """The models and voice states, loaded when a language is first spoken and unloaded when it stays silent.
    Pocket generation is not thread-safe and uses the CPU fully, so one generation runs at a time (the bridge
    queues its requests anyway); loading and unloading happen under the same lock."""
    voices_dir: Path
    languages: List[str] = field(default_factory=lambda: ["fr"])
    models: Dict[str, object] = field(default_factory=dict)  # code -> TTSModel
    states: Dict[Tuple[str, str], object] = field(default_factory=dict)
    lock: threading.Lock = field(default_factory=threading.Lock)
    # Builds a language's model (``TTSModel.load_model``; a fake in the tests).
    loader: Optional[Callable[[str], object]] = None
    # Never unloaded (the main language); the others go after ``idle`` seconds without speaking (0: never).
    keep: set = field(default_factory=set)
    idle: float = 0
    last_used: Dict[str, float] = field(default_factory=dict)
    # The phones' voice packages (--onnx): voices are the packages' Bips, not voices_dir's .safetensors.
    onnx: Optional[Path] = None
    # The Pocket version speaking (« 4.0.1 »): pocket-tts', or the packages' (their manifests' `engine`).
    version: Optional[str] = None

    @classmethod
    def load(cls, voices_dir: Path, languages: list[str], quantize: bool = False, keep: Optional[set] = None,
             idle: float = 0) -> "Engine":
        def loader(code: str) -> object:
            from pocket_tts import TTSModel  # heavy import, kept out of the tests

            return TTSModel.load_model(language=LANGUAGE_DIRS[code], quantize=quantize)

        engine = cls(voices_dir, languages=languages, loader=loader, keep=keep if keep is not None else set(languages[:1]),
                     idle=idle, version=pocket_version())
        check_voices(voices_dir, languages, engine.version)
        with engine.lock:
            for code in engine.keep:
                engine.model(code)
        return engine

    def model(self, code: str) -> object:
        """The language's model, loaded on its first use. Call with ``lock`` held."""
        model = self.models.get(code)
        if model is None:
            if code not in self.languages or self.loader is None:
                raise VoiceNotFound(code)
            started = time.monotonic()
            model = self.models[code] = self.loader(code)
            log.info("loaded %s model in %.1f s", LANGUAGE_DIRS[code], time.monotonic() - started)
        self.last_used[code] = time.monotonic()
        return model

    def unload_idle(self, now: Optional[float] = None) -> list[str]:
        """Unloads the languages silent for ``idle`` seconds (not while a generation runs): their memory goes back
        to the system. Returns the languages unloaded."""
        if not self.idle or not self.lock.acquire(blocking=False):
            return []
        try:
            now = time.monotonic() if now is None else now
            gone = [code for code in list(self.models)
                    if code not in self.keep and now - self.last_used.get(code, now) >= self.idle]
            for code in gone:
                del self.models[code]
                for key in [key for key in self.states if key[0] == code]:
                    del self.states[key]
                log.info("unloaded %s model after %.0f s of silence", LANGUAGE_DIRS[code], self.idle)
        finally:
            self.lock.release()
        if gone:
            release_memory()
        return gone

    def watch_idle(self, every: float = 30) -> None:
        """Unloads silent languages in the background."""
        def loop() -> None:
            while True:
                time.sleep(every)
                self.unload_idle()

        if self.idle:
            threading.Thread(target=loop, name="pocket-unload", daemon=True).start()

    @property
    def sample_rate(self) -> int:
        model = next(iter(self.models.values()), None)
        return getattr(model, "sample_rate", 24000)

    @classmethod
    def load_onnx(cls, packages: Path, keep: Optional[set] = None, idle: float = 0, threads: int = 2) -> "Engine":
        """The phones' voice packages instead of Pocket's models: the languages are those whose package is there."""
        languages = sorted(onnx_voices(packages))
        engine = cls(packages, languages=languages, loader=lambda code: OnnxModel(packages, code, threads),
                     keep=keep if keep is not None else set(languages[:1]), idle=idle, onnx=packages,
                     version=onnx_version(packages))
        with engine.lock:
            for code in engine.keep & set(languages):
                engine.model(code)
        return engine

    def available_voices(self) -> Dict[str, list[str]]:
        if self.onnx is not None:
            return {code: names for code, names in onnx_voices(self.onnx).items() if code in self.languages}
        return {code: sorted(p.stem for p in (self.voices_dir / LANGUAGE_DIRS[code]).glob("*.safetensors"))
                for code in self.languages}

    def split(self, voice: str) -> Tuple[str, str]:
        """``"en/colibri"`` -> ("en", "colibri"), checked against the languages served and the voices on disk."""
        code, sep, name = voice.partition("/")
        if not sep:
            code, name = "fr", voice
        if code not in self.languages or not VOICE_NAME.match(name):
            raise VoiceNotFound(voice)
        if self.onnx is not None:
            if name not in self.available_voices().get(code, []):
                raise VoiceNotFound(voice)
        elif not (self.voices_dir / LANGUAGE_DIRS[code] / f"{name}.safetensors").is_file():
            raise VoiceNotFound(voice)
        return code, name

    def resolve(self, voice: str) -> Tuple[object, object]:
        """``"en/colibri"`` -> (English model, colibri's English voice state). Call with ``lock`` held."""
        code, name = self.split(voice)
        model = self.model(code)
        key = (code, name)
        if key not in self.states:
            prompt = name if self.onnx is not None else self.voices_dir / LANGUAGE_DIRS[code] / f"{name}.safetensors"
            self.states[key] = model.get_state_for_audio_prompt(prompt)
        return model, self.states[key]

    def stream(self, voice: str, text: str, stop: threading.Event, pace: str = "read") -> Iterator[bytes]:
        """PCM16 chunks as Pocket decodes them, phrased sentence by sentence; ``stop`` ends the generation early."""

        def generate(sentence: str) -> Iterator:
            # Pocket's own end-of-speech margin (its frames_after_eos guess): forcing it short clipped endings.
            # A trailing-off « … » gets more room (6 frames, ~0.5 s) to fade out by itself.
            extra = {"frames_after_eos": 6} if trails_off(sentence) else {}
            for chunk in model.generate_audio_stream(state, sentence, stop=stop, **extra):
                yield chunk.detach().cpu().numpy() if hasattr(chunk, "detach") else chunk   # Pocket's torch, or numpy

        with self.lock:
            model, state = self.resolve(voice)
            try:
                for audio in phrased(text, generate, self.sample_rate, pace, stop):
                    if stop.is_set():
                        break
                    if len(audio):
                        yield to_pcm16(audio)
            finally:
                # Silence counts from the end of speech (also when the listener left mid-sentence).
                self.last_used[self.split(voice)[0]] = time.monotonic()

    def synthesize(self, voice: str, text: str, pace: str = "read") -> bytes:
        return b"".join(self.stream(voice, text, threading.Event(), pace))


class OnnxModel:
    """A language's voice package (voices/onnx/reference.py's loop, as the apps speak) behind the Pocket model's
    interface: the Bips' voices are the package's, the state of one is its name."""
    sample_rate = 24000

    def __init__(self, packages: Path, code: str, threads: int = 2):
        sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "voices" / "onnx"))
        from reference import Package  # numpy + ONNX Runtime only

        index = json.loads((packages / "index.json").read_text(encoding="utf-8"))
        self.package = Package(packages / index["languages"][LANGUAGE_DIRS[code]]["folder"],
                               packages / index["mimi"]["folder"], threads=threads)

    def get_state_for_audio_prompt(self, name: object) -> str:
        return str(name)

    def generate_audio_stream(self, state: str, text: str, stop: Optional[threading.Event] = None, **_) -> Iterator:
        for chunk in self.package.speak(text, state):
            if stop is not None and stop.is_set():
                return
            yield chunk


def pocket_version() -> Optional[str]:
    """The installed pocket-tts' version (« 4.0.1 »)."""
    from importlib.metadata import PackageNotFoundError, version

    try:
        return version("pocket-tts")
    except PackageNotFoundError:
        return None


def _major(version: Optional[str]) -> Optional[int]:
    match = re.match(r"(\d+)", version or "")
    return int(match.group(1)) if match else None


def check_voices(voices_dir: Path, languages: List[str], version: Optional[str]) -> None:
    """A voice state only fits the weights it was computed with: says so when the voices' folder (its voices.json
    `engine`) and the running Pocket differ (an update stopped half way: `bipagents update` again)."""
    for code in languages:
        try:
            made = json.loads((voices_dir / LANGUAGE_DIRS[code] / "voices.json").read_text(encoding="utf-8")).get("engine")
        except (OSError, ValueError):
            continue
        if _major(str(made).split()[-1] if made else None) != _major(version):
            log.warning("%s voices were made with %s but pocket-tts %s runs: run `bipagents update`",
                        LANGUAGE_DIRS[code], made, version)


def onnx_version(packages: Path) -> Optional[str]:
    """The packages' Pocket version, the lowest when they differ (« pocket-tts 4.0.1 » in each manifest)."""
    try:
        index = json.loads((packages / "index.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    found = []
    for entry in index.get("languages", {}).values():
        try:
            manifest = json.loads((packages / str(entry.get("folder", "")) / "manifest.json").read_text(encoding="utf-8"))
        except (OSError, ValueError, AttributeError):
            continue
        version = str(manifest.get("engine", "")).split()[-1:]
        if version and version[0][:1].isdigit():
            found.append(version[0])
    return min(found, key=lambda v: tuple(int(p) if p.isdigit() else 0 for p in v.split("."))) if found else None


def onnx_voices(packages: Path) -> Dict[str, list[str]]:
    """The Bips of each language whose package is there, by language code."""
    try:
        index = json.loads((packages / "index.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    found: Dict[str, list[str]] = {}
    for code, name in LANGUAGE_DIRS.items():
        entry = index.get("languages", {}).get(name)
        manifest = packages / str((entry or {}).get("folder", "")) / "manifest.json"
        if entry and manifest.is_file():
            found[code] = sorted(json.loads(manifest.read_text(encoding="utf-8")).get("voices", {}))
    return found


def release_memory() -> None:
    """Hands freed memory back to the system (glibc keeps it otherwise)."""
    import gc

    gc.collect()
    try:
        import ctypes

        ctypes.CDLL("libc.so.6").malloc_trim(0)
    except (OSError, AttributeError):
        pass  # not glibc (macOS): the allocator returns large blocks by itself


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
            engine.split(voice)   # the voice exists; its model loads in the generation thread
        except VoiceNotFound:
            raise HTTPException(404, f"unknown voice: {voice}") from None

    @app.get("/health")
    async def health() -> dict:
        return {"status": "ok", "engine": engine.version, "languages": list(engine.languages),
                "loaded": sorted(engine.models), "voices": engine.available_voices()}

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
    parser.add_argument("--keep", default="", help="languages kept loaded (default: the first of --languages; none: none)")
    parser.add_argument("--idle-unload", type=float, default=900,
                        help="seconds without speaking before another language's model is unloaded (0: never)")
    parser.add_argument("--onnx", type=Path, default=None,
                        help="the phones' voice packages (index.json and its folders) instead of Pocket's models")
    parser.add_argument("--threads", type=int, default=0,
                        help="CPU threads for generation (default: PyTorch's choice); 2 leaves room for Hermes on a 4 vCPU VPS")
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s: %(message)s")
    languages = [code.strip() for code in args.languages.split(",") if code.strip()]
    unknown = [code for code in languages if code not in LANGUAGE_DIRS]
    if unknown:
        parser.error(f"unknown language(s): {', '.join(unknown)} (use {', '.join(LANGUAGE_DIRS)})")
    keep = {code.strip() for code in args.keep.split(",") if code.strip() and code.strip() != "none"}
    if args.keep.strip() != "none" and not keep:
        keep = set(languages[:1])
    if args.onnx is not None:
        engine = Engine.load_onnx(args.onnx.expanduser(), keep=keep, idle=args.idle_unload, threads=args.threads or 2)
    else:
        if args.threads > 0:
            import torch

            torch.set_num_threads(args.threads)
        engine = Engine.load(args.voices, languages, quantize=args.quantize, keep=keep & set(languages),
                             idle=args.idle_unload)
    engine.watch_idle()
    log.info("voices: %s (loaded: %s)", engine.available_voices(), sorted(engine.models))

    import uvicorn

    uvicorn.run(create_app(engine), host=args.host, port=args.port, log_level="warning")


if __name__ == "__main__":
    main()
