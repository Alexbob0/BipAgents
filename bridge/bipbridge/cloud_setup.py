"""The app's « no computer » path: a server the app created in the person's own account (DigitalOcean, OVHcloud), set
up by cloud-init with the installer's `--cloud` mode, then paired with the app without a QR code.

The app made a one-time secret (`pair`, 32 random bytes) and put it in the server's user-data, and nothing else secret:
no model key, since the provider keeps user-data. The bridge's door is on the Internet with its own self-signed
certificate (lan.py). First the app learns that this certificate is really its server's: the bridge proves it knows
`pair` with an HMAC over the app's nonce and the certificate's fingerprint, which nobody in between can forge, and the
app pins it. Then the app claims the server over that pinned connection: it proves it knows `pair` too (an HMAC over
the bridge's one-time challenge and the exact request), sends the model key and the agents to create, and receives the
bridge key. The secret is then deleted: any later claim is refused.

    GET  /v1/setup?nonce=<hex>   {state: installing | ready | claimed, step, total, label, fingerprint, challenge, proof}
    POST /v1/setup/claim         {challenge, mac, request: "<JSON>"}  ->  {bridgeURL, bridgeKey, fingerprint, agents}
"""
from __future__ import annotations

import asyncio
import base64
import hashlib
import hmac
import json
import logging
import os
import secrets
import time
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

import httpx
from fastapi import APIRouter, HTTPException, Query, Request
from pydantic import BaseModel, Field, ValidationError

from .auth import address_of
from .config import Config
from .logs import fields

log = logging.getLogger("bipbridge.cloud_setup")

SETUP_CONTEXT = "bipagents-setup-v1"
CLAIM_CONTEXT = "bipagents-claim-v1"
CHALLENGE_SECONDS = 600


def proof(secret: bytes, *parts: str) -> str:
    """HMAC-SHA256 of the parts, one per line, lowercase hex (the apps compute the same)."""
    return hmac.new(secret, "\n".join(parts).encode("utf-8"), hashlib.sha256).hexdigest()


def _read_json(path: Optional[str]) -> Dict[str, Any]:
    if not path:
        return {}
    try:
        data = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def secret_of(config: Config) -> Optional[bytes]:
    """The pairing secret while the server waits for its app; None once claimed (the file is gone)."""
    value = _read_json(config.setup_file).get("pair")
    if not isinstance(value, str):
        return None
    try:
        secret = base64.urlsafe_b64decode(value + "=" * (-len(value) % 4))
    except ValueError:
        return None
    return secret if len(secret) >= 16 else None


def progress_of(config: Config) -> Dict[str, Any]:
    """What the installer wrote at each step (`--cloud`): step, total, label, done."""
    data = _read_json(config.setup_progress)
    return {"step": int(data.get("step") or 0), "total": int(data.get("total") or 0),
            "label": str(data.get("label") or ""), "done": bool(data.get("done"))}


class Challenges:
    """The bridge's one-time challenges for a claim: each one used once, within ten minutes."""

    def __init__(self, clock: Callable[[], float] = time.monotonic):
        self.clock = clock
        self._open: Dict[str, float] = {}

    def issue(self) -> str:
        now = self.clock()
        self._open = {c: t for c, t in self._open.items() if now - t < CHALLENGE_SECONDS}
        if len(self._open) > 64:   # never more than a handful in flight: the oldest go
            for old in sorted(self._open, key=self._open.get)[:32]:
                self._open.pop(old)
        challenge = secrets.token_hex(16)
        self._open[challenge] = now
        return challenge

    def take(self, challenge: str) -> bool:
        issued = self._open.pop(challenge, None)
        return issued is not None and self.clock() - issued < CHALLENGE_SECONDS


class ModelChoice(BaseModel):
    provider: str = Field(..., pattern=r"^(openrouter|custom)$")   # custom: any OpenAI-compatible API (EUrouter…)
    name: str = Field(..., min_length=1, max_length=200)
    base_url: Optional[str] = Field(default=None, max_length=300, pattern=r"^https://[^\s]+$")
    key: str = Field(..., min_length=8, max_length=400, pattern=r"^[^\s]+$")
    vision: bool = False


class AgentChoice(BaseModel):
    name: str = Field(..., min_length=1, max_length=40)
    kind: Optional[str] = Field(default=None, max_length=30)


class ClaimRequest(BaseModel):
    model: ModelChoice
    agents: List[AgentChoice] = Field(default_factory=list, max_length=8)
    language: Optional[str] = Field(default=None, max_length=20)


class Claim(BaseModel):
    challenge: str = Field(..., min_length=16, max_length=128)
    mac: str = Field(..., min_length=64, max_length=64)
    request: str = Field(..., min_length=2, max_length=20_000)   # the JSON the mac covers, exactly as the app sent it


MODEL_KEYS = {"openrouter": "OPENROUTER_API_KEY", "custom": "CUSTOM_MODEL_API_KEY"}


def model_commands(config: Config, model: ModelChoice) -> List[List[str]]:
    """The hub profile's model (every agent is cloned from it), as the installer sets it (hermes.set_model)."""
    def setting(key: str, value: str) -> List[str]:
        return [config.hermes.bin, "config", "set", key, value]   # the hub is the default profile: no -p
    key_env = MODEL_KEYS[model.provider]
    commands = [setting("model.provider", model.provider), setting("model.default", model.name)]
    base_url = model.base_url or ("https://openrouter.ai/api/v1" if model.provider == "openrouter" else None)
    if base_url:
        commands.append(setting("model.base_url", base_url))
    if model.provider == "custom":
        commands.append(setting("model.key_env", key_env))
    commands += [setting(key_env, model.key), setting("model.supports_vision", "true" if model.vision else "false")]
    return commands


