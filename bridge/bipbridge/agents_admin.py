"""Agents created from the app (« + »): on an install made by the installer, the bridge manages the person's own Hermes
(`[hermes]` in its config) and creates a profile for each new agent — the kind or the « Autre » description the person
chose, the name they gave — then serves it at once, without a restart. Everything runs as the person's own account.

Routes (bridge key):
- `GET /v1/agent-kinds`: the kinds of agents offered, and whether this install can create agents.
- `POST /v1/agents`: `{"name", "kind", "lang"}` or `{"name", "custom": {"role", "tone", "rules", "category"}, "lang"}`
  → the new agent as `GET /v1/agents` lists it (id, name, tailnet url, key).
- `PUT /v1/agents/{id}/language`: `{"lang"}` — the language it writes first in (its SOUL.md's language block)."""
from __future__ import annotations

import asyncio
import logging
import os
import re
import secrets
import shutil
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Sequence

import httpx
import yaml
from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel, Field

from . import browsers, shares, souls
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
    language: str = Field(default="", max_length=40)   # older apps: its language by name (« Français »)
    category: str = Field(default="tech")


class NewAgent(BaseModel):
    name: str = Field(..., min_length=1, max_length=40)
    kind: Optional[str] = None
    # The agent's language (« en », « fr-FR »…): its personality is written in it, and it writes first in it.
    # Older apps send none: French, as before.
    lang: Optional[str] = None
    custom: Optional[CustomAgent] = None


class AgentLanguage(BaseModel):
    lang: str = Field(..., min_length=2, max_length=20)


class AgentThinking(BaseModel):
    level: str = Field(..., pattern="^(none|low|medium|high|xhigh)$")


Runner = Callable[[Sequence[str]], "asyncio.Future[str]"]


async def run_hermes(args: Sequence[str], env: Dict[str, str]) -> str:
    proc = await asyncio.create_subprocess_exec(*args, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
                                                env={**os.environ, **env})
    out, err = await asyncio.wait_for(proc.communicate(), timeout=120)
    if proc.returncode != 0:
        raise RuntimeError(f"{' '.join(args[:4])} failed: {err.decode(errors='replace').strip()[-300:]}")
    return out.decode(errors="replace")


# Tools that read this machine's files: off by default where the agents are not isolated from the bridge. The browser
# too: Hermes' local Chromium opens file:// pages (checked: it read a file next to the bridge's keys).
DISK_TOOLS = ("terminal", "file", "code_execution", "browser")


def mark_as_bot(folder: Path) -> None:
    """Bot Mode: the agents of an install can message each other in their « Bot Chat »."""
    path = folder / "profile.yaml"
    data = yaml.safe_load(path.read_text(encoding="utf-8")) if path.exists() else None
    data = data if isinstance(data, dict) else {}
    data.setdefault("ui_meta", {}).setdefault("hermes-bots", {})
    path.write_text(yaml.safe_dump(data, allow_unicode=True, sort_keys=False), encoding="utf-8")


def _line_key(line: str) -> str:
    return line.split("=", 1)[0].strip() if "=" in line and not line.lstrip().startswith("#") else ""


def append_agent(config_path: str, agent: AgentConfig, kept: Sequence[str] = ()) -> None:
    """The new agent in the bridge's config file, so it is still there after a restart. `kept`: other lines of its
    section to keep (what the bridge does not set itself: its voice, ntfy, shares…)."""
    q = lambda v: '"' + v.replace("\\", "\\\\").replace('"', '\\"') + '"'
    key_line = [] if any(_line_key(k) == "hermes_key_file" for k in kept) else [f"hermes_key = {q(agent.hermes_key)}"]
    lines = ["", f"[agents.{agent.name}]", f"display_name = {q(agent.display_name)}",
             f"hermes_url = {q(agent.hermes_url)}"] + key_line
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
    for key in ("hermes_home", "hermes_root", "space", "desktop_url", "desktop_vnc", "desktop_vnc_password_file"):
        if getattr(agent, key):
            lines.append(f"{key} = {q(getattr(agent, key))}")
    written = {_line_key(line) for line in lines} | set(MANAGED_KEYS)
    lines += [line for line in kept if _line_key(line) not in written]
    with open(config_path, "a", encoding="utf-8") as fh:
        fh.write("\n".join(lines) + "\n")


