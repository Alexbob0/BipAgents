"""Agents created from the app (« + »): on an install made by the installer, the bridge manages the person's own Hermes
(`[hermes]` in its config) and creates a profile for each new agent — the kind or the « Autre » description the person
chose, the name they gave — then serves it at once, without a restart. Everything runs as the person's own account.

Routes (bridge key):
- `GET /v1/agent-kinds`: the kinds of agents offered, and whether this install can create agents.
- `POST /v1/agents`: `{"name", "kind"}` or `{"name", "custom": {"role", "tone", "rules", "language", "category"}}`
  → the new agent as `GET /v1/agents` lists it (id, name, tailnet url, key)."""
from __future__ import annotations

import asyncio
import logging
import os
import secrets
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Sequence

import httpx
import yaml
from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel, Field

from . import souls
from .config import AgentConfig, Config
from . import peers
from . import services as connected
from .spaces import SpaceError, Spaces
from .vault import CannotHold
from .logs import fields

log = logging.getLogger("bipbridge.agents")


class CustomAgent(BaseModel):
    role: str = Field(..., min_length=3, max_length=600)
    tone: str = Field(default="", max_length=200)
    rules: str = Field(default="", max_length=600)
    language: str = Field(default="français", max_length=40)
    category: str = Field(default="tech")


class NewAgent(BaseModel):
    name: str = Field(..., min_length=1, max_length=40)
    kind: Optional[str] = None
    custom: Optional[CustomAgent] = None


Runner = Callable[[Sequence[str]], "asyncio.Future[str]"]


async def run_hermes(args: Sequence[str], env: Dict[str, str]) -> str:
    proc = await asyncio.create_subprocess_exec(*args, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
                                                env={**os.environ, **env})
    out, err = await asyncio.wait_for(proc.communicate(), timeout=120)
    if proc.returncode != 0:
        raise RuntimeError(f"{' '.join(args[:4])} failed: {err.decode(errors='replace').strip()[-300:]}")
    return out.decode(errors="replace")


def mark_as_bot(folder: Path) -> None:
    """Bot Mode: the agents of an install can message each other in their « Bot Chat »."""
    path = folder / "profile.yaml"
    data = yaml.safe_load(path.read_text(encoding="utf-8")) if path.exists() else None
    data = data if isinstance(data, dict) else {}
    data.setdefault("ui_meta", {}).setdefault("hermes-bots", {})
    path.write_text(yaml.safe_dump(data, allow_unicode=True, sort_keys=False), encoding="utf-8")


def append_agent(config_path: str, agent: AgentConfig) -> None:
    """The new agent in the bridge's config file, so it is still there after a restart."""
    q = lambda v: '"' + v.replace("\\", "\\\\").replace('"', '\\"') + '"'
    lines = ["", f"[agents.{agent.name}]", f"display_name = {q(agent.display_name)}",
             f"hermes_url = {q(agent.hermes_url)}", f"hermes_key = {q(agent.hermes_key)}"]
    if agent.public_url:
        lines.append(f"public_url = {q(agent.public_url)}")
    if agent.upload_dir_host:
        lines += [f"upload_dir_host = {q(agent.upload_dir_host)}", f"upload_dir_container = {q(agent.upload_dir_container or '')}"]
    if agent.isolated:
        lines.append("isolated = true")
    if agent.sandboxed:
        lines.append("sandboxed = true")
    if agent.python:
        lines.append("python = [" + ", ".join(q(a) for a in agent.python) + "]")
    for key in ("hermes_home", "hermes_root", "space"):
        if getattr(agent, key):
            lines.append(f"{key} = {q(getattr(agent, key))}")
    with open(config_path, "a", encoding="utf-8") as fh:
        fh.write("\n".join(lines) + "\n")


