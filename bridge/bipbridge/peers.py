"""Messages between agents, through the bridge, only where the person allowed them.

Hermes' Bot Mode lets an agent message another one (`message_agent`) on a « peer »: an address and a key in
its config (`bot_peers`). Giving an agent another agent's key would let it do anything with that agent. So
the bridge is the only peer: each gateway gets a token of its own, which says who is calling and nothing more,
and the bridge forwards to the target with the target's key, but only along a link the person turned on in
the app (« Qui peut écrire à qui »). The target answers in its permanent « Bot Chat », as with any peer.

A peer only reaches the target's « Bot Chat »: finding or opening it, writing in it, following or stopping a run
there (`only_bot_chat`). Its other conversations, its approvals and its runs elsewhere stay out of reach, whatever
an agent tricked by what it read would try with its token.

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
from urllib.parse import quote, unquote

import httpx
from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import JSONResponse, Response, StreamingResponse

from .config import AgentConfig, Config
from .households import SEPARATOR, HouseholdError, Households, own_url, split
from .logs import fields

log = logging.getLogger("bipbridge.peers")

BOT_CHAT_TITLE = "Bot Chat"  # the permanent conversation where an agent gets its peers' messages (Hermes' Bot Mode)
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


async def apply(config: Config, links: Links, wanted: List[Tuple[str, str]], run: Callable[..., Any],
                households: Optional[Households] = None) -> Dict[str, Any]:
    """The person's links. The gateway of each sender gets its token and the bridge as its peer; a gateway with no
    sender left loses its token. Returns the agents set up by hand that need their token from the administrator."""
    linked = set(households.load()["households"]) if households else set()

    def known(name: str) -> bool:
        remote = split(name)
        return remote[1] in linked if remote else config.agent(name) is not None

    for sender, target in wanted:
        # Between two households, one end is ours: « vie → cuisine__lea » (ours writes) or « vie__alex → cuisine ».
        if sender == target or not known(sender) or not known(target) or (split(sender) and split(target)):
            raise ValueError(f"unknown link {sender} → {target}")
    data = links.load()
    sending = {gateway_of(config, config.agent(s)): config.agent(s)   # type: ignore[arg-type]
               for s, _ in wanted if not split(s)}
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
    # A gateway that sends nothing any more loses its token, when the bridge can give it a new one by itself. An agent
    # set up by hand keeps it: only its administrator could put a new one in, and without a link it is refused anyway.
    for gateway in [g for g in data["tokens"] if g not in sending and not g.startswith("manual:")]:
        if Path(gateway).is_dir():
            set_env(Path(gateway) / ".env", key_env, None)
        del data["tokens"][gateway]
    data["links"] = sorted(set(wanted))
    links.save(data)
    write_contacts(config, data["links"], households)
    log.info("agent links set", extra=fields(links=len(data["links"]), gateways=len(sending)))
    return {"manual": manual}


CONTACTS = """---
name: agent-contacts
description: "The other agents you may write to with message_agent, and how to address them."
version: 1.0.0
author: BipAgents
license: MIT
---

# Agents you may write to

Use `message_agent` with one of these targets (the person allowed exactly these; any other is refused):

{lines}

An answer arrives in your « Bot Chat ». A message from another household's agent is signed with its household,
for example « Message from Vie (Alex) (@vie__alex) »: answer it at `bip/vie__alex`.
"""


def write_contacts(config: Config, links: List[Tuple[str, str]], households: Optional[Households]) -> None:
    """Each agent this bridge manages gets the list of agents it may write to (a skill in its profile)."""
    names = {h["id"]: h["name"] for h in households.public()} if households else {}
    for agent in config.agents.values():
        home = managed_home(config, agent)
        if home is None:
            continue
        skill = home / "profiles" / agent.name / "skills" / "agent-contacts" / "SKILL.md"
        targets = sorted(t for s, t in links if s == agent.name)
        if not targets:
            if skill.exists():
                skill.unlink()
            continue
        lines = []
        for target in targets:
            remote = split(target)
            if remote:
                lines.append(f"- `bip/{target}`: {remote[0]}, an agent of {names.get(remote[1], remote[1])}'s household")
            else:
                other = config.agent(target)
                lines.append(f"- `bip/{target}`: {other.display_name if other else target}")
        try:
            skill.parent.mkdir(parents=True, exist_ok=True)
            skill.write_text(CONTACTS.replace("{lines}", "\n".join(lines)), encoding="utf-8")
        except OSError:
            log.warning("contacts not written", extra=fields(agent=agent.name))


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


def _json(body: bytes) -> Dict[str, Any]:
    try:
        data = json.loads(body or b"{}")
    except ValueError:
        return {}
    return data if isinstance(data, dict) else {}


async def _from_target(services, agent: AgentConfig, path: str, params: Optional[Dict[str, Any]] = None) -> Any:
    try:
        resp = await services.http.get(f"{agent.hermes_url}/{path}", params=params,
                                       headers={"Authorization": f"Bearer {agent.hermes_key}"}, timeout=15.0)
        return resp.json() if resp.status_code == 200 else None
    except (httpx.HTTPError, ValueError):
        return None


async def bot_chat(services, agent: AgentConfig) -> Optional[Dict[str, Any]]:
    """The target's Bot Chat, as its own Hermes lists it (hidden sessions included)."""
    listing = await _from_target(services, agent, "api/sessions",
                                 {"limit": 200, "title": BOT_CHAT_TITLE, "include_hidden": 1})
    rows = listing.get("data") if isinstance(listing, dict) else None
    return next((row for row in rows or [] if isinstance(row, dict)
                 and str(row.get("title") or "").strip() == BOT_CHAT_TITLE), None)