async def create_agent(config: Config, body: NewAgent, http: httpx.AsyncClient,
                       run: Callable[..., Any] = run_hermes) -> AgentConfig:
    install = config.hermes
    name = body.name.strip()
    lang = body.lang or (body.custom.language if body.custom and body.custom.language else "fr")
    if body.custom:
        if body.custom.category not in souls.CATEGORIES:
            raise ValueError("unknown category")
        soul = souls.custom_soul(name, body.custom.role, body.custom.tone, body.custom.rules, lang=lang)
        description = body.custom.role.strip()[:120]
    elif body.kind:
        found = souls.template(body.kind, lang)
        if not found or not found["soul"]:
            raise ValueError("unknown kind")
        soul, description = souls.soul_for(name, body.kind, lang), found["description"]
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
    if not install.isolated:
        # Same account as this bridge and no sandbox: nothing that reads the disk until the person turns it on.
        await run([hermes, "-p", profile, "tools", "disable", *DISK_TOOLS, "--platform", "api_server"], env)
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
    await wait_served(http, agent, bot_chat=True)
    if config.browser.enabled:
        try:
            agent = await browsers.setup(config, agent, run, install.home or str(Path.home() / ".hermes"))
        except (browsers.BrowserError, RuntimeError, OSError) as exc:
            # The agent works without it; `python -m bipbridge browser add <agent>` sets it up later.
            log.warning("browser not set up", extra=fields(agent=profile, error=str(exc)[-200:]))
    if config.source_path:
        append_agent(config.source_path, agent)
    config.agents[profile] = agent
    log.info("agent created", extra=fields(agent=profile, kind=body.kind or "custom"))
    return agent


async def wait_served(http: httpx.AsyncClient, agent: AgentConfig, bot_chat: bool = False) -> None:
    headers = {"Authorization": f"Bearer {agent.hermes_key}"}
    for _ in range(90):   # a new profile is served at once; its key is picked up within ~30 s
        try:
            resp = await http.get(f"{agent.hermes_url}/v1/capabilities", headers=headers, timeout=3)
            if resp.status_code == 200:
                if bot_chat:
                    await http.post(f"{agent.hermes_url}/api/sessions", json={"title": "Bot Chat"}, headers=headers,
                                    timeout=10)
                return
        except httpx.HTTPError:
            pass
        await asyncio.sleep(1)
    raise RuntimeError("the gateway did not pick the agent up")


PROFILE_ID = re.compile(r"^[a-z0-9][a-z0-9_-]{0,63}$")


def unclaimed_profiles(config: Config) -> List[Dict[str, str]]:
    """Profiles of this install's Hermes that the app cannot reach yet: created on the server (`hermes profile
    create`, another tool) rather than from the app. Never the hub's own profile, a deleted one (Hermes leaves a
    tombstone in profiles/.deleted/) or a folder that is not a profile."""
    if not config.hermes.enabled:
        return []
    root = Path(config.hermes.home or Path.home() / ".hermes") / "profiles"
    found = []
    for folder in sorted(root.iterdir()) if root.is_dir() else []:
        name = folder.name
        if (not folder.is_dir() or not PROFILE_ID.match(name) or name in config.agents
                or (root / ".deleted" / name).exists()
                or not any((folder / m).is_file() for m in ("config.yaml", "SOUL.md", "profile.yaml"))):
            continue
        try:
            meta = yaml.safe_load((folder / "profile.yaml").read_text(encoding="utf-8")) or {}
        except (OSError, yaml.YAMLError):
            meta = {}
        description = str(meta.get("description") or "") if isinstance(meta, dict) else ""
        found.append({"id": name, "name": name.replace("-", " ").replace("_", " ").title(), "description": description})
    return found


def profile_key(folder: Path) -> Optional[str]:
    try:
        lines = (folder / ".env").read_text(encoding="utf-8").splitlines()
    except OSError:
        return None
    for line in lines:
        key, _, value = line.partition("=")
        if key.strip() == "API_SERVER_KEY" and value.strip():
            return value.strip().strip('"').strip("'")
    return None


