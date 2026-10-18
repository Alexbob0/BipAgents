"""Tools the bridge gives the agents, over MCP (``/v1/agent-tools/mcp``): ``create_audio`` reads a text aloud with the
voice the person chose for the agent in the app (its server voice for reading — Drama 3, Kokoro… — else its Bip).

The agent's own Hermes calls it (an MCP server entry ``bipagents`` in its profile, with a token of its own), not its
sandbox: nothing is opened to the containers, and the voice server stays on this machine's loopback. The text is cut
into paragraphs and short groups of sentences (a voice server skips or repeats lines in a long block), read with the
person's pronunciations, one piece after the other in the background queue, so a Live started meanwhile is not held
up. The file lands in the bridge's own folder, served to the app like any ``MEDIA:`` file under ``/bipagents/audio``.
"""
from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import os
import re
import secrets
import shutil
import time
import unicodedata
import wave
from datetime import datetime
from pathlib import Path
from typing import Any, AsyncIterator, Callable, Dict, List, Optional, Tuple

from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import JSONResponse, Response, StreamingResponse

from . import __version__, shares
from .config import AgentConfig, Config
from .logs import fields
from .tts import BACKGROUND, OUTPUT_RATE, Resampler, TtsError, mono
from .voices import relay_for, voice_for

log = logging.getLogger("bipbridge.agent_tools")

SERVER_NAME = "bipagents"          # the entry in the agents' `mcp_servers`
PATH = "/v1/agent-tools/mcp"
MEDIA_PREFIX = "/bipagents/audio"  # how the app asks for the files (`GET /v1/media?path=…`)
MAX_CHARS = 20000                  # about 25 minutes of audio
CHUNK_CHARS = 350
PAUSE_SENTENCES_MS = 150
PAUSE_PARAGRAPH_MS = 500
TOOL_TIMEOUT = 1800                # seconds Hermes waits for a call (its default is 300: too short for a podcast)
KEEPALIVE_SECONDS = 10.0           # an SSE comment while it reads, so the client's read timeout never fires

CREATE_AUDIO = {
    "name": "create_audio",
    "description": (
        "Turn a text into an audio file read aloud with your voice — the one the person chose for you in the "
        "BipAgents app — for example a podcast, a story or a long message to listen to. It takes about as long as the "
        "audio lasts. The result gives a MEDIA: line: put it alone on a line in your reply, and the person gets the "
        "audio in the app. Write the text as it should be heard: no markdown, no lists, no links."),
    "inputSchema": {
        "type": "object",
        "properties": {
            "text": {"type": "string", "description": "What to read aloud.", "maxLength": MAX_CHARS},
            "title": {"type": "string", "description": "A short title, used for the file name.", "maxLength": 80},
        },
        "required": ["text"],
    },
}


def _digest(token: str) -> str:
    return hashlib.sha256(token.encode()).hexdigest()


def store_path(config: Config) -> Path:
    return Path(config.outbox.db_path).parent / "agent-tools.json"


def audio_root(config: Config) -> Path:
    return Path(config.outbox.db_path).parent / "agent-audio"


def media_roots(config: Config) -> Dict[str, str]:
    """`[media] roots` plus the agents' audio files."""
    return {**config.media_roots, MEDIA_PREFIX: str(audio_root(config))}


def tools_url(config: Config) -> str:
    """The bridge as the agents' Hermes reaches it: this machine, unless the agents' gateways run elsewhere (`peer_url`,
    set for spaces in containers of their own)."""
    if config.peer_url and config.peer_url.endswith("/peer"):
        return config.peer_url[: -len("/peer")] + PATH
    return f"http://127.0.0.1:{config.port}{PATH}"


def load_tokens(config: Config) -> Dict[str, str]:
    try:
        data = json.loads(store_path(config).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    tokens = data.get("tokens") if isinstance(data, dict) else None
    return {str(k): str(v) for k, v in tokens.items()} if isinstance(tokens, dict) else {}


def save_tokens(config: Config, tokens: Dict[str, str]) -> None:
    path = store_path(config)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".part")
    tmp.write_text(json.dumps({"tokens": tokens}, indent=1) + "\n", encoding="utf-8")
    os.chmod(tmp, 0o600)
    os.replace(tmp, path)


