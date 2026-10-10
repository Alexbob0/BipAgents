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
    # Same model and settings as the person's other agents (--clone), its own key, its personality.
    await run([hermes, "profile", "create", profile, "--clone", "--no-alias", "--description", description], env)
    key = secrets.token_urlsafe(32)
    await run([hermes, "-p", profile, "config", "set", "API_SERVER_KEY", key], env)
    folder = Path(install.home or Path.home() / ".hermes") / "profiles" / profile
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "SOUL.md").write_text(soul, encoding="utf-8")
    mark_as_bot(folder)
    uploads = folder / "uploads"
    agent = AgentConfig(name=profile, display_name=name, hermes_url=f"{install.hub_url}/p/{profile}", hermes_key=key,
                        upload_dir_host=str(uploads), upload_dir_container=str(uploads),
                        public_url=f"{install.public_hub_url}/p/{profile}" if install.public_hub_url else None)
    # The gateway picks new profiles up within 30 s; its permanent « Bot Chat » is opened as soon as it answers.
    headers = {"Authorization": f"Bearer {key}"}
    for _ in range(45):
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

    return router