async def claim_profile(config: Config, name: str, display_name: Optional[str], http: httpx.AsyncClient,
                        run: Callable[..., Any] = run_hermes) -> AgentConfig:
    """A profile found on the server, served to the app like an agent made there: its own key (kept when it has
    one), Bot Mode and its permanent « Bot Chat » (the app's « Discussion », where it can write to the other agents),
    its place in this bridge's config, its visible browser when the install gives one. Its personality, tools and
    memory stay as they are."""
    if name not in {p["id"] for p in unclaimed_profiles(config)}:
        raise LookupError(name)
    install = config.hermes
    env = {"HERMES_HOME": install.home} if install.home else {}
    folder = Path(install.home or Path.home() / ".hermes") / "profiles" / name
    key = profile_key(folder)
    if not key:
        key = secrets.token_urlsafe(32)
        await run([install.bin, "-p", name, "config", "set", "API_SERVER_KEY", key], env)
    uploads = folder / "uploads"
    seen_by_agent = f"{install.container_home}/profiles/{name}/uploads" if install.container_home else str(uploads)
    title = (display_name or "").strip() or name.replace("-", " ").replace("_", " ").title()
    agent = AgentConfig(name=name, display_name=title, hermes_url=f"{install.hub_url}/p/{name}", hermes_key=key,
                        upload_dir_host=str(uploads), upload_dir_container=seen_by_agent,
                        public_url=f"{install.public_hub_url}/p/{name}" if install.public_hub_url else None)
    mark_as_bot(folder)
    await wait_served(http, agent, bot_chat=True)
    if config.browser.enabled:
        try:
            agent = await browsers.setup(config, agent, run, install.home or str(Path.home() / ".hermes"))
        except (browsers.BrowserError, RuntimeError, OSError) as exc:
            log.warning("browser not set up", extra=fields(agent=name, error=str(exc)[-200:]))
    if config.source_path:
        append_agent(config.source_path, agent)
    config.agents[name] = agent
    log.info("agent claimed", extra=fields(agent=name))
    return agent


class Claim(BaseModel):
    id: str = Field(max_length=64)
    name: Optional[str] = Field(default=None, max_length=60)


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


def editable(config: Config, agent_id: str) -> AgentConfig:
    """An agent whose personality, language and thinking the app may change: one this bridge created, or one set
    up by hand whose HERMES_HOME it was given (`host_home`)."""
    agent = config.agent(agent_id)
    if agent is not None and agent.host_home:
        return agent
    return managed(config, agent_id)


def home_root(config: Config, agent: AgentConfig) -> str:
    """The HERMES_HOME of the gateway that serves the agent: its space's, the one given for an agent set up by
    hand, or the hub's."""
    return agent.space or agent.host_home or config.hermes.home or str(Path.home() / ".hermes")


def profile_folder(config: Config, agent: AgentConfig) -> Path:
    return Path(home_root(config, agent)) / "profiles" / agent.name


# What the bridge writes in an agent's section: on a rewrite (a move, a browser), these are replaced, any other line
# of the section is kept.
MANAGED_KEYS = ("display_name", "hermes_url", "hermes_key", "public_url", "upload_dir_host", "upload_dir_container",
                "isolated", "sandboxed", "python", "hermes_home", "hermes_root", "space", "desktop_url",
                "desktop_vnc", "desktop_vnc_password_file")


def agent_section_lines(config_path: str, name: str) -> List[str]:
    """The lines of `[agents.<name>]` in the bridge's config file, its header and blank lines left out."""
    lines, inside = [], False
    for line in Path(config_path).read_text(encoding="utf-8").splitlines():
        header = line.strip()
        if header.startswith("[") and header.endswith("]"):
            inside = header == f"[agents.{name}]"
            continue
        if inside and header:
            lines.append(line)
    return lines


def save_agent(config: Config, agent: AgentConfig) -> None:
    """The agent's section of the bridge's config file rewritten (after a move, a browser): what the bridge sets is
    replaced, the rest of the section (voice, ntfy, shares, a key kept in a file…) stays."""
    if config.source_path:
        kept = agent_section_lines(config.source_path, agent.name)
        remove_agent_section(config.source_path, agent.name)
        append_agent(config.source_path, agent, kept)
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


def read_soul(config: Config, agent: AgentConfig) -> str:
    path = profile_folder(config, agent) / "SOUL.md"
    return path.read_text(encoding="utf-8") if path.exists() else ""


