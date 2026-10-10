"""Messages between agents, through the bridge, only where the person allowed them.

Hermes' Bot Mode lets an agent message another one (`message_agent`) on a « peer »: an address and a key in
its config (`bot_peers`). Giving an agent another agent's key would let it do anything with that agent. So
the bridge is the only peer: each gateway gets a token of its own, which says who is calling and nothing more,
and the bridge forwards to the target with the target's key, but only along a link the person turned on in
the app (« Qui peut écrire à qui »). The target answers in its permanent « Bot Chat », as with any peer.

Hermes delivers these messages as the gateway (`hermes -p default peer dm`): the token says which gateway
calls, and the message's signature (« Message from … (@agent): ») which of its agents. An agent in its own
space is alone in its gateway, so it is known for sure; in the shared hub, an agent could sign as another hub
agent (same person, nothing secret there). Agents of the hub can also message each other directly.
"""
from __future__ import annotations

import hashlib
import json
import re
import logging
import os
import secrets
from pathlib import Path
from typing import Any, AsyncIterator, Callable, Dict, List, Optional, Set, Tuple

import httpx
from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import StreamingResponse

from .config import AgentConfig, Config
from .logs import fields

log = logging.getLogger("bipbridge.peers")

PEER_NAME = "bip"            # what agents see in their roster: message_agent target "bip/<agent>"
_HOP = {"host", "content-length", "connection", "keep-alive", "transfer-encoding", "authorization", "upgrade"}


def _digest(token: str) -> str:
    return hashlib.sha256(token.encode()).hexdigest()