def agent_for(config: Config, token: str) -> Optional[AgentConfig]:
    digest = _digest(token)
    for name, known in load_tokens(config).items():
        if secrets.compare_digest(known, digest):
            return config.agent(name)
    return None


async def register(config: Config, agent: AgentConfig, run: Callable[..., Any]) -> bool:
    """The agent's Hermes knows the bridge's tools: an MCP entry with its own token in its profile. Agents set up by
    hand are left as they are. Returns whether the profile was changed (a new conversation picks it up)."""
    folder = shares.profile_dir(config, agent)
    if folder is None or not folder.is_dir():
        return False
    url = tools_url(config)
    entry = (shares.profile_config(folder).get("mcp_servers") or {}).get(SERVER_NAME) or {}
    tokens = load_tokens(config)
    header = str(((entry.get("headers") or {}) if isinstance(entry, dict) else {}).get("Authorization", ""))
    current = header[len("Bearer "):] if header.startswith("Bearer ") else ""
    if (isinstance(entry, dict) and entry.get("url") == url and current and tokens.get(agent.name) == _digest(current)
            and str(entry.get("timeout")) == str(TOOL_TIMEOUT)):
        return False
    token = current if current and tokens.get(agent.name) == _digest(current) else secrets.token_urlsafe(32)
    env = {"HERMES_HOME": str(folder.parent.parent)}
    base = [config.hermes.bin, "-p", agent.name, "config", "set"]
    await run(base + [f"mcp_servers.{SERVER_NAME}.url", url], env)
    await run(base + [f"mcp_servers.{SERVER_NAME}.headers.Authorization", f"Bearer {token}"], env)
    await run(base + [f"mcp_servers.{SERVER_NAME}.timeout", str(TOOL_TIMEOUT)], env)
    tokens[agent.name] = _digest(token)
    save_tokens(config, tokens)
    log.info("agent tools registered", extra=fields(agent=agent.name))
    return True


async def register_all(config: Config, run: Callable[..., Any]) -> None:
    """At start: every agent this install manages gets the tools (agents made before them, or moved since)."""
    for agent in list(config.agents.values()):
        try:
            await register(config, agent, run)
        except (RuntimeError, OSError) as exc:
            log.warning("agent tools not registered", extra=fields(agent=agent.name, error=str(exc)[-200:]))


def forget(config: Config, name: str) -> None:
    """A deleted agent: its token goes."""
    tokens = load_tokens(config)
    if tokens.pop(name, None) is not None:
        save_tokens(config, tokens)


# --- the audio ---------------------------------------------------------------------------------------------------

_SENTENCE_END = re.compile(r"(?<=[.!?…])\s+")
_SOFT_BREAK = re.compile(r"(?<=[,;:])\s+")


def _pieces(text: str, limit: int) -> List[str]:
    """`text` in pieces of at most `limit` characters, cut between sentences, else at commas, else between words."""
    out: List[str] = []
    for unit in _SENTENCE_END.split(text):
        unit = unit.strip()
        if not unit:
            continue
        parts = [unit] if len(unit) <= limit else [p for p in _SOFT_BREAK.split(unit) if p.strip()]
        for part in parts:
            while len(part) > limit:
                cut = part.rfind(" ", 0, limit)
                cut = cut if cut > limit // 2 else limit
                out.append(part[:cut].strip())
                part = part[cut:].strip()
            if out and part and len(out[-1]) + 1 + len(part) <= limit:
                out[-1] = f"{out[-1]} {part}"
            elif part:
                out.append(part)
    return out


def split_for_speech(text: str, limit: int = CHUNK_CHARS) -> List[Tuple[str, int]]:
    """`(piece, pause after it in ms)`: paragraphs (blank lines) kept apart by a longer pause. `MEDIA:` lines go."""
    text = "\n".join(line for line in text.splitlines() if not line.strip().startswith("MEDIA:"))
    chunks: List[Tuple[str, int]] = []
    for paragraph in re.split(r"\n\s*\n", text):
        paragraph = " ".join(paragraph.split())
        if not paragraph:
            continue
        pieces = _pieces(paragraph, limit)
        chunks += [(piece, PAUSE_SENTENCES_MS) for piece in pieces[:-1]] + [(pieces[-1], PAUSE_PARAGRAPH_MS)]
    return chunks