def set_language(config: Config, agent: AgentConfig, lang: str) -> str:
    """The agent's language block rewritten (the rest of its personality untouched); returns the language kept."""
    code = souls.agent_language(lang)
    (profile_folder(config, agent) / "SOUL.md").write_text(souls.with_language(read_soul(config, agent), code),
                                                            encoding="utf-8")
    return code


THINKING_PROVIDER = "bipagents-thinking"
THINKING_LEVELS = ("none", "low", "medium", "high", "xhigh")
DEFAULT_THINKING = "medium"   # what Hermes asks a custom server for when nothing is set


def _profile_config(config: Config, agent: AgentConfig) -> Path:
    return profile_folder(config, agent) / "config.yaml"


def read_thinking(config: Config, agent: AgentConfig) -> Optional[str]:
    """How much the agent thinks before answering (none … xhigh), or None when its model is not one the bridge can
    set (an OpenAI-compatible server of the person's: provider `custom` with an address)."""
    try:
        data = yaml.safe_load(_profile_config(config, agent).read_text(encoding="utf-8")) or {}
    except (OSError, yaml.YAMLError):
        return None
    model = data.get("model") if isinstance(data, dict) else None
    if not isinstance(model, dict) or model.get("provider") != "custom" or not model.get("base_url"):
        return None
    effort = (data.get("agent") or {}).get("reasoning_effort") if isinstance(data.get("agent"), dict) else None
    if effort is None or effort == "":
        return DEFAULT_THINKING
    effort = str(effort).strip().lower()
    if effort in ("none", "false", "disabled", "off"):   # YAML false reads « false »
        return "none"
    return effort if effort in THINKING_LEVELS else DEFAULT_THINKING


def set_thinking(config: Config, agent: AgentConfig, level: str) -> str:
    """The agent's thinking level, read by Hermes from the next message (it reloads a profile's config when it
    changes): `reasoning_effort`, which GLM-like servers follow (the model still answers an easy question at once).
    « none » also turns the chat template's `enable_thinking` off (bound to the model's address), for servers that
    only follow that one. Never on: it would win over a turn that asks for no thinking (Live)."""
    if level not in THINKING_LEVELS or read_thinking(config, agent) is None:
        raise LookupError(agent.name)
    path = _profile_config(config, agent)
    data = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
    providers = data.get("providers") if isinstance(data.get("providers"), dict) else {}
    if level == "none":
        providers[THINKING_PROVIDER] = {"api": data["model"]["base_url"],
                                        "extra_body": {"chat_template_kwargs": {"enable_thinking": False}}}
    else:
        providers.pop(THINKING_PROVIDER, None)
    if providers:
        data["providers"] = providers
    else:
        data.pop("providers", None)
    settings = data.get("agent") if isinstance(data.get("agent"), dict) else {}
    settings["reasoning_effort"] = level
    data["agent"] = settings
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(yaml.safe_dump(data, allow_unicode=True, sort_keys=False), encoding="utf-8")
    shutil.copymode(path, temporary)
    os.replace(temporary, path)   # never a half-written config for a gateway reading it
    return level


def heal_souls(config: Config) -> None:
    """Agents whose bridge-owned block (language, what they read) an earlier bridge wrote: brought up to date, the
    personality the person wrote untouched."""
    for name in config.agents:
        try:
            agent = editable(config, name)
            soul = read_soul(config, agent)
            _, lang = souls.split_language(soul)
            if lang is not None and souls.language_block(lang) not in soul:
                (profile_folder(config, agent) / "SOUL.md").write_text(souls.with_language(soul, lang), encoding="utf-8")
        except (LookupError, OSError):
            continue


def heal_thinking(config: Config) -> None:
    """Profiles set by an earlier bridge with the chat template's thinking forced on: that wins over a turn asking for
    none (Live), so it goes (their reasoning_effort stays)."""
    for name in config.agents:
        try:
            agent = editable(config, name)
            if read_thinking(config, agent) not in (None, "none"):
                path = _profile_config(config, agent)
                data = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
                if THINKING_PROVIDER in (data.get("providers") or {}):
                    set_thinking(config, agent, read_thinking(config, agent))
        except (LookupError, OSError, yaml.YAMLError, KeyError):
            continue