async def create_agent(config: Config, body: NewAgent, http: httpx.AsyncClient,
                       run: Callable[..., Any] = run_hermes) -> AgentConfig:
    install = config.hermes
    name = body.name.strip()
    if body.custom:
        if body.custom.category not in souls.CATEGORIES:
            raise ValueError("unknown category")
        soul = souls.custom_soul(name, body.custom.role, body.custom.tone, body.custom.rules, body.custom.language)
        description = body.custom.role.strip()[:120]
    elif body.kind:
        found = souls.template(body.kind)
        if not found or not found["soul"]:
            raise ValueError("unknown kind")
        soul, description = souls.soul_for(name, body.kind), found["description"]
    else:
        raise ValueError("a kind or a custom description is needed")
    profile = souls.profile_name(name, config.agents)
    env = {"HERMES_HOME": install.home} if install.home else {}
    hermes = install.bin
    # The hub's (default profile's) model and settings, never another agent's: cloning the active profile would copy
    # its MCP servers and peer keys. Its own key, its personality.
    await run([hermes, "profile", "create", profile, "--clone-from", "default", "--no-alias", "--description", description], env)
    key = secrets.token_urlsafe(32)
    await run([hermes, "-p", profile, "config", "set", "API_SERVER_KEY", key], env)
    folder = Path(install.home or Path.home() / ".hermes") / "profiles" / profile
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "SOUL.md").write_text(soul, encoding="utf-8")
    mark_as_bot(folder)
    uploads = folder / "uploads"
    seen_by_agent = f"{install.container_home}/profiles/{profile}/uploads" if install.container_home else str(uploads)
    agent = AgentConfig(name=profile, display_name=name, hermes_url=f"{install.hub_url}/p/{profile}", hermes_key=key,
                        upload_dir_host=str(uploads), upload_dir_container=seen_by_agent,
                        public_url=f"{install.public_hub_url}/p/{profile}" if install.public_hub_url else None)
    # The gateway picks new profiles up within 30 s; its permanent « Bot Chat » is opened as soon as it answers.
    headers = {"Authorization": f"Bearer {key}"}
    for _ in range(90):   # a new profile is served at once; its key is picked up within ~30 s
        try:
            resp = await http.get(f"{agent.hermes_url}/v1/capabilities", headers=headers, timeout=3)
            if resp.status_code == 200:
                await http.post(f"{agent.hermes_url}/api/sessions", json={"title": "Bot Chat"}, headers=headers, timeout=10)
                break
        except httpx.HTTPError:
            pass
        await asyncio.sleep(1)
    else:
        raise RuntimeError("the gateway did not pick the new agent up")
    if config.source_path:
        append_agent(config.source_path, agent)
    config.agents[profile] = agent
    log.info("agent created", extra=fields(agent=profile, kind=body.kind or "custom"))
    return agent


class AgentSettings(BaseModel):
    soul: Optional[str] = Field(default=None, max_length=8000)
    toolsets: Optional[Dict[str, bool]] = None


def managed(config: Config, agent_id: str) -> AgentConfig:
    """An agent this bridge created: in the person's hub, or moved into its own space (never one set up by hand,
    like an agent in a container of its own)."""
    agent = config.agent(agent_id)
    hub = config.hermes.hub_url
    if agent is None or not config.hermes.enabled or not (agent.space or agent.hermes_url.startswith(f"{hub}/p/")):
        raise LookupError(agent_id)
    return agent


def home_root(config: Config, agent: AgentConfig) -> str:
    """The HERMES_HOME of the gateway that serves the agent: its space's, or the hub's."""
    return agent.space or config.hermes.home or str(Path.home() / ".hermes")


def profile_folder(config: Config, agent: AgentConfig) -> Path:
    return Path(home_root(config, agent)) / "profiles" / agent.name


def save_agent(config: Config, agent: AgentConfig) -> None:
    """The agent's section of the bridge's config file rewritten (after a move)."""
    if config.source_path:
        remove_agent_section(config.source_path, agent.name)
        append_agent(config.source_path, agent)
    config.agents[agent.name] = agent


def remove_agent_section(config_path: str, name: str) -> None:
    """Drops `[agents.<name>]` from the bridge's config file (its lines, up to the next table)."""
    lines = Path(config_path).read_text(encoding="utf-8").splitlines(keepends=True)
    kept, skipping = [], False
    for line in lines:
        header = line.strip()
        if header.startswith("[") and header.endswith("]"):
            skipping = header == f"[agents.{name}]"
        if not skipping:
            kept.append(line)
    Path(config_path).write_text("".join(kept).rstrip("\n") + "\n", encoding="utf-8")


async def read_settings(config: Config, agent: AgentConfig, http: httpx.AsyncClient) -> Dict[str, Any]:
    soul_path = profile_folder(config, agent) / "SOUL.md"
    soul = soul_path.read_text(encoding="utf-8") if soul_path.exists() else ""
    resp = await http.get(f"{agent.hermes_url}/v1/toolsets", headers={"Authorization": f"Bearer {agent.hermes_key}"},
                          timeout=10)
    resp.raise_for_status()
    toolsets = [{k: t.get(k) for k in ("name", "label", "description", "enabled", "configured")}
                for t in resp.json().get("data", []) if isinstance(t, dict) and t.get("name")]
    return {"id": agent.name, "name": agent.display_name, "soul": soul, "toolsets": toolsets, "isolated": agent.isolated,
            "services": connected.listing(config, agent)}