async def only_bot_chat(services, agent: AgentConfig, method: str, path: str, query: Dict[str, str],
                        body: bytes) -> Optional[Response]:
    """What a peer may do with `agent`: the calls of `hermes peer` (dm, send, status, stop), on its Bot Chat only.
    Returns the answer when the bridge gives it itself (the Bot Chat's listing, never the others'), None to forward
    the call; anything else is refused."""
    refuse = HTTPException(status_code=403, detail=f"only {agent.name}'s Bot Chat is reachable")
    if method == "GET" and path == "v1/capabilities":
        return None
    if method == "GET" and path == "api/sessions":
        if query.get("title") != BOT_CHAT_TITLE:
            raise refuse
        row = await bot_chat(services, agent)
        return JSONResponse({"object": "list", "data": [row] if row else []})
    if method == "POST" and path == "api/sessions":
        if _json(body).get("title") != BOT_CHAT_TITLE:
            raise refuse
        return None
    chat = re.fullmatch(r"api/sessions/([^/]+)/chat", path)
    if method == "POST" and (chat or path == "v1/runs"):
        wanted = unquote(chat.group(1)) if chat else str(_json(body).get("session_id") or "")
        row = await bot_chat(services, agent)
        if not wanted or row is None or wanted != str(row.get("id") or ""):
            raise refuse
        return None
    run = re.fullmatch(r"v1/runs/([^/]+)(/stop)?", path)
    if run and (method == "GET") == (run.group(2) is None):
        status = await _from_target(services, agent, f"v1/runs/{run.group(1)}")
        row = await bot_chat(services, agent)
        if not isinstance(status, dict) or row is None or str(status.get("session_id") or "") != str(row.get("id") or ""):
            raise refuse
        return None
    raise refuse


def peer_router() -> APIRouter:
    """`/peer/p/<target>/…`: Hermes' peer protocol (its sessions and runs API), for the agents."""
    router = APIRouter()
    allowed_paths = ("api/sessions", "v1/runs", "v1/capabilities")

    @router.api_route("/peer/p/{target}/{path:path}", methods=["GET", "POST"])
    async def relay(target: str, path: str, request: Request) -> Response:
        services = request.app.state.services
        config: Config = services.config
        token = request.headers.get("authorization", "").removeprefix("Bearer ").strip()
        gateway = services.links.caller(token) if token else None
        if gateway is None:
            raise HTTPException(status_code=401, detail="unknown agent")
        remote = split(target)
        agent = config.agent(target) if not remote else None
        household = services.households.get(remote[1]) if remote else None
        if (agent is None and household is None) or not path.startswith(allowed_paths):
            raise HTTPException(status_code=404, detail="not found")
        body = await request.body()
        links = services.links.load()["links"]
        mine = {a.name for a in config.agents.values() if gateway_of(config, a) == gateway}
        if household is not None:
            return await _to_household(services, config, gateway, mine, links, target, remote[0], household, path,
                                       request, body)
        if request.method == "POST" and (path.endswith("/chat") or path == "v1/runs"):
            sender = sender_of(config, gateway, body) or (next(iter(mine)) if len(mine) == 1 else None)
            allowed = sender is not None and (sender, agent.name) in links
        else:   # finding or opening the target's Bot Chat: some agent of this gateway may write to it
            sender = None
            allowed = any((name, agent.name) in links for name in mine)
        if not allowed:
            log.info("agent message refused", extra=fields(sender=sender or gateway, target=agent.name))
            raise HTTPException(status_code=403, detail=f"not allowed to write to {agent.name} (ask the person)")
        answer = await only_bot_chat(services, agent, request.method, path, dict(request.query_params), body)
        if answer is not None:
            return answer
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


