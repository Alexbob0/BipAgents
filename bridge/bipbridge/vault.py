"""The agents' vault: credentials given from the app, used by the agents without them ever reading one.

The bridge keeps the master copy, encrypted (Fernet) in its data folder, which no agent can reach: agents
run in their own container or sandbox. Each item is then written into the Hermes vault of the agents the
person allowed, and only theirs. Hermes fills the login form itself on the item's exact origin, and the
password never reaches the model, the conversation or the logs (`browser_vault_fill`).

Write-only: the app never reads a secret back. It replaces or deletes it.

An agent can hold an item only when it has its own space (`isolated`): in a shared gateway, an agent with a
terminal could read another's vault. Agents created from the app live in the shared hub until they move
into their own space.
"""
from __future__ import annotations

import asyncio
import json
import logging
import os
import uuid
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional
from urllib.parse import urlsplit

from cryptography.fernet import Fernet, InvalidToken
from fastapi import APIRouter, HTTPException, Request

from .config import AgentConfig, Config
from .logs import fields

log = logging.getLogger("bipbridge.vault")

KINDS = ("login", "payment", "address")
IDENTIFIER_TYPES = ("email", "phone", "username")
PAYMENT_FIELDS = ("card_number", "cardholder_name", "exp_month", "exp_year", "cvc", "billing_postal_code")
ADDRESS_FIELDS = ("address_line1", "address_line2", "city", "state", "postal_code", "country")

# Runs inside the agent's Hermes (its Python, its HERMES_HOME): the request, secret included, comes on stdin,
# never on the command line. Prints JSON only.
HERMES_SCRIPT = r"""
import json, sys
from pathlib import Path
req = json.load(sys.stdin)
if req.get("root"):  # Hermes' own Python with its code folder and dependencies, as its launcher sets them up
    sys.path.insert(0, req["root"])
    import hermes_bootstrap
from agent.vault_store import VaultStore
store = VaultStore(base_dir=Path(req["home"]) / "vault")
if req["op"] == "add":
    meta = store.add_item(req["kind"], req["label"], req["secret"], origin=req.get("origin"))
    print(json.dumps({"id": meta.id}))
elif req["op"] == "remove":
    print(json.dumps({"removed": store.remove_item(req["id"])}))
else:
    print(json.dumps({"items": [m.to_dict() for m in store.list_items()]}))
"""


class VaultError(Exception):
    """Safe to show (never contains a secret)."""


class CannotHold(VaultError):
    """Some agents cannot hold credentials yet: `reasons` maps agent id -> needs_own_space | not_configured."""

    def __init__(self, reasons: Dict[str, str]):
        super().__init__("some agents cannot hold credentials")
        self.reasons = reasons


class Unprotected(VaultError):
    """This install's agents could read the bridge's files: it keeps no credentials."""

    def __init__(self) -> None:
        super().__init__("this server does not isolate its agents: no credentials are kept")


@dataclass
class Item:
    id: str
    kind: str
    label: str
    origin: Optional[str]
    identifier_type: Optional[str] = None
    identifier: Optional[str] = None
    has_otp: bool = False
    agents: Dict[str, str] = field(default_factory=dict)   # agent id -> its handle in that agent's Hermes vault
    created_at: str = ""
    updated_at: str = ""

    def public(self) -> Dict[str, Any]:
        out = asdict(self)
        out["agents"] = sorted(self.agents)
        return out


def now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def normalize_origin(url: str) -> str:
    """« sncf-connect.com/login » → « https://sncf-connect.com »: the origin of what was typed, nothing guessed
    (no « www. » added). Hermes fills only on this exact origin."""
    raw = (url or "").strip()
    if not raw:
        raise VaultError("site is required")
    if "://" not in raw:
        raw = "https://" + raw
    parts = urlsplit(raw)
    if parts.scheme not in ("https", "http") or not parts.hostname:
        raise VaultError("site must be a web address")
    port = f":{parts.port}" if parts.port and parts.port != {"https": 443, "http": 80}[parts.scheme] else ""
    return f"{parts.scheme}://{parts.hostname.lower()}{port}"


def in_hub(config: Config, agent: AgentConfig) -> bool:
    return bool(config.hermes.enabled and not agent.space and agent.hermes_url.startswith(f"{config.hermes.hub_url}/p/"))


def holder(config: Config, agent: AgentConfig) -> Optional[str]:
    """None when the agent can hold credentials, else why not."""
    if not config.hermes.isolated:
        return "unprotected_install"    # the agents could read the bridge's own copy: no credentials at all
    if not agent.isolated:
        # An agent of the shared hub can move into its own space from the app; one set up by hand is up to its
        # administrator (its own container, then `isolated`, `python` and `hermes_home` in this file).
        return "needs_own_space" if in_hub(config, agent) else "not_configured"
    if agent.space and not agent.sandboxed:
        # Its own gateway, but under the same account as the hub's agents, without a sandbox: they could still
        # read its vault on disk.
        return "needs_sandbox"
    if not agent.python or not agent.hermes_home:
        return "not_configured"
    return None


