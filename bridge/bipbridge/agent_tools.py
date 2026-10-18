"""Tools the bridge gives the agents, over MCP (``/v1/agent-tools/mcp``): ``create_audio`` reads a text aloud with the
voice the person chose for the agent in the app (its server voice for reading — Drama 3, Kokoro… — else its Bip);
``create_podcast`` makes a conversation between hosts, each with a voice of their own (the person's voice model when
it reads, else Bips); ``list_voices`` lists the voices the agent may give them.

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

import httpx

from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import JSONResponse, Response, StreamingResponse

from . import __version__, shares
from .config import AgentConfig, Config
from .logs import fields
from .tts import BACKGROUND, OUTPUT_RATE, Resampler, TtsError, mono, voice_language
from .voices import relay_for, voice_for

log = logging.getLogger("bipbridge.agent_tools")

SERVER_NAME = "bipagents"          # the entry in the agents' `mcp_servers`
PATH = "/v1/agent-tools/mcp"
MEDIA_PREFIX = "/bipagents/audio"  # how the app asks for the files (`GET /v1/media?path=…`)
MAX_CHARS = 20000                  # about 25 minutes of audio
CHUNK_CHARS = 350
MAX_TURNS = 400
MAX_SPEAKERS = 4
PAUSE_TURN_MS = 350
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


CREATE_PODCAST = {
    "name": "create_podcast",
    "description": (
        "Make a podcast where two or more hosts talk with each other, each with their own voice — like a radio show: "
        "the first host speaks with your voice (or the best one of the person's voice model), the others get "
        "different voices (or the ones you pick with list_voices). Write a real conversation: short turns, reactions, "
        "questions, a little humour, each host with a name and a personality; no markdown, no lists, no links. It "
        "takes about as long as the audio lasts. The result gives a MEDIA: line: put it alone on a line in your reply."),
    "inputSchema": {
        "type": "object",
        "properties": {
            "title": {"type": "string", "description": "A short title, used for the file name.", "maxLength": 80},
            "turns": {
                "type": "array", "maxItems": MAX_TURNS,
                "description": "The conversation, in order: who speaks and what they say.",
                "items": {"type": "object", "required": ["speaker", "text"], "properties": {
                    "speaker": {"type": "string", "description": "The host's name, the same in every turn."},
                    "text": {"type": "string", "description": "What they say in this turn."}}},
            },
            "voices": {"type": "object", "additionalProperties": {"type": "string"},
                       "description": "Optional: a voice per host name, from list_voices."},
        },
        "required": ["turns"],
    },
}

LIST_VOICES = {
    "name": "list_voices",
    "description": "The voices create_podcast may give its hosts: yours first, then the person's voice model's, then "
                   "the Bips (the app's mascots), in your language.",
    "inputSchema": {"type": "object", "properties": {}},
}

TOOLS = [CREATE_AUDIO, CREATE_PODCAST, LIST_VOICES]


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
        self._pocket_voices: Optional[Tuple[float, Dict[str, List[str]]]] = None

    async def pocket_voices(self) -> Dict[str, List[str]]:
        """The Bips Pocket serves, by language (its `/health`), kept a few minutes."""
        pocket = self.tts.pocket
        if pocket is None:
            return {}
        now = time.monotonic()
        if self._pocket_voices and now - self._pocket_voices[0] < 300:
            return self._pocket_voices[1]
        try:
            resp = await self.tts.client.get(f"{pocket.base_url}/health", timeout=5.0)
            found = resp.json().get("voices") if resp.status_code == 200 else None
        except (httpx.HTTPError, ValueError, AttributeError):
            found = None
        voices = {str(k): [str(v) for v in names] for k, names in found.items()} if isinstance(found, dict) else {}
        self._pocket_voices = (now, voices)
        return voices

    async def voices(self, agent: AgentConfig) -> List[Dict[str, str]]:
        """The voices an agent may give a podcast's hosts: its own first, then its voice model's in its language,
        then the Bips in its language."""
        own = voice_for(self.config, agent, "reading")
        language = voice_language(own)
        found: List[Dict[str, str]] = [{"voice": own, "kind": "your voice"}]
        if self.config.voice_server is not None:
            for name in await self.tts.server_voices():
                prefix = name.split("-", 1)[0]
                if len(prefix) == 2 and prefix != language:
                    continue   # « en-naturelle-s47 »: another language's voice
                found.append({"voice": f"server:{language}/{name}", "kind": "voice model"})
        bips = (await self.pocket_voices()).get(language, [])
        for name in bips:
            found.append({"voice": f"pocket:{name}" if language == "fr" else f"pocket:{language}/{name}", "kind": "Bip"})
        seen: set = set()
        unique = []
        for entry in found:
            if entry["voice"] not in seen:
                seen.add(entry["voice"])
                unique.append(entry)
        return unique

    async def cast(self, agent: AgentConfig, speakers: List[str], wanted: Dict[str, str]) -> Dict[str, str]:
        """A voice per speaker, in order of appearance: the one asked for (from `voices`), else voices of the person's
        voice model when it reads (natural voices make a better podcast than Bips), the agent's own leading, then
        different ones of the same family first (« naturelle » with « naturelle »); without a voice model, Bips."""
        available = await self.voices(agent)
        allowed = {entry["voice"] for entry in available}
        for speaker, voice in wanted.items():
            if voice not in allowed:
                raise ValueError(f"unknown voice for {speaker}: {voice} (see list_voices)")
        own = available[0]["voice"]
        server = self.config.voice_server
        model = [e["voice"] for e in available if e["voice"].startswith("server:")]
        if model and server is not None and server.uses.get("reading", True):
            pool = model
        else:
            pool = [e["voice"] for e in available if e["voice"].split(":")[0] == own.split(":")[0]]
        if own not in pool:   # the agent speaks with a Bip: a natural voice of the model leads, not a mascot's
            pool = sorted(pool, key=lambda v: 0 if "natur" in v else 1)
        lead = own if own in pool or not pool else pool[0]
        name = lead.rsplit("/", 1)[-1]
        family = name.split("-")[1] if name.count("-") >= 2 else ""
        ranked = [v for v in pool if family and f"-{family}-" in v] + pool + [e["voice"] for e in available]
        cast: Dict[str, str] = {}
        used = set(wanted.values())
        for speaker in speakers:
            if speaker in wanted:
                cast[speaker] = wanted[speaker]
                continue
            choice = lead if lead not in used else next((v for v in ranked if v not in used), lead)
            cast[speaker] = choice
            used.add(choice)
        return cast

    async def _speak(self, text: str, voice: str, relay: Optional[str]) -> bytearray:
        """PCM16 at 24 kHz, the pieces of `text` read one after the other (no pause after the last)."""
        chunks = split_for_speech(text)
        pcm = bytearray()
        for index, (piece, pause) in enumerate(chunks):
            audio = await self.tts.synthesize(piece, voice, "pcm16", BACKGROUND, max_chars=0, relay=relay)
            data = audio.data if audio.channels == 1 else mono(audio.data, audio.channels)
            if audio.sample_rate != OUTPUT_RATE:
                data = Resampler(audio.sample_rate, OUTPUT_RATE).feed(data, last=True)
            pcm += data + (_silence(pause) if index < len(chunks) - 1 else b"")
        return pcm

    async def _save(self, agent: AgentConfig, pcm: bytes, title: str) -> Dict[str, Any]:
        folder = audio_root(self.config) / agent.name
        folder.mkdir(parents=True, exist_ok=True)
        wav_path = folder / f"{datetime.now().strftime('%Y%m%d-%H%M%S')}-{_slug(title)}.wav"
        with wave.open(str(wav_path), "wb") as out:
            out.setnchannels(1)
            out.setsampwidth(2)
            out.setframerate(OUTPUT_RATE)
            out.writeframes(bytes(pcm))
        final = await _encode(wav_path)
        return {"path": f"{MEDIA_PREFIX}/{agent.name}/{final.name}", "seconds": len(pcm) / 2 / OUTPUT_RATE}

    async def create(self, agent: AgentConfig, text: str, title: str = "") -> Dict[str, Any]:
        text = (text or "").strip()
        if not text:
            raise ValueError("the text is empty")
        if len(text) > MAX_CHARS:
            raise ValueError(f"the text is too long ({len(text)} characters, at most {MAX_CHARS}): split it")
        if not split_for_speech(text):
            raise ValueError("nothing to read once MEDIA: lines are removed")
        voice = voice_for(self.config, agent, "reading")
        async with self._locks.setdefault(agent.name, asyncio.Lock()):
            started = time.monotonic()
            made = await self._save(agent, await self._speak(text, voice, relay_for(agent)), title or text[:40])
        log.info("agent audio made", extra=fields(agent=agent.name, chars=len(text), seconds=round(made["seconds"]),
                                                  took=round(time.monotonic() - started)))
        return made

    async def podcast(self, agent: AgentConfig, turns: List[Dict[str, Any]], title: str = "",
                      voices: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        clean: List[Tuple[str, str]] = []
        for turn in turns:
            speaker = " ".join(str(turn.get("speaker") or "").split())[:40] if isinstance(turn, dict) else ""
            text = str(turn.get("text") or "").strip() if isinstance(turn, dict) else ""
            if speaker and text and split_for_speech(text):
                clean.append((speaker, text))
        if not clean:
            raise ValueError("no turn to read: each turn needs a speaker and a text")
        if len(clean) > MAX_TURNS:
            raise ValueError(f"too many turns ({len(clean)}, at most {MAX_TURNS}): make two episodes")
        total = sum(len(text) for _, text in clean)
        if total > MAX_CHARS:
            raise ValueError(f"the podcast is too long ({total} characters, at most {MAX_CHARS}): make two episodes")
        speakers = list(dict.fromkeys(speaker for speaker, _ in clean))
        if len(speakers) > MAX_SPEAKERS:
            raise ValueError(f"too many speakers ({len(speakers)}, at most {MAX_SPEAKERS})")
        cast = await self.cast(agent, speakers, {str(k): str(v) for k, v in (voices or {}).items() if k in speakers})
        relay = relay_for(agent)
        async with self._locks.setdefault(agent.name, asyncio.Lock()):
            started = time.monotonic()
            pcm = bytearray()
            for index, (speaker, text) in enumerate(clean):
                pcm += await self._speak(text, cast[speaker], relay if cast[speaker] == cast[speakers[0]] else None)
                if index < len(clean) - 1:
                    pcm += _silence(PAUSE_TURN_MS)
            made = await self._save(agent, pcm, title or "podcast")
        log.info("agent podcast made", extra=fields(agent=agent.name, turns=len(clean), speakers=len(speakers),
                                                    seconds=round(made["seconds"]), took=round(time.monotonic() - started)))
        return {**made, "cast": cast}


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
    if name == LIST_VOICES["name"]:
        lines = [f"{entry['voice']} ({entry['kind']})" for entry in await maker.voices(agent)]
        return _tool_text("\n".join(lines))
    if name not in (CREATE_AUDIO["name"], CREATE_PODCAST["name"]):
        return _tool_text(f"unknown tool: {name}", error=True)
    title = str(arguments.get("title") or "")
    try:
        if name == CREATE_PODCAST["name"]:
            turns = arguments.get("turns") if isinstance(arguments.get("turns"), list) else []
            voices = arguments.get("voices") if isinstance(arguments.get("voices"), dict) else {}
            made = await maker.podcast(agent, turns, title, voices)
        else:
            made = await maker.create(agent, str(arguments.get("text") or ""), title)
    except ValueError as exc:
        return _tool_text(str(exc), error=True)
    except TtsError as exc:
        log.warning("agent audio failed", extra=fields(agent=agent.name, error=str(exc)))
        return _tool_text(f"the voice failed ({exc}); try again later", error=True)
    cast = "".join(f"\n- {speaker}: {voice}" for speaker, voice in (made.get("cast") or {}).items())
    return _tool_text(f"Audio ready ({_duration(made['seconds'])}).{chr(10) + 'Voices:' + cast if cast else ''}\n"
                      f"To give it to the person, put this line alone at the end of your reply:\nMEDIA:{made['path']}")


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
                "instructions": "BipAgents: create_audio reads a text aloud with your voice; create_podcast makes a "
                                "conversation between hosts with different voices; both give a MEDIA: line for your reply."}))
        if method == "ping":
            return JSONResponse(_result(request_id, {}))
        if method == "tools/list":
            return JSONResponse(_result(request_id, {"tools": TOOLS}))
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
