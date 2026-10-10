"""Where the model is: a cloud API, a machine of the local network, or this machine. Most of the time it runs
elsewhere than the agents. Finds the OpenAI-compatible servers of the house, lists their models, and checks that a
model answers and whether it reads images (Hermes then sends photos to it as they are)."""
from __future__ import annotations

import asyncio
import base64
import ipaddress
import time
from dataclasses import dataclass, field
from typing import Dict, Iterable, List, Optional

import httpx

# Usual ports of local model servers: Ollama, LM Studio, vLLM / SGLang, llama.cpp, and other OpenAI-compatible ones.
LAN_PORTS = {11434: "Ollama", 1234: "LM Studio", 8000: "vLLM", 8080: "llama.cpp", 8888: "OpenAI-compatible",
             5000: "OpenAI-compatible", 30000: "SGLang"}


@dataclass
class Provider:
    key: str
    name: str
    base_url: str
    needs_key: bool = True
    key_hint: str = ""


CLOUD = [
    Provider("openrouter", "OpenRouter (des centaines de modèles)", "https://openrouter.ai/api/v1",
             key_hint="https://openrouter.ai/keys"),
    Provider("openai", "OpenAI", "https://api.openai.com/v1", key_hint="https://platform.openai.com/api-keys"),
    Provider("anthropic", "Anthropic (Claude)", "https://api.anthropic.com/v1",
             key_hint="https://console.anthropic.com/settings/keys"),
    Provider("mistral", "Mistral", "https://api.mistral.ai/v1", key_hint="https://console.mistral.ai/api-keys"),
]


@dataclass
class Server:
    """An OpenAI-compatible server that answered `GET /v1/models`."""
    base_url: str            # ends with /v1
    kind: str
    models: List[str] = field(default_factory=list)


@dataclass
class ModelCheck:
    ok: bool
    seconds: float = 0.0
    vision: Optional[bool] = None
    error: Optional[str] = None


def candidate_hosts(addresses: Iterable[str]) -> List[str]:
    """Every host of this machine's /24 networks, this machine included (a model may run here too)."""
    hosts = ["127.0.0.1"]
    for address in addresses:
        network = ipaddress.ip_network(f"{address}/24", strict=False)
        hosts += [str(h) for h in network.hosts() if str(h) not in hosts]
    return hosts


async def _probe(client: httpx.AsyncClient, host: str, port: int) -> Optional[Server]:
    base = f"http://{host}:{port}/v1"
    try:
        resp = await client.get(f"{base}/models")
        data = resp.json()
    except (httpx.HTTPError, ValueError):
        return None
    if resp.status_code != 200 or not isinstance(data, dict) or not isinstance(data.get("data"), list):
        return None
    names = [m.get("id") for m in data["data"] if isinstance(m, dict) and m.get("id")]
    return Server(base_url=base, kind=LAN_PORTS.get(port, "OpenAI-compatible"), models=names)


async def discover(addresses: Iterable[str], ports: Iterable[int] = tuple(LAN_PORTS), timeout: float = 1.5,
                   concurrency: int = 96, transport: Optional[httpx.AsyncBaseTransport] = None) -> List[Server]:
    """Model servers answering on the local network (a few seconds for a /24). A busy server can miss one round:
    an empty result is tried once more, slower."""
    found = await _discover_once(addresses, ports, timeout, concurrency, transport)
    if not found:
        found = await _discover_once(addresses, ports, timeout * 2, concurrency // 2, transport)
    return found


async def _discover_once(addresses: Iterable[str], ports: Iterable[int], timeout: float, concurrency: int,
                         transport: Optional[httpx.AsyncBaseTransport]) -> List[Server]:
    semaphore = asyncio.Semaphore(concurrency)
    async with httpx.AsyncClient(timeout=timeout, transport=transport) as client:
        async def guarded(host: str, port: int) -> Optional[Server]:
            async with semaphore:
                return await _probe(client, host, port)
        found = await asyncio.gather(*(guarded(h, p) for h in candidate_hosts(addresses) for p in ports))
    return [s for s in found if s is not None]


async def list_models(base_url: str, api_key: Optional[str] = None,
                      transport: Optional[httpx.AsyncBaseTransport] = None) -> List[str]:
    headers = {"Authorization": f"Bearer {api_key}"} if api_key else {}
    async with httpx.AsyncClient(timeout=15, transport=transport) as client:
        resp = await client.get(f"{base_url.rstrip('/')}/models", headers=headers)
        resp.raise_for_status()
        return [m["id"] for m in resp.json().get("data", []) if isinstance(m, dict) and m.get("id")]


def _red_square(size: int = 32) -> str:
    """A small red PNG, built here (zlib + CRCs): enough to tell a model that reads images from one that refuses them."""
    import struct
    import zlib

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    rows = b"".join(b"\x00" + b"\xff\x00\x00" * size for _ in range(size))
    png = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
           + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
    return base64.b64encode(png).decode()


_RED_SQUARE = _red_square()


async def check(base_url: str, model: str, api_key: Optional[str] = None, *,
                transport: Optional[httpx.AsyncBaseTransport] = None) -> ModelCheck:
    """A one-word reply, then the same with a tiny image: does it answer, how fast, does it see images?"""
    headers = {"Authorization": f"Bearer {api_key}"} if api_key else {}
    url = f"{base_url.rstrip('/')}/chat/completions"
    text = {"model": model, "max_tokens": 8, "messages": [{"role": "user", "content": "Réponds juste : OK"}]}
    image = {"model": model, "max_tokens": 8, "messages": [{"role": "user", "content": [
        {"type": "text", "text": "Quelle couleur ? Un mot."},
        {"type": "image_url", "image_url": {"url": f"data:image/png;base64,{_RED_SQUARE}"}}]}]}
    async with httpx.AsyncClient(timeout=120, transport=transport) as client:
        start = time.monotonic()
        try:
            resp = await client.post(url, json=text, headers=headers)
        except httpx.HTTPError as exc:
            return ModelCheck(ok=False, error=type(exc).__name__)
        seconds = time.monotonic() - start
        if resp.status_code != 200:
            return ModelCheck(ok=False, seconds=seconds, error=f"HTTP {resp.status_code}")
        try:
            sees = await client.post(url, json=image, headers=headers)
            vision: Optional[bool] = sees.status_code == 200
        except httpx.HTTPError:
            vision = None
    return ModelCheck(ok=True, seconds=seconds, vision=vision)


def summary(servers: List[Server]) -> Dict[str, List[str]]:
    return {s.base_url: s.models for s in servers}