def write_model_health(config: Config, base_url: str, key: str) -> None:
    """« Model unreachable » in the app: the bridge checks the model's /models with its key (`[model]`)."""
    config.model_health_url, config.model_health_key = base_url.rstrip("/") + "/models", key
    if not config.source_path:
        return
    path = Path(config.source_path)
    lines, inside, kept = path.read_text(encoding="utf-8").splitlines(), False, []
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            inside = stripped == "[model]"
        if not inside:
            kept.append(line)
    text = "\n".join(kept).rstrip("\n") + "\n\n[model]\n" + \
        f"health_url = {json.dumps(config.model_health_url)}\nhealth_key = {json.dumps(key)}\n"
    path.write_text(text, encoding="utf-8")
    os.chmod(path, 0o600)


def _remember(config: Config, **values: Any) -> None:
    """What a claim already did, kept with the secret: a claim that failed half-way is tried again without redoing it."""
    data = _read_json(config.setup_file)
    data.update(values)
    path = Path(config.setup_file or "")
    tmp = path.with_suffix(".tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        json.dump(data, fh)
    os.replace(tmp, path)


async def claim_server(config: Config, request: ClaimRequest, http: httpx.AsyncClient,
                       run: Optional[Callable[..., Any]] = None) -> List[Dict[str, str]]:
    """The model set, the gateway restarted to read its key, the agents created. Returns the agents (id, name)."""
    from .agents_admin import NewAgent, create_agent, run_hermes
    run = run or run_hermes
    env = {"HERMES_HOME": config.hermes.home} if config.hermes.home else {}
    state = _read_json(config.setup_file)
    if not state.get("model_set"):
        for command in model_commands(config, request.model):
            await run(command, env)
        await run([config.hermes.bin, "gateway", "restart"], env)
        write_model_health(config, request.model.base_url or "https://openrouter.ai/api/v1", request.model.key)
        _remember(config, model_set=True)
    created: Dict[str, str] = dict(state.get("created") or {})   # the app's index -> the profile made for it
    for index, choice in enumerate(request.agents):
        if str(index) in created:
            continue
        agent = await create_agent(config, NewAgent(name=choice.name, kind=choice.kind or "daily",
                                                    lang=request.language), http, run=run)
        created[str(index)] = agent.name
        _remember(config, created=created)
    return [{"id": created[str(i)], "name": config.agents[created[str(i)]].display_name}
            for i in range(len(request.agents)) if created.get(str(i)) in config.agents]


def cloud_setup_router() -> APIRouter:
    router = APIRouter()
    challenges = Challenges()
    lock = asyncio.Lock()

    def guarded(request: Request) -> Config:
        config: Config = request.app.state.services.config
        if not config.setup_file:
            raise HTTPException(status_code=404, detail="not found")
        guard = getattr(request.app.state, "guard", None)
        if guard is not None and guard.blocked(address_of(request)):
            raise HTTPException(status_code=429, detail="too many wrong proofs, try again later")
        return config

    @router.get("/setup")
    async def setup_state(request: Request,
                          nonce: str = Query(..., min_length=32, max_length=128, pattern=r"^[0-9a-fA-F]+$")) -> Dict[str, Any]:
        config = guarded(request)
        secret = secret_of(config)
        if secret is None:
            return {"state": "claimed"}
        from .lan import fingerprint
        print_fp = fingerprint(config) or ""
        step = progress_of(config)
        challenge = challenges.issue()
        return {"state": "ready" if step["done"] else "installing", "step": step["step"], "total": step["total"],
                "label": step["label"], "fingerprint": print_fp, "challenge": challenge,
                "proof": proof(secret, SETUP_CONTEXT, nonce.lower(), print_fp, challenge)}

    @router.post("/setup/claim")
    async def claim(body: Claim, request: Request) -> Dict[str, Any]:
        config = guarded(request)
        services = request.app.state.services
        async with lock:
            secret = secret_of(config)
            if secret is None:
                raise HTTPException(status_code=410, detail="this server is already paired")
            if not challenges.take(body.challenge):
                raise HTTPException(status_code=400, detail="challenge unknown or expired: ask /v1/setup again")
            if not hmac.compare_digest(proof(secret, CLAIM_CONTEXT, body.challenge, body.request), body.mac.lower()):
                guard = getattr(request.app.state, "guard", None)
                if guard is not None:
                    guard.fail(address_of(request), "pairing proof")
                raise HTTPException(status_code=401, detail="wrong pairing proof")
            if not progress_of(config)["done"]:
                raise HTTPException(status_code=409, detail="still installing")
            try:
                wanted = ClaimRequest.model_validate_json(body.request)
            except ValidationError as exc:
                raise HTTPException(status_code=400, detail=f"invalid request: {exc.errors()[0].get('msg', '')}") from exc
            if wanted.model.provider == "custom" and not wanted.model.base_url:
                raise HTTPException(status_code=400, detail="a custom model needs its base_url")
            try:
                agents = await claim_server(config, wanted, services.http)
            except (RuntimeError, asyncio.TimeoutError, ValueError, OSError) as exc:
                log.warning("claim failed", extra=fields(error=str(exc)[-300:]))
                raise HTTPException(status_code=502, detail="the server could not finish: try again") from exc
            Path(config.setup_file or "").unlink(missing_ok=True)   # paired: the secret is gone for good
            log.info("server claimed", extra=fields(agents=len(agents), address=address_of(request)))
        from .lan import fingerprint
        return {"bridgeURL": config.public_url, "bridgeKey": config.bridge_key, "fingerprint": fingerprint(config),
                "agents": agents}

    return router