async def run_in_hermes(agent: AgentConfig, request: Dict[str, Any]) -> Dict[str, Any]:
    proc = await asyncio.create_subprocess_exec(
        *agent.python, "-c", HERMES_SCRIPT, stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE)
    payload = json.dumps({"home": agent.hermes_home, "root": agent.hermes_root, **request}).encode()
    out, err = await asyncio.wait_for(proc.communicate(payload), timeout=60)
    if proc.returncode != 0:
        # Hermes' VaultError messages never contain secrets; keep only the last line.
        last = (err.decode(errors="replace").strip().splitlines() or ["?"])[-1][-200:]
        raise VaultError(f"{agent.name}: Hermes refused the item ({last})")
    try:
        return json.loads(out.decode().strip().splitlines()[-1])
    except (ValueError, IndexError) as exc:
        raise VaultError(f"{agent.name}: unexpected answer from Hermes") from exc


class Vault:
    def __init__(self, folder: str, run: Callable[..., Any] = run_in_hermes):
        self.folder = Path(folder)
        self.path = self.folder / "vault.json.enc"
        self.key_path = self.folder / "vault.key"
        self.run = run
        self.lock = asyncio.Lock()

    # -- storage ---------------------------------------------------------------------------------------

    def _fernet(self) -> Fernet:
        self.folder.mkdir(mode=0o700, parents=True, exist_ok=True)
        if not self.key_path.exists():
            fd = os.open(self.key_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "wb") as fh:
                fh.write(Fernet.generate_key())
        return Fernet(self.key_path.read_bytes().strip())

    def _read(self) -> List[Dict[str, Any]]:
        if not self.path.exists():
            return []
        try:
            return json.loads(self._fernet().decrypt(self.path.read_bytes()))
        except InvalidToken as exc:
            raise VaultError("vault cannot be decrypted with its key") from exc

    def _write(self, records: List[Dict[str, Any]]) -> None:
        blob = self._fernet().encrypt(json.dumps(records).encode())
        tmp = self.path.with_name(f".{self.path.name}.{os.getpid()}.tmp")
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "wb") as fh:
            fh.write(blob)
        os.replace(tmp, self.path)

    @staticmethod
    def _item(record: Dict[str, Any]) -> Item:
        return Item(**{k: v for k, v in record.items() if k != "secret"})

    def items(self) -> List[Item]:
        return [self._item(r) for r in self._read()]

    # -- what the app sends ------------------------------------------------------------------------------

    @staticmethod
    def parse(body: Dict[str, Any], existing: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
        """A full record (metadata + secret) from the app's fields; `existing` for an update (fields left out
        are kept, the secret included)."""
        record = dict(existing or {})
        kind = str(body.get("kind") or record.get("kind") or "login")
        if kind not in KINDS:
            raise VaultError("unknown kind")
        record["kind"] = kind
        label = str(body.get("label") if body.get("label") is not None else record.get("label") or "").strip()
        if not label:
            raise VaultError("name is required")
        record["label"] = label[:120]
        secret = dict(record.get("secret") or {})
        if kind == "login":
            if body.get("site") is not None or not record.get("origin"):
                record["origin"] = normalize_origin(str(body.get("site") or ""))
            id_type = str(body.get("identifier_type") or record.get("identifier_type") or "email")
            if id_type not in IDENTIFIER_TYPES:
                raise VaultError("unknown identifier type")
            identifier = str(body.get("identifier") if body.get("identifier") is not None
                             else record.get("identifier") or "").strip()
            if not identifier:
                raise VaultError("user name is required")
            record.update(identifier_type=id_type, identifier=identifier[:200])
            if body.get("password"):
                secret["password"] = str(body["password"])
            if body.get("otp_secret") is not None:
                if str(body["otp_secret"]).strip():
                    secret["otp_secret"] = str(body["otp_secret"]).strip()
                else:
                    secret.pop("otp_secret", None)
            if not secret.get("password"):
                raise VaultError("password is required")
            record["has_otp"] = bool(secret.get("otp_secret"))
        else:
            allowed = PAYMENT_FIELDS if kind == "payment" else ADDRESS_FIELDS
            for name in allowed:
                if body.get(name) is not None:
                    secret[name] = str(body[name]).strip()
            record["origin"] = normalize_origin(str(body["site"])) if body.get("site") else record.get("origin")
            record.update(identifier_type=None, identifier=None, has_otp=False)
        record["secret"] = secret
        return record

    @staticmethod
    def to_hermes(record: Dict[str, Any]) -> Dict[str, Any]:
        secret = dict(record["secret"])
        if record["kind"] == "login":
            secret.update(identifier_type=record["identifier_type"], identifier=record["identifier"])
        return {"op": "add", "kind": record["kind"], "label": record["label"], "origin": record.get("origin"),
                "secret": secret}

    # -- operations ------------------------------------------------------------------------------------

    def _check(self, config: Config, agent_ids: List[str]) -> List[AgentConfig]:
        agents, reasons = [], {}
        for agent_id in agent_ids:
            agent = config.agent(agent_id)
            if agent is None:
                raise VaultError(f"unknown agent {agent_id}")
            why = holder(config, agent)
            if why:
                reasons[agent_id] = why
            agents.append(agent)
        if reasons:
            raise CannotHold(reasons)
        return agents

    async def _give(self, record: Dict[str, Any], agents: List[AgentConfig]) -> None:
        for agent in agents:
            answer = await self.run(agent, self.to_hermes(record))
            record["agents"][agent.name] = str(answer["id"])
            log.info("vault item given", extra=fields(agent=agent.name, item=record["id"]))

    async def _take_back(self, config: Config, record: Dict[str, Any], agent_ids: List[str]) -> None:
        for agent_id in agent_ids:
            handle = record["agents"].pop(agent_id, None)
            agent = config.agent(agent_id)
            if handle and agent and agent.python and agent.hermes_home:
                try:
                    await self.run(agent, {"op": "remove", "id": handle})
                except (VaultError, OSError, asyncio.TimeoutError):
                    log.warning("vault item not removed from agent", extra=fields(agent=agent_id, item=record["id"]))
            log.info("vault item taken back", extra=fields(agent=agent_id, item=record["id"]))

    async def create(self, config: Config, body: Dict[str, Any]) -> Item:
        if not config.hermes.isolated:
            raise Unprotected()
        record = self.parse(body)
        agents = self._check(config, [str(a) for a in body.get("agents") or []])
        record.update(id=f"item_{uuid.uuid4().hex[:12]}", agents={}, created_at=now(), updated_at=now())
        async with self.lock:
            try:
                await self._give(record, agents)
            except Exception:
                await self._take_back(config, record, list(record["agents"]))
                raise
            records = self._read()
            records.append(record)
            self._write(records)
        return self._item(record)

    async def update(self, config: Config, item_id: str, body: Dict[str, Any]) -> Item:
        if not config.hermes.isolated:
            raise Unprotected()
        async with self.lock:
            records = self._read()
            index = next((i for i, r in enumerate(records) if r["id"] == item_id), None)
            if index is None:
                raise KeyError(item_id)
            before = records[index]
            record = self.parse(body, before)
            record["agents"] = dict(before["agents"])
            had = list(before["agents"])
            wanted = [str(a) for a in body["agents"]] if body.get("agents") is not None else had
            changed = any(record.get(k) != before.get(k) for k in ("label", "origin", "identifier", "identifier_type",
                                                                   "secret"))
            # A changed item is written again everywhere (Hermes has no update): taken back, then given.
            give = self._check(config, [a for a in wanted if a not in had or changed])
            await self._take_back(config, record, [a for a in had if a not in wanted or changed])
            await self._give(record, give)
            record["updated_at"] = now()
            records[index] = record
            self._write(records)
        return self._item(record)

    async def delete(self, config: Config, item_id: str) -> None:
        async with self.lock:
            records = self._read()
            record = next((r for r in records if r["id"] == item_id), None)
            if record is None:
                raise KeyError(item_id)
            await self._take_back(config, record, list(record["agents"]))
            self._write([r for r in records if r["id"] != item_id])

    async def forget_agent(self, config: Config, agent_id: str) -> None:
        """An agent deleted: its handles go (its own vault is deleted with it)."""
        async with self.lock:
            records = self._read()
            for record in records:
                record["agents"].pop(agent_id, None)
            self._write(records)


# -- routes (bridge key; the app asks for Face ID before calling them) ------------------------------------

def vault_router() -> APIRouter:
    router = APIRouter()

    def answer(exc: Exception) -> HTTPException:
        if isinstance(exc, CannotHold):
            return HTTPException(status_code=409, detail={"code": "cannot_hold", "agents": exc.reasons})
        if isinstance(exc, Unprotected):
            return HTTPException(status_code=409, detail={"code": "unprotected_install"})
        if isinstance(exc, KeyError):
            return HTTPException(status_code=404, detail="no such item")
        return HTTPException(status_code=400, detail=str(exc))

    @router.get("/vault")
    async def listing(request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        config = services.config
        return {
            "protected": config.hermes.isolated,
            "items": [item.public() for item in services.vault.items()],
            "agents": [{"id": a.name, "name": a.display_name, "can_hold": holder(config, a) is None,
                        "why": holder(config, a)} for a in config.agents.values()],
        }

    @router.post("/vault", status_code=201)
    async def create(request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        try:
            return (await services.vault.create(services.config, await request.json())).public()
        except (VaultError, ValueError) as exc:
            raise answer(exc) from exc

    @router.patch("/vault/{item_id}")
    async def update(item_id: str, request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        try:
            return (await services.vault.update(services.config, item_id, await request.json())).public()
        except (VaultError, ValueError, KeyError) as exc:
            raise answer(exc) from exc

    @router.delete("/vault/{item_id}", status_code=204)
    async def delete(item_id: str, request: Request) -> None:
        services = request.app.state.services
        try:
            await services.vault.delete(services.config, item_id)
        except (VaultError, KeyError) as exc:
            raise answer(exc) from exc

    return router