class Links:
    """`<data>/peers.json`: the allowed links (from → to) and each sending agent's token digest."""

    def __init__(self, path: str):
        self.path = Path(path)

    def load(self) -> Dict[str, Any]:
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            data = {}
        return {"links": [tuple(x) for x in data.get("links", []) if len(x) == 2], "tokens": data.get("tokens", {})}

    def save(self, data: Dict[str, Any]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_name(self.path.name + ".tmp")
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump({"links": [list(x) for x in sorted(data["links"])], "tokens": data["tokens"]}, fh)
        os.replace(tmp, self.path)

    def caller(self, token: str) -> Optional[str]:
        """The gateway (see `gateway_of`) this token was given to."""
        digest = _digest(token)
        return next((gw for gw, d in self.load()["tokens"].items() if secrets.compare_digest(d, digest)), None)

    def allowed(self, sender: str, target: str) -> bool:
        return (sender, target) in self.load()["links"]


def managed_home(config: Config, agent: AgentConfig) -> Optional[Path]:
    """The HERMES_HOME of the gateway serving an agent this bridge manages (None: set up by hand)."""
    if agent.space:
        return Path(agent.space)
    if config.hermes.enabled and agent.hermes_url.startswith(f"{config.hermes.hub_url}/p/"):
        return Path(config.hermes.home or Path.home() / ".hermes")
    return None


def gateway_of(config: Config, agent: AgentConfig) -> str:
    """Which gateway serves the agent: its HERMES_HOME, or « manual:<agent> » for one set up by hand."""
    home = managed_home(config, agent)
    return str(home) if home is not None else f"manual:{agent.name}"


SIGNATURE = re.compile(r"^Message from .{0,200}? \(@([A-Za-z0-9_-]{1,64})\):")


def sender_of(config: Config, gateway: str, body: bytes) -> Optional[str]:
    """The agent of `gateway` that signed this message (Hermes puts « Message from … (@agent): » first)."""
    try:
        data = json.loads(body or b"{}")
    except ValueError:
        return None
    text = str(data.get("message") or data.get("input") or "") if isinstance(data, dict) else ""
    found = SIGNATURE.match(text)
    agent = config.agent(found.group(1)) if found else None
    return agent.name if agent is not None and gateway_of(config, agent) == gateway else None


def set_env(path: Path, key: str, value: Optional[str]) -> None:
    lines = path.read_text(encoding="utf-8").splitlines() if path.exists() else []
    kept = [line for line in lines if line.split("=", 1)[0].strip() != key]
    if value is not None:
        kept.append(f"{key}={value}")
    path.write_text("\n".join(kept) + "\n", encoding="utf-8")
    path.chmod(0o600)


def peer_url(config: Config) -> str:
    return config.peer_url or f"http://127.0.0.1:{config.port}/peer"


async def apply(config: Config, links: Links, wanted: List[Tuple[str, str]], run: Callable[..., Any]) -> Dict[str, Any]:
    """The person's links. The gateway of each sender gets its token and the bridge as its peer; a gateway with no
    sender left loses its token. Returns the agents set up by hand that need their token from the administrator."""
    for sender, target in wanted:
        if sender == target or config.agent(sender) is None or config.agent(target) is None:
            raise ValueError(f"unknown link {sender} → {target}")
    data = links.load()
    sending = {gateway_of(config, config.agent(s)): config.agent(s) for s, _ in wanted}   # type: ignore[arg-type]
    manual: Dict[str, str] = {}
    key_env = f"HERMES_PEER_{PEER_NAME.upper()}_KEY"
    for gateway, agent in sorted(sending.items()):
        if gateway in data["tokens"]:
            continue
        if gateway.startswith("manual:"):
            manual[agent.name] = ""      # the administrator adds it (python -m bipbridge peer-token)
            continue
        token = secrets.token_urlsafe(32)
        env = {"HERMES_HOME": gateway}
        # The gateway's own (default) profile: Hermes reads the peer and its key there when it delivers.
        await run([config.hermes.bin, "config", "set", f"bot_peers.{PEER_NAME}.url", peer_url(config)], env)
        await run([config.hermes.bin, "config", "set", f"bot_peers.{PEER_NAME}.note",
                   "Tes autres agents, par BipAgents (cible : bip/<agent>)"], env)
        set_env(Path(gateway) / ".env", key_env, token)
        data["tokens"][gateway] = _digest(token)
    for gateway in [g for g in data["tokens"] if g not in sending]:
        if not gateway.startswith("manual:") and Path(gateway).is_dir():
            set_env(Path(gateway) / ".env", key_env, None)
        del data["tokens"][gateway]
    data["links"] = sorted(set(wanted))
    links.save(data)
    log.info("agent links set", extra=fields(links=len(data["links"]), gateways=len(sending)))
    return {"manual": manual}


def manual_token(config: Config, links: Links, name: str) -> str:
    """A new token for an agent set up by hand (its old one stops working). Printed once, never stored."""
    agent = config.agent(name)
    if agent is None:
        raise ValueError(f"unknown agent {name}")
    token = secrets.token_urlsafe(32)
    data = links.load()
    data["tokens"][gateway_of(config, agent)] = _digest(token)
    links.save(data)
    return token


def peer_router() -> APIRouter:
    """`/peer/p/<target>/…`: Hermes' peer protocol (its sessions and runs API), for the agents."""
    router = APIRouter()
    allowed_paths = ("api/sessions", "v1/runs", "v1/capabilities")

    @router.api_route("/peer/p/{target}/{path:path}", methods=["GET", "POST"])
    async def relay(target: str, path: str, request: Request) -> StreamingResponse:
        services = request.app.state.services
        config: Config = services.config
        token = request.headers.get("authorization", "").removeprefix("Bearer ").strip()
        gateway = services.links.caller(token) if token else None
        if gateway is None:
            raise HTTPException(status_code=401, detail="unknown agent")
        agent = config.agent(target)
        if agent is None or not path.startswith(allowed_paths):
            raise HTTPException(status_code=404, detail="not found")
        body = await request.body()
        links = services.links.load()["links"]
        mine = {a.name for a in config.agents.values() if gateway_of(config, a) == gateway}
        if request.method == "POST" and (path.endswith("/chat") or path == "v1/runs"):
            sender = sender_of(config, gateway, body) or (next(iter(mine)) if len(mine) == 1 else None)
            allowed = sender is not None and (sender, agent.name) in links
        else:   # finding or opening the target's Bot Chat: some agent of this gateway may write to it
            sender = None
            allowed = any((name, agent.name) in links for name in mine)
        if not allowed:
            log.info("agent message refused", extra=fields(sender=sender or gateway, target=agent.name))
            raise HTTPException(status_code=403, detail=f"not allowed to write to {agent.name} (ask the person)")
        headers = {k: v for k, v in request.headers.items() if k.lower() not in _HOP}
        headers["Authorization"] = f"Bearer {agent.hermes_key}"
        upstream_request = services.http.build_request(
            request.method, f"{agent.hermes_url}/{path}", params=request.query_params, headers=headers,
            content=body, timeout=httpx.Timeout(30.0, read=None))   # a turn can take minutes
        try:
            upstream = await services.http.send(upstream_request, stream=True)
        except httpx.HTTPError as exc:
            raise HTTPException(status_code=502, detail="agent unreachable") from exc
        if sender:
            log.info("agent message", extra=fields(sender=sender, target=agent.name))

        async def body() -> AsyncIterator[bytes]:
            try:
                async for chunk in upstream.aiter_bytes():
                    yield chunk
            finally:
                await upstream.aclose()

        response_headers = {k: v for k, v in upstream.headers.items()
                            if k.lower() not in _HOP | {"content-encoding"}}
        return StreamingResponse(body(), status_code=upstream.status_code, headers=response_headers)

    return router