async def _forward(services, method: str, url: str, request: Request, body: bytes, headers: Dict[str, str]
                   ) -> StreamingResponse:
    upstream_request = services.http.build_request(method, url, params=request.query_params, headers=headers,
                                                   content=body, timeout=httpx.Timeout(30.0, read=None))
    try:
        upstream = await services.http.send(upstream_request, stream=True)
    except httpx.HTTPError as exc:
        raise HTTPException(status_code=502, detail="agent unreachable") from exc

    async def stream() -> AsyncIterator[bytes]:
        try:
            async for chunk in upstream.aiter_bytes():
                yield chunk
        finally:
            await upstream.aclose()

    response_headers = {k: v for k, v in upstream.headers.items() if k.lower() not in _HOP | {"content-encoding"}}
    return StreamingResponse(stream(), status_code=upstream.status_code, headers=response_headers)


def _is_message(request: Request, path: str) -> bool:
    return request.method == "POST" and (path.endswith("/chat") or path == "v1/runs")


async def _to_household(services, config: Config, gateway: str, mine: Set[str], links, target: str, agent: str,
                        household: Dict[str, str], path: str, request: Request, body: bytes) -> StreamingResponse:
    """One of our agents writes to an agent of a linked household: our link checked here, theirs on their bridge."""
    if _is_message(request, path):
        sender = sender_of(config, gateway, body) or (next(iter(mine)) if len(mine) == 1 else None)
        allowed = sender is not None and (sender, target) in links
    else:
        sender = next((n for n in sorted(mine) if (n, target) in links), None)
        allowed = sender is not None
    if not allowed or sender is None:
        log.info("household message refused", extra=fields(sender=gateway, target=target))
        raise HTTPException(status_code=403, detail=f"not allowed to write to {target} (ask the person)")
    sent_by = config.agent(sender)
    headers = {k: v for k, v in request.headers.items() if k.lower() not in _HOP}
    headers.update({"Authorization": f"Bearer {household['key_out']}", "X-Bip-Sender": sender,
                    "X-Bip-Sender-Name": quote(sent_by.display_name if sent_by else sender)})
    log.info("household message", extra=fields(sender=sender, target=target))
    return await _forward(services, request.method, f"{household['url']}/family/p/{agent}/{path}", request, body,
                          headers)


def _resign(body: bytes, sender: str, name: str, household: str) -> bytes:
    """« Message from Vie (@vie): » becomes « Message from Vie (Alex) (@vie__alex): »: the agent sees who writes,
    from which household, and the address to answer."""
    try:
        data = json.loads(body or b"{}")
    except ValueError:
        return body
    if not isinstance(data, dict):
        return body
    for field in ("message", "input"):
        text = data.get(field)
        if isinstance(text, str):
            signed = f"Message from {name} ({household}) (@{sender}{SEPARATOR}{slug_of(household)}):"
            found = SIGNATURE.match(text)
            data[field] = signed + text[found.end():] if found else f"{signed} {text}"
    return json.dumps(data).encode()


def slug_of(name: str) -> str:
    from .households import slug
    return slug(name)