async def write_settings(config: Config, agent: AgentConfig, body: AgentSettings, run: Callable[..., Any] = run_hermes) -> None:
    if body.soul is not None:
        (profile_folder(config, agent) / "SOUL.md").write_text(body.soul.strip() + "\n", encoding="utf-8")
    env = {"HERMES_HOME": home_root(config, agent)}
    for action in ("enable", "disable"):
        names = sorted(n for n, on in (body.toolsets or {}).items() if on == (action == "enable"))
        if any(not n.replace("-", "").replace("_", "").isalnum() for n in names):
            raise ValueError("invalid toolset name")
        if names:
            await run([config.hermes.bin, "-p", agent.name, "tools", action, *names, "--platform", "api_server"], env)


async def delete_agent(config: Config, agent: AgentConfig, run: Callable[..., Any] = run_hermes) -> None:
    if agent.space:
        await Spaces(run).remove(config, agent)
    else:
        env = {"HERMES_HOME": config.hermes.home} if config.hermes.home else {}
        await run([config.hermes.bin, "profile", "delete", agent.name, "--yes"], env)
    if config.source_path:
        remove_agent_section(config.source_path, agent.name)
    config.agents.pop(agent.name, None)
    log.info("agent deleted", extra=fields(agent=agent.name))


def agents_admin_router() -> APIRouter:
    router = APIRouter()
    lock = asyncio.Lock()   # one creation at a time (profile names, the config file)

    @router.get("/agent-kinds")
    async def kinds(request: Request) -> Dict[str, Any]:
        config = request.app.state.services.config
        return {"can_create": config.hermes.enabled,
                "kinds": [{"id": t["category"], "label": t["label"], "description": t["description"],
                           "custom": not t["soul"]} for t in souls.templates()]}

    @router.post("/agents", status_code=201)
    async def new_agent(body: NewAgent, request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        config: Config = services.config
        if not config.hermes.enabled:
            raise HTTPException(status_code=409, detail="this install does not manage its agents (set up by hand)")
        async with lock:
            try:
                agent = await create_agent(config, body, services.http)
            except ValueError as exc:
                raise HTTPException(status_code=400, detail=str(exc)) from exc
            except (RuntimeError, asyncio.TimeoutError) as exc:
                log.warning("agent creation failed", extra=fields(error=str(exc)[:200]))
                raise HTTPException(status_code=502, detail="Hermes could not create the agent") from exc
        return {"id": agent.name, "name": agent.display_name, "url": agent.public_url,
                "key": agent.hermes_key if agent.public_url else None}

    @router.get("/agents/{agent_id}/settings")
    async def get_settings(agent_id: str, request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        try:
            agent = managed(services.config, agent_id)
        except LookupError as exc:
            raise HTTPException(status_code=404, detail="not an agent this install manages") from exc
        try:
            return await read_settings(services.config, agent, services.http)
        except httpx.HTTPError as exc:
            raise HTTPException(status_code=502, detail="Hermes unreachable") from exc

    @router.patch("/agents/{agent_id}/settings")
    async def patch_settings(agent_id: str, body: AgentSettings, request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        try:
            agent = managed(services.config, agent_id)
        except LookupError as exc:
            raise HTTPException(status_code=404, detail="not an agent this install manages") from exc
        async with lock:
            try:
                await write_settings(services.config, agent, body)
            except ValueError as exc:
                raise HTTPException(status_code=400, detail=str(exc)) from exc
            except (RuntimeError, asyncio.TimeoutError) as exc:
                raise HTTPException(status_code=502, detail="Hermes could not change the agent") from exc
        try:
            return await read_settings(services.config, agent, services.http)
        except httpx.HTTPError:
            return {"id": agent.name, "name": agent.display_name}

    @router.get("/agent-links")
    async def agent_links(request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        config: Config = services.config
        return {"agents": [{"id": a.name, "name": a.display_name,
                            "managed": peers.managed_home(config, a) is not None} for a in config.agents.values()],
                "links": [{"from": s, "to": t} for s, t in services.links.load()["links"]]}

    @router.put("/agent-links")
    async def set_agent_links(request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        body = await request.json()
        wanted = [(str(x.get("from", "")).lower(), str(x.get("to", "")).lower()) for x in body.get("links", [])
                  if isinstance(x, dict)]
        async with lock:
            try:
                result = await peers.apply(services.config, services.links, wanted, run_hermes)
            except ValueError as exc:
                raise HTTPException(status_code=400, detail=str(exc)) from exc
            except (RuntimeError, asyncio.TimeoutError) as exc:
                raise HTTPException(status_code=502, detail="Hermes could not be set up") from exc
        # Agents set up by hand need their token added by the administrator: say which, never the token itself.
        return {"links": [{"from": s, "to": t} for s, t in services.links.load()["links"]],
                "needs_setup": sorted(result["manual"])}

    @router.get("/tool-catalog")
    async def tool_catalog() -> Dict[str, Any]:
        return {"services": [{k: item.get(k) for k in ("id", "label", "description", "icon", "personal", "fields")}
                             for item in connected.catalog()]}

    def managed_or_404(request: Request, agent_id: str) -> AgentConfig:
        try:
            return managed(request.app.state.services.config, agent_id)
        except LookupError as exc:
            raise HTTPException(status_code=404, detail="not an agent this install manages") from exc

    @router.post("/agents/{agent_id}/services", status_code=201)
    async def connect_service(agent_id: str, request: Request) -> Dict[str, Any]:
        agent = managed_or_404(request, agent_id)
        try:
            return await connected.add(request.app.state.services.config, agent, await request.json(), run_hermes)
        except CannotHold as exc:
            raise HTTPException(status_code=409, detail={"code": "cannot_hold", "agents": exc.reasons}) from exc
        except connected.ServiceError as exc:
            raise HTTPException(status_code=400, detail=str(exc)) from exc
        except (RuntimeError, asyncio.TimeoutError) as exc:
            raise HTTPException(status_code=502, detail="Hermes could not add the service") from exc

    @router.delete("/agents/{agent_id}/services/{name}", status_code=204)
    async def disconnect_service(agent_id: str, name: str, request: Request) -> None:
        agent = managed_or_404(request, agent_id)
        try:
            await connected.remove(request.app.state.services.config, agent, name, run_hermes)
        except KeyError as exc:
            raise HTTPException(status_code=404, detail="no such service") from exc
        except (RuntimeError, asyncio.TimeoutError) as exc:
            raise HTTPException(status_code=502, detail="Hermes could not remove the service") from exc

    @router.post("/agents/{agent_id}/services/{name}/test")
    async def test_service(agent_id: str, name: str, request: Request) -> Dict[str, Any]:
        agent = managed_or_404(request, agent_id)
        try:
            return await connected.test(request.app.state.services.config, agent, name, run_hermes)
        except KeyError as exc:
            raise HTTPException(status_code=404, detail="no such service") from exc
        except (RuntimeError, asyncio.TimeoutError) as exc:
            return {"ok": False, "message": str(exc)[-300:], "tools": None}

    @router.post("/agents/{agent_id}/space")
    async def own_space(agent_id: str, request: Request) -> Dict[str, Any]:
        """Moves an agent of the shared hub into its own space (so it may hold credentials). Takes up to two minutes."""
        services = request.app.state.services
        try:
            agent = managed(services.config, agent_id)
        except LookupError as exc:
            raise HTTPException(status_code=404, detail="not an agent this install manages") from exc
        async with lock:
            try:
                moved = await Spaces(run_hermes).move(services.config, agent, services.http)
            except SpaceError as exc:
                raise HTTPException(status_code=409, detail=str(exc)) from exc
            except (RuntimeError, OSError, asyncio.TimeoutError) as exc:
                log.warning("move into a space failed", extra=fields(agent=agent.name, error=str(exc)[-200:]))
                raise HTTPException(status_code=502, detail="the space could not be created") from exc
            save_agent(services.config, moved)
        return {"id": moved.name, "name": moved.display_name, "url": moved.public_url,
                "key": moved.hermes_key if moved.public_url else None, "isolated": True}

    @router.delete("/agents/{agent_id}", status_code=204)
    async def remove(agent_id: str, request: Request) -> None:
        services = request.app.state.services
        try:
            agent = managed(services.config, agent_id)
        except LookupError as exc:
            raise HTTPException(status_code=404, detail="not an agent this install manages") from exc
        async with lock:
            try:
                await delete_agent(services.config, agent)
                await services.vault.forget_agent(services.config, agent.name)
            except (RuntimeError, asyncio.TimeoutError) as exc:
                raise HTTPException(status_code=502, detail="Hermes could not delete the agent") from exc

    return router
