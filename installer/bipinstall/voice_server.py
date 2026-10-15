"""The person's own voice model: a server already running on this machine or the local network, with OpenAI's
`POST /v1/audio/speech` (Fish Audio Drama 3 on audio.cpp, Kokoro, Kyutai…). Nothing is installed here: it is found (or
its address given), and the bridge's `[voice_server]` says which uses go to it — the Live, the spoken replies, the
reading of messages. Pocket's Bips speak for the rest, and stand in when it is down.

`bipinstall voice` connects it to this account's bridge outside an install (run it in each account that should use it).
"""
from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Dict, Iterable, List, Optional, Sequence

import httpx

from . import hermes, models, ui

# Where voice servers usually listen: audio.cpp (as BipAgents ran Drama 3), Kokoro-FastAPI, Kyutai's server.
VOICE_PORTS = {8099: "audio.cpp", 8880: "Kokoro", 8097: "Kyutai"}
USES = ("live", "replies", "reading")
USE_QUESTIONS = {"live": "the Live (talking in real time)", "replies": "spoken replies to your voice notes",
                 "reading": "messages and reports read aloud"}


@dataclass
class VoiceServer:
    url: str                                   # without /v1
    voices: List[str] = field(default_factory=list)
    model: str = "tts-1"
    stream_rate: Optional[int] = None          # raw PCM it streams (audio.cpp: its model's rate)
    kind: str = "OpenAI-compatible"

    @property
    def label(self) -> str:
        return f"{self.kind} — {self.url} ({len(self.voices)} voice(s))"


def voice_names(data: object) -> List[str]:
    """`{"voices": ["a", …]}` (audio.cpp, Kokoro), `{"voices": [{"id": …}]}`, Kyutai's `{"aliases": {…}}`, a list."""
    if isinstance(data, dict):
        data = data.get("voices", data.get("aliases", data.get("data")))
    if isinstance(data, dict):
        data = list(data)
    if not isinstance(data, list):
        return []
    names = [item if isinstance(item, str) else (item.get("id") or item.get("name") or item.get("voice_id"))
             for item in data if isinstance(item, (str, dict))]
    return [str(name) for name in names if name]


async def describe(client: httpx.AsyncClient, url: str, kind: Optional[str] = None) -> Optional[VoiceServer]:
    """The voice server at `url`, if one answers there: its voices, and the model it expects."""
    url = url.rstrip("/").removesuffix("/v1")
    voices: List[str] = []
    for path in ("/v1/audio/voices", "/v1/voices"):
        try:
            resp = await client.get(url + path)
            voices = voice_names(resp.json()) if resp.status_code == 200 else []
        except (httpx.HTTPError, ValueError):
            continue
        if voices:
            break
    if not voices:
        return None
    model = "tts-1"
    try:
        resp = await client.get(url + "/v1/models")
        ids = [m.get("id") for m in resp.json().get("data", []) if isinstance(m, dict)] if resp.status_code == 200 else []
        model = next((i for i in ids if i), model)
    except (httpx.HTTPError, ValueError, AttributeError):
        pass
    drama = model.startswith("drama")
    return VoiceServer(url, voices, model, 44100 if drama else None,
                       "Fish Audio Drama 3" if drama else kind or "OpenAI-compatible")


async def discover(addresses: Iterable[str], ports: Dict[int, str] = VOICE_PORTS, timeout: float = 1.5,
                   concurrency: int = 96, transport: Optional[httpx.AsyncBaseTransport] = None) -> List[VoiceServer]:
    """Voice servers answering on this machine and its local network."""
    semaphore = asyncio.Semaphore(concurrency)
    async with httpx.AsyncClient(timeout=timeout, transport=transport) as client:
        async def one(host: str, port: int) -> Optional[VoiceServer]:
            async with semaphore:
                return await describe(client, f"http://{host}:{port}", ports[port])
        found = await asyncio.gather(*(one(h, p) for h in models.candidate_hosts(addresses) for p in ports))
    return [s for s in found if s is not None]


def check(url: str, transport: Optional[httpx.AsyncBaseTransport] = None) -> Optional[VoiceServer]:
    async def run() -> Optional[VoiceServer]:
        async with httpx.AsyncClient(timeout=5, transport=transport) as client:
            return await describe(client, url)
    return asyncio.run(run())