async def read_settings(config: Config, agent: AgentConfig, http: httpx.AsyncClient) -> Dict[str, Any]:
    # The language block is the bridge's: shown and changed as « language », never in the editable personality.
    soul, lang = souls.split_language(read_soul(config, agent))
    if agent.host_home:   # set up by hand: its tools are its administrator's
        return {"id": agent.name, "name": agent.display_name, "soul": soul.strip(), "language": lang,
                "thinking": read_thinking(config, agent), "by_hand": True, "toolsets": [], "isolated": agent.isolated,
                "protected": True, "services": []}
    resp = await http.get(f"{agent.hermes_url}/v1/toolsets", headers={"Authorization": f"Bearer {agent.hermes_key}"},
                          timeout=10)
    resp.raise_for_status()
    toolsets = [{k: t.get(k) for k in ("name", "label", "description", "enabled", "configured")}
                for t in resp.json().get("data", []) if isinstance(t, dict) and t.get("name")]
    return {"id": agent.name, "name": agent.display_name, "soul": soul.strip(), "language": lang,
            "thinking": read_thinking(config, agent), "toolsets": toolsets, "isolated": agent.isolated,
            "protected": config.hermes.isolated,
            "services": connected.listing(config, agent)}


async def write_settings(config: Config, agent: AgentConfig, body: AgentSettings, run: Callable[..., Any] = run_hermes) -> None:
    if agent.host_home and body.toolsets:
        raise ValueError("the tools of an agent set up by hand are changed on its server")
    if body.soul is not None:
        # The personality the person edited, with the language block it had (they never see it).
        _, lang = souls.split_language(read_soul(config, agent))
        text = body.soul.strip() + "\n"
        (profile_folder(config, agent) / "SOUL.md").write_text(souls.with_language(text, lang) if lang else text,
                                                                encoding="utf-8")
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
    if browsers.folder(config, agent.name).exists():
        await browsers.remove(config, agent.name, run)
    if config.source_path:
        remove_agent_section(config.source_path, agent.name)
    config.agents.pop(agent.name, None)
    await shares.forget(config, agent.name, run)
    log.info("agent deleted", extra=fields(agent=agent.name))