def _silence(ms: int) -> bytes:
    return b"\x00\x00" * (OUTPUT_RATE * ms // 1000)


def _slug(title: str) -> str:
    ascii_title = unicodedata.normalize("NFKD", title).encode("ascii", "ignore").decode()
    slug = re.sub(r"[^a-z0-9]+", "-", ascii_title.lower()).strip("-")
    return slug[:40] or "audio"


async def _encode(wav_path: Path) -> Path:
    """An .m4a next to the WAV when ffmpeg is here (a tenth of the size), else the WAV itself."""
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        return wav_path
    target = wav_path.with_suffix(".m4a")
    proc = await asyncio.create_subprocess_exec(ffmpeg, "-nostdin", "-y", "-loglevel", "error", "-i", str(wav_path),
                                                "-c:a", "aac", "-b:a", "64k", str(target),
                                                stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.PIPE)
    _, err = await proc.communicate()
    if proc.returncode != 0 or not target.exists():
        log.warning("audio not encoded, kept as WAV", extra=fields(error=err.decode(errors="replace")[-200:]))
        target.unlink(missing_ok=True)
        return wav_path
    wav_path.unlink(missing_ok=True)
    return target


class AudioMaker:
    """One audio at a time per agent (a second request waits for the first)."""

    def __init__(self, config: Config, tts: Any):
        self.config = config
        self.tts = tts
        self._locks: Dict[str, asyncio.Lock] = {}

    async def create(self, agent: AgentConfig, text: str, title: str = "") -> Dict[str, Any]:
        text = (text or "").strip()
        if not text:
            raise ValueError("the text is empty")
        if len(text) > MAX_CHARS:
            raise ValueError(f"the text is too long ({len(text)} characters, at most {MAX_CHARS}): split it")
        chunks = split_for_speech(text)
        if not chunks:
            raise ValueError("nothing to read once MEDIA: lines are removed")
        voice = voice_for(self.config, agent, "reading")
        relay = relay_for(agent)
        lock = self._locks.setdefault(agent.name, asyncio.Lock())
        async with lock:
            started = time.monotonic()
            pcm = bytearray()
            for piece, pause in chunks:
                audio = await self.tts.synthesize(piece, voice, "pcm16", BACKGROUND, max_chars=0, relay=relay)
                data = audio.data if audio.channels == 1 else mono(audio.data, audio.channels)
                if audio.sample_rate != OUTPUT_RATE:
                    data = Resampler(audio.sample_rate, OUTPUT_RATE).feed(data, last=True)
                pcm += data + _silence(pause)
            folder = audio_root(self.config) / agent.name
            folder.mkdir(parents=True, exist_ok=True)
            name = f"{datetime.now().strftime('%Y%m%d-%H%M%S')}-{_slug(title or text[:40])}"
            wav_path = folder / f"{name}.wav"
            with wave.open(str(wav_path), "wb") as out:
                out.setnchannels(1)
                out.setsampwidth(2)
                out.setframerate(OUTPUT_RATE)
                out.writeframes(bytes(pcm))
            final = await _encode(wav_path)
        seconds = len(pcm) / 2 / OUTPUT_RATE
        log.info("agent audio made", extra=fields(agent=agent.name, chars=len(text), pieces=len(chunks),
                                                  seconds=round(seconds), took=round(time.monotonic() - started)))
        return {"path": f"{MEDIA_PREFIX}/{agent.name}/{final.name}", "seconds": seconds}


def purge_audio(config: Config, retention_days: int, now: Optional[float] = None) -> int:
    root = audio_root(config)
    if not root.is_dir():
        return 0
    cutoff = (now or time.time()) - retention_days * 86400
    removed = 0
    for path in root.glob("*/*"):
        try:
            if path.is_file() and not path.is_symlink() and path.stat().st_mtime < cutoff:
                path.unlink()
                removed += 1
        except OSError:
            continue
    return removed


# --- MCP (streamable HTTP, JSON-RPC) ------------------------------------------------------------------------------

def _result(request_id: Any, result: Dict[str, Any]) -> Dict[str, Any]:
    return {"jsonrpc": "2.0", "id": request_id, "result": result}


def _error(request_id: Any, code: int, message: str) -> Dict[str, Any]:
    return {"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message}}


def _tool_text(text: str, error: bool = False) -> Dict[str, Any]:
    return {"content": [{"type": "text", "text": text}], "isError": error}


def _duration(seconds: float) -> str:
    minutes, rest = divmod(int(round(seconds)), 60)
    return f"{minutes} min {rest:02d} s" if minutes else f"{rest} s"


async def call_tool(maker: AudioMaker, agent: AgentConfig, name: str, arguments: Dict[str, Any]) -> Dict[str, Any]:
    if name != CREATE_AUDIO["name"]:
        return _tool_text(f"unknown tool: {name}", error=True)
    try:
        made = await maker.create(agent, str(arguments.get("text") or ""), str(arguments.get("title") or ""))
    except ValueError as exc:
        return _tool_text(str(exc), error=True)
    except TtsError as exc:
        log.warning("agent audio failed", extra=fields(agent=agent.name, error=str(exc)))
        return _tool_text(f"the voice failed ({exc}); try again later", error=True)
    return _tool_text(f"Audio ready ({_duration(made['seconds'])}). To give it to the person, put this line alone at "
                      f"the end of your reply:\nMEDIA:{made['path']}")


def agent_tools_router() -> APIRouter:
    router = APIRouter()

    def _agent(request: Request) -> AgentConfig:
        token = request.headers.get("authorization", "").removeprefix("Bearer ").strip()
        agent = agent_for(request.app.state.services.config, token) if token else None
        if agent is None:
            raise HTTPException(status_code=401, detail="unknown agent")
        return agent

    @router.post(PATH)
    async def mcp(request: Request) -> Response:
        agent = _agent(request)
        services = request.app.state.services
        try:
            message = await request.json()
        except ValueError:
            return JSONResponse(_error(None, -32700, "parse error"), status_code=400)
        if not isinstance(message, dict) or message.get("jsonrpc") != "2.0":
            return JSONResponse(_error(None, -32600, "invalid request"), status_code=400)
        method, request_id, params = message.get("method"), message.get("id"), message.get("params") or {}
        if request_id is None:   # a notification (`notifications/initialized`…) or a response: nothing to answer
            return Response(status_code=202)
        if method == "initialize":
            return JSONResponse(_result(request_id, {
                "protocolVersion": str(params.get("protocolVersion") or "2025-06-18"),
                "capabilities": {"tools": {"listChanged": False}},
                "serverInfo": {"name": SERVER_NAME, "version": __version__},
                "instructions": "BipAgents: create_audio reads a text aloud with your voice, for the person's app."}))
        if method == "ping":
            return JSONResponse(_result(request_id, {}))
        if method == "tools/list":
            return JSONResponse(_result(request_id, {"tools": [CREATE_AUDIO]}))
        if method != "tools/call":
            return JSONResponse(_error(request_id, -32601, f"method not found: {method}"))
        name = str(params.get("name") or "")
        arguments = params.get("arguments") if isinstance(params.get("arguments"), dict) else {}
        work = asyncio.create_task(call_tool(services.audio_maker, agent, name, arguments))
        if "text/event-stream" not in request.headers.get("accept", ""):
            return JSONResponse(_result(request_id, await work))

        async def events() -> AsyncIterator[bytes]:
            try:
                while True:
                    done, _ = await asyncio.wait({work}, timeout=KEEPALIVE_SECONDS)
                    if done:
                        break
                    yield b": reading aloud\n\n"
                payload = json.dumps(_result(request_id, work.result()))
                yield f"event: message\ndata: {payload}\n\n".encode()
            finally:
                if not work.done():
                    work.cancel()

        return StreamingResponse(events(), media_type="text/event-stream", headers={"Cache-Control": "no-cache"})

    @router.get(PATH)
    async def no_stream(request: Request) -> Response:
        _agent(request)
        return Response(status_code=405, headers={"Allow": "POST"})   # no server-initiated stream

    @router.delete(PATH)
    async def end_session(request: Request) -> Response:
        _agent(request)
        return Response(status_code=200)

    return router