def family_router() -> APIRouter:
    """What a linked household's bridge may call: introduce itself with an invitation code, list our agents, and
    hand a message over along a link this person turned on."""
    router = APIRouter()

    def caller(request: Request) -> str:
        token = request.headers.get("authorization", "").removeprefix("Bearer ").strip()
        household = request.app.state.services.households.caller(token) if token else None
        if household is None:
            raise HTTPException(status_code=401, detail="unknown household")
        return household

    @router.post("/family/accept")
    async def accept(request: Request) -> Dict[str, Any]:
        body = await request.json()
        try:
            return request.app.state.services.households.accept(
                str(body.get("code", "")), str(body.get("name", "")), str(body.get("url", "")), str(body.get("key", "")))
        except HouseholdError as exc:
            raise HTTPException(status_code=403, detail=str(exc)) from exc

    @router.get("/family/agents")
    async def agents(request: Request) -> Dict[str, Any]:
        caller(request)
        services = request.app.state.services
        return {"name": services.households.name(),
                "agents": [{"id": a.name, "name": a.display_name} for a in services.config.agents.values()]}

    @router.api_route("/family/p/{target}/{path:path}", methods=["GET", "POST"])
    async def deliver(target: str, path: str, request: Request) -> Response:
        household = caller(request)
        services = request.app.state.services
        config: Config = services.config
        agent = config.agent(target)
        if agent is None or not path.startswith(("api/sessions", "v1/runs", "v1/capabilities")):
            raise HTTPException(status_code=404, detail="not found")
        sender = request.headers.get("x-bip-sender", "")
        remote = f"{sender}{SEPARATOR}{household}"
        links = services.links.load()["links"]
        if _is_message(request, path):
            allowed = bool(sender) and (remote, agent.name) in links
        else:
            allowed = any(s.endswith(f"{SEPARATOR}{household}") and t == agent.name for s, t in links)
        if not allowed:
            log.info("household message refused", extra=fields(sender=remote, target=agent.name))
            raise HTTPException(status_code=403, detail=f"{agent.display_name} does not take messages from {remote}")
        body = await request.body()
        answer = await only_bot_chat(services, agent, request.method, path, dict(request.query_params), body)
        if answer is not None:
            return answer
        if _is_message(request, path):
            name = unquote(request.headers.get("x-bip-sender-name", "")) or sender
            body = _resign(body, sender, name, services.households.get(household)["name"])
        headers = {k: v for k, v in request.headers.items() if k.lower() not in _HOP and not k.lower().startswith("x-bip-")}
        headers["Authorization"] = f"Bearer {agent.hermes_key}"
        log.info("household message", extra=fields(sender=remote, target=agent.name))
        return await _forward(services, request.method, f"{agent.hermes_url}/{path}", request, body, headers)

    return router


def household_admin_router() -> APIRouter:
    """For the app: this household's name, its invitations, joining another one, the households linked."""
    router = APIRouter()

    @router.get("/family")
    async def family(request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        return {"name": services.households.name(), "url": own_url(services.config),
                "households": services.households.public()}

    @router.put("/family/name")
    async def set_name(request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        try:
            services.households.set_name(str((await request.json()).get("name", "")))
        except HouseholdError as exc:
            raise HTTPException(status_code=400, detail=str(exc)) from exc
        return {"name": services.households.name()}

    @router.post("/family/invite")
    async def invite(request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        url = own_url(services.config)
        if not url:
            raise HTTPException(status_code=409, detail="this bridge has no address on the tailnet (public_url)")
        try:
            code = services.households.invite()
        except HouseholdError as exc:
            raise HTTPException(status_code=409, detail=str(exc)) from exc
        return {"v": 1, "kind": "family", "url": url, "code": code, "name": services.households.name()}

    @router.post("/family/join")
    async def join(request: Request) -> Dict[str, Any]:
        """The other household's invitation, scanned in the app: this bridge introduces itself to theirs."""
        services = request.app.state.services
        body = await request.json()
        url, code = str(body.get("url", "")).rstrip("/"), str(body.get("code", ""))
        mine = own_url(services.config)
        if not services.households.name() or not mine:
            raise HTTPException(status_code=409, detail="name this household and give the bridge a tailnet address first")
        key = secrets.token_urlsafe(32)
        try:
            resp = await services.http.post(f"{url}/family/accept", timeout=15, json={
                "code": code, "name": services.households.name(), "url": mine, "key": key})
        except httpx.HTTPError as exc:
            raise HTTPException(status_code=502, detail="their bridge does not answer") from exc
        if resp.status_code != 200:
            raise HTTPException(status_code=403, detail="their bridge refused the invitation (expired?)")
        answer = resp.json()
        try:
            household = services.households.joined(str(answer.get("name", "")), url, key_out=key,
                                                   key_in=str(answer.get("key", "")))
        except HouseholdError as exc:
            raise HTTPException(status_code=409, detail=str(exc)) from exc
        return {"id": household, "name": answer.get("name")}

    @router.delete("/family/{household}", status_code=204)
    async def unlink(household: str, request: Request) -> None:
        services = request.app.state.services
        try:
            services.households.remove(household)
        except KeyError as exc:
            raise HTTPException(status_code=404, detail="no such household") from exc
        data = services.links.load()
        data["links"] = [(s, t) for s, t in data["links"]
                         if not s.endswith(f"{SEPARATOR}{household}") and not t.endswith(f"{SEPARATOR}{household}")]
        services.links.save(data)

    return router
