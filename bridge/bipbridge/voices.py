"""Which voice speaks for what: each agent has a Bip (Pocket) and, when the person has their own voice server, a voice
there; `[voice_server]` says which uses go to the server (the Live, the replies to voice notes, the reading of
messages and reports), the Bip speaks for the rest.

- ``GET  /v1/voices``                  -> the server's voices (the app shows them as « Voix 1, Voix 2… ») and its uses
- ``PUT  /v1/voices/uses``             ``{"live": true, "replies": false, "reading": true}``
- ``PUT  /v1/agents/{id}/voices``      ``{"voice": "pocket:galet", "server_voice": "server:fr/af_bella"}``
"""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict, List, Optional

from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel, Field

from .config import VOICE_USES, AgentConfig, Config


def voice_for(config: Config, agent: Optional[AgentConfig], use: Optional[str]) -> str:
    """The agent's voice for `use`: its server voice when that use goes to the server, else its Bip."""
    server = config.voice_server
    if agent is not None and use in VOICE_USES and server is not None and server.uses.get(use, True) \
            and agent.server_voice:
        return agent.server_voice
    return (agent.voice if agent else None) or config.default_voice


def relay_for(agent: Optional[AgentConfig]) -> Optional[str]:
    """The Bip standing in for the agent's server voice when the server fails or is too slow."""
    voice = agent.voice if agent else None
    return voice if voice and voice.startswith("pocket:") else None


def set_section_keys(path: str, header: str, values: Dict[str, Any]) -> None:
    """`values` written in the `[header]` table of a TOML file (replaced where they are, else added at its end; the
    table is created when missing). None removes a key. Other lines stay as they are."""
    lines = Path(path).read_text(encoding="utf-8").splitlines()
    start = next((i for i, line in enumerate(lines) if line.strip() == f"[{header}]"), None)
    if start is None:
        lines += ["", f"[{header}]"]
        start = len(lines) - 1
    end = next((i for i in range(start + 1, len(lines)) if lines[i].strip().startswith("[")), len(lines))
    while end > start + 1 and not lines[end - 1].strip():
        end -= 1   # new keys go before the blank lines that end the table
    pending = dict(values)
    kept: List[str] = []
    for line in lines[start + 1:end]:
        key = line.split("=", 1)[0].strip() if "=" in line and not line.lstrip().startswith("#") else ""
        if key in pending:
            value = pending.pop(key)
            if value is not None:
                kept.append(f"{key} = {_toml(value)}")
        else:
            kept.append(line)
    kept += [f"{key} = {_toml(value)}" for key, value in pending.items() if value is not None]
    Path(path).write_text("\n".join(lines[:start + 1] + kept + lines[end:]).rstrip("\n") + "\n", encoding="utf-8")


def _toml(value: Any) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    return json.dumps(value, ensure_ascii=False)   # a TOML basic string for str, a number for int


class Uses(BaseModel):
    live: Optional[bool] = None
    replies: Optional[bool] = None
    reading: Optional[bool] = None


class AgentAppearance(BaseModel):
    category: str = Field(..., max_length=20)
    color: Optional[str] = Field(None, pattern=r"^#[0-9A-Fa-f]{6}$")


class AgentVoices(BaseModel):
    voice: Optional[str] = Field(None, max_length=300)
    server_voice: Optional[str] = Field(None, max_length=300)


def voices_router() -> APIRouter:
    router = APIRouter()

    def services(request: Request) -> Any:
        return request.app.state.services

    @router.get("/voices")
    async def list_voices(request: Request) -> Dict[str, Any]:
        config: Config = request.app.state.config
        tts = services(request).tts
        server = config.voice_server
        return {"pocket": tts.pocket is not None,
                "server": None if server is None else {"voices": await tts.server_voices(), "uses": server.uses}}

    @router.put("/voices/uses")
    async def set_uses(body: Uses, request: Request) -> Dict[str, bool]:
        config: Config = request.app.state.config
        if config.voice_server is None:
            raise HTTPException(status_code=409, detail="no voice server")
        changed = {use: value for use, value in body.model_dump().items() if value is not None}
        if config.source_path:
            set_section_keys(config.source_path, "voice_server", changed)
        config.voice_server.uses.update(changed)
        return config.voice_server.uses

    @router.put("/agents/{agent_id}/voices")
    async def set_agent_voices(agent_id: str, body: AgentVoices, request: Request) -> Dict[str, Optional[str]]:
        config: Config = request.app.state.config
        agent = config.agent(agent_id)
        if agent is None:
            raise HTTPException(status_code=404, detail="unknown agent")
        values = {"voice": body.voice or None, "server_voice": body.server_voice or None}
        if config.source_path:
            set_section_keys(config.source_path, f"agents.{agent.name}", values)
        agent.voice, agent.server_voice = values["voice"], values["server_voice"]
        return values

    @router.put("/agents/{agent_id}/appearance")
    async def set_agent_appearance(agent_id: str, body: AgentAppearance, request: Request) -> Dict[str, Optional[str]]:
        """The agent's Bip as the person chose it in the app (category, custom colour): every app shows the same one."""
        from .souls import CATEGORIES
        config: Config = request.app.state.config
        agent = config.agent(agent_id)
        if agent is None:
            raise HTTPException(status_code=404, detail="unknown agent")
        if body.category not in CATEGORIES:
            raise HTTPException(status_code=400, detail="unknown category")
        values = {"category": body.category, "color": body.color.upper() if body.color else None}
        if config.source_path:
            set_section_keys(config.source_path, f"agents.{agent.name}", values)
        agent.category, agent.color = values["category"], values["color"]
        return values

    return router