def agents_admin_router() -> APIRouter:
    router = APIRouter()
    lock = asyncio.Lock()   # one creation at a time (profile names, the config file)

    @router.get("/agent-kinds")
    async def kinds(request: Request, lang: Optional[str] = None) -> Dict[str, Any]:
        config = request.app.state.services.config
        return {"can_create": config.hermes.enabled,
                "kinds": [{"id": t["category"], "label": t["label"], "description": t["description"],
                           "custom": not t["soul"]} for t in souls.templates(lang or "fr")]}

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

    @router.post("/agents/claim", status_code=201)
    async def claim(body: Claim, request: Request) -> Dict[str, Any]:
        """A profile created on the server (listed under `unclaimed` by GET /v1/agents), served to the app."""
        services = request.app.state.services
        async with lock:
            try:
                agent = await claim_profile(services.config, body.id, body.name, services.http)
            except LookupError as exc:
                raise HTTPException(status_code=404, detail="no such profile waiting on the server") from exc
            except (RuntimeError, asyncio.TimeoutError) as exc:
                log.warning("claim failed", extra=fields(agent=body.id, error=str(exc)[-200:]))
                raise HTTPException(status_code=502, detail="Hermes did not serve the agent") from exc
        return {"id": agent.name, "name": agent.display_name, "url": agent.public_url,
                "key": agent.hermes_key if agent.public_url else None}

    @router.get("/agents/{agent_id}/settings")
    async def get_settings(agent_id: str, request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        try:
            agent = editable(services.config, agent_id)
        except LookupError as exc:
            raise HTTPException(status_code=404, detail="not an agent this install manages") from exc
        try:
            return await read_settings(services.config, agent, services.http)
        except httpx.HTTPError as exc:
            raise HTTPException(status_code=502, detail="Hermes unreachable") from exc

    @router.put("/agents/{agent_id}/language")
    async def put_language(agent_id: str, body: AgentLanguage, request: Request) -> Dict[str, Any]:
        """The agent's language, set in the app: it writes first in it (and answers in whatever it is written to)."""
        services = request.app.state.services
        try:
            agent = editable(services.config, agent_id)
        except LookupError as exc:
            raise HTTPException(status_code=404, detail="not an agent this install manages") from exc
        async with lock:
            return {"id": agent.name, "language": set_language(services.config, agent, body.lang)}

    @router.get("/agents/{agent_id}/thinking")
    async def get_thinking(agent_id: str, request: Request) -> Dict[str, Any]:
        """How much the agent thinks before answering (null: its model can't be set from here)."""
        services = request.app.state.services
        try:
            agent = editable(services.config, agent_id)
        except LookupError as exc:
            raise HTTPException(status_code=404, detail="not an agent this install manages") from exc
        return {"id": agent.name, "thinking": read_thinking(services.config, agent)}

    @router.put("/agents/{agent_id}/thinking")
    async def put_thinking(agent_id: str, body: AgentThinking, request: Request) -> Dict[str, Any]:
        """The thinking level, from the app, between two messages."""
        services = request.app.state.services
        try:
            agent = editable(services.config, agent_id)
        except LookupError as exc:
            raise HTTPException(status_code=404, detail="not an agent this install manages") from exc
        async with lock:
            try:
                return {"id": agent.name, "thinking": set_thinking(services.config, agent, body.level)}
            except LookupError as exc:
                raise HTTPException(status_code=409, detail="this agent's model can't be switched from here") from exc

    @router.patch("/agents/{agent_id}/settings")
    async def patch_settings(agent_id: str, body: AgentSettings, request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        try:
            agent = editable(services.config, agent_id)
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
        agents = [{"id": a.name, "name": a.display_name, "managed": peers.managed_home(config, a) is not None}
                  for a in config.agents.values()]
        # The agents of the linked households, as their bridges list them (a household that does not answer is
        # left out for now; its links stay).
        for household in services.households.public():
            entry = services.households.get(household["id"])
            try:
                resp = await services.http.get(f"{entry['url']}/family/agents", timeout=4,
                                               headers={"Authorization": f"Bearer {entry['key_out']}"})
                listed = resp.json().get("agents", []) if resp.status_code == 200 else []
            except (httpx.HTTPError, ValueError):
                listed = []
            for remote in listed:
                agents.append({"id": f"{remote['id']}{peers.SEPARATOR}{household['id']}",
                               "name": f"{remote['name']} ({household['name']})", "managed": False,
                               "household": household["id"]})
        return {"agents": agents, "links": [{"from": s, "to": t} for s, t in services.links.load()["links"]]}

    @router.put("/agent-links")
    async def set_agent_links(request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        body = await request.json()
        wanted = [(str(x.get("from", "")).lower(), str(x.get("to", "")).lower()) for x in body.get("links", [])
                  if isinstance(x, dict)]
        async with lock:
            try:
                result = await peers.apply(services.config, services.links, wanted, run_hermes, services.households)
            except ValueError as exc:
                raise HTTPException(status_code=400, detail=str(exc)) from exc
            except (RuntimeError, asyncio.TimeoutError) as exc:
                raise HTTPException(status_code=502, detail="Hermes could not be set up") from exc
        # Agents set up by hand need their token added by the administrator: say which, never the token itself.
        return {"links": [{"from": s, "to": t} for s, t in services.links.load()["links"]],
                "needs_setup": sorted(result["manual"])}

    @router.get("/agent-shares")
    async def agent_shares(request: Request) -> Dict[str, Any]:
        """Who may read what another agent shares (read-only), and which agents can take part."""
        config: Config = request.app.state.services.config
        return {"agents": [{"id": a.name, "name": a.display_name, "why_not": shares.why_not(config, a),
                            "external": a.shares == "external"} for a in config.agents.values()],
                "reads": [{"reader": r, "source": s} for r, s in shares.load(config)]}

    @router.put("/agent-shares")
    async def set_agent_shares(request: Request) -> Dict[str, Any]:
        config: Config = request.app.state.services.config
        body = await request.json()
        wanted = [(str(x.get("reader", "")).lower(), str(x.get("source", "")).lower()) for x in body.get("reads", [])
                  if isinstance(x, dict)]
        async with lock:
            try:
                reads = await shares.apply(config, wanted, run_hermes)
            except ValueError as exc:
                raise HTTPException(status_code=400, detail=str(exc)) from exc
            except (RuntimeError, asyncio.TimeoutError) as exc:
                raise HTTPException(status_code=502, detail="Hermes could not be set up") from exc
        return {"reads": [{"reader": r, "source": s} for r, s in reads]}

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