def toml_lines(server: VoiceServer, uses: Dict[str, bool]) -> List[str]:
    """The bridge's `[voice_server]` table."""
    q = lambda v: '"' + v.replace("\\", "\\\\").replace('"', '\\"') + '"'
    lines = ["[voice_server]", f"url = {q(server.url)}", f"model = {q(server.model)}"]
    if server.stream_rate:
        lines.append(f"stream_rate = {server.stream_rate}")
    lines += [f"{use} = {'true' if uses.get(use, True) else 'false'}" for use in USES]
    return lines


def ask_uses(ask: Callable[[str, str], str], yes: Callable[[str], bool]) -> Dict[str, bool]:
    """Which uses go to the voice server (the Bips speak for the others)."""
    return {use: yes(ask(f"  Your voice model for {USE_QUESTIONS[use]}? (y/n)", "y")) for use in USES}


def parse_uses(text: str) -> Dict[str, bool]:
    """`--voice-uses live,reading` -> those on, the others off."""
    chosen = {part.strip() for part in text.split(",") if part.strip()}
    unknown = chosen - set(USES)
    if unknown:
        raise ValueError(f"unknown uses: {', '.join(sorted(unknown))} (live, replies, reading)")
    return {use: use in chosen for use in USES}


def connect(config_path: Path, lines: Sequence[str], runner: hermes.Runner) -> bool:
    """This account's bridge uses the voice server: its `[voice_server]` table written (replacing an earlier one, or an
    earlier installer's `[drama]`)."""
    if not config_path.exists():
        return False
    kept, skipping = [], False
    for line in config_path.read_text(encoding="utf-8").splitlines():
        header = line.strip()
        if header.startswith("[") and header.endswith("]"):
            skipping = header in ("[voice_server]", "[drama]")
        if not skipping:
            kept.append(line)
    text = "\n".join(kept).rstrip("\n") + "\n\n" + "\n".join(lines) + "\n"
    runner.log.append(f"write [voice_server] in {config_path}")
    if not runner.dry:
        config_path.write_text(text, encoding="utf-8")
    return True


def choose(found: List[VoiceServer], ask: Callable[[str, str], str],
           choose_one: Callable[..., int], say: Callable[[str], None]) -> Optional[VoiceServer]:
    """The server the person picks among those found, or at the address they give; None: Pocket's Bips only.
    `choose_one` answers from 1."""
    options = [s.label for s in found] + ["Another address", "None: the Bips' voices only (Pocket)"]
    picked = choose_one("Your voice model", options, 1 if found else len(options)) - 1
    if picked < len(found):
        return found[picked]
    if picked == len(found):
        url = ask("  Its address (e.g. http://192.168.1.20:8880)", "").strip()
        server = check(url) if url else None
        if server is None and url:
            say(ui.fail(f"No voice server answers at {url} (GET /v1/audio/voices): the Bips' voices only."))
        return server
    return None


def main(argv: List[str]) -> int:
    """`bipinstall voice`: connects this account's bridge to a voice server already running on this machine or the
    local network (found, or `--url`), and says which uses go to it."""
    import argparse
    from .__main__ import ask, choose as choose_one, say, yes
    from .detect import detect
    parser = argparse.ArgumentParser(prog="bipinstall voice", description=main.__doc__)
    parser.add_argument("--url", help="the voice server's address (default: look for one on the local network)")
    parser.add_argument("--uses", help="what it speaks for, among live, replies, reading (default: asked)")
    parser.add_argument("--off", action="store_true", help="back to the Bips' voices only")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)
    runner = hermes.Runner(dry=args.dry_run)
    config = Path.home() / ".config" / "bipagents" / "bridge.toml"
    if not config.exists():
        print(ui.fail("No BipAgents bridge in this account: install BipAgents first (install.sh)."))
        return 1
    if args.off:
        connect(config, [], runner)
    else:
        if args.url:
            server = check(args.url)
            if server is None:
                print(ui.fail(f"No voice server answers at {args.url} (GET /v1/audio/voices)."))
                return 1
        else:
            print(ui.note("Looking for voice servers on this machine and the local network…"))
            server = choose(asyncio.run(discover(detect().lan_addresses)), ask, choose_one, say)
            if server is None:
                print(ui.note("Nothing changed: the Bips' voices only."))
                return 0
        uses = parse_uses(args.uses) if args.uses else ask_uses(ask, yes)
        connect(config, toml_lines(server, uses), runner)
        print(ui.done(f"Your bridge uses {server.label}."))
    from .family import bridge_restart
    runner.run(bridge_restart(), check=False)
    if not args.off:
        print("  In the app: each agent › Voice › « Your voice model ». What it speaks for: Settings › Voice.")
    if runner.dry:
        print("\nCommands it would run:")
        for line in runner.log:
            print(f"  $ {line}")
    return 0
