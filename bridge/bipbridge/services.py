"""Services connected to an agent (MCP servers), from the app's catalog (tool_catalog.yaml).

Written with Hermes' own CLI (`config set` / `config unset`) into the agent's profile, which its gateway
re-reads within about a minute: no restart. A token goes to the profile's .env (0600) and the config only
refers to it (`Bearer ${MCP_<NAME>_API_KEY}`), so it never appears on a command line or in config.yaml.
"""
from __future__ import annotations

import logging
import re
import unicodedata
from functools import lru_cache
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional
from urllib.parse import urlsplit

import yaml

from .config import AgentConfig, Config
from .logs import fields

log = logging.getLogger("bipbridge.services")

NAME_RE = re.compile(r"^[a-z0-9][a-z0-9_]{0,31}$")


class ServiceError(Exception):
    """Safe to show."""


@lru_cache(maxsize=1)
def catalog() -> List[Dict[str, Any]]:
    with open(Path(__file__).with_name("tool_catalog.yaml"), encoding="utf-8") as fh:
        return yaml.safe_load(fh)


def entry(catalog_id: str) -> Dict[str, Any]:
    for item in catalog():
        if item["id"] == catalog_id:
            return item
    raise ServiceError("unknown service")


def env_name(name: str) -> str:
    return f"MCP_{name.upper()}_API_KEY"


def web_url(value: str) -> str:
    value = value.strip().rstrip("/") if value else ""
    if "://" not in value and value:
        value = "https://" + value
    parts = urlsplit(value)
    if parts.scheme not in ("http", "https") or not parts.hostname:
        raise ServiceError("the address must be a web address")
    return value


def plan(body: Dict[str, Any]) -> Dict[str, Any]:
    """What to write for the app's form: name, url, token (or None), and whether it needs the agent's own space."""
    item = entry(str(body.get("catalog") or ""))
    values = {f["id"]: str(body.get(f["id"]) or "").strip() for f in item["fields"]}
    for f in item["fields"]:
        if not values[f["id"]] and not f.get("optional"):
            raise ServiceError(f"{f['label']} is required")
    name = values.get("name") or item["id"]
    plain = unicodedata.normalize("NFKD", name).encode("ascii", "ignore").decode().lower()   # « Météo » → meteo
    name = re.sub(r"[^a-z0-9_]+", "_", plain).strip("_")[:32] or item["id"]
    if not NAME_RE.match(name):
        raise ServiceError("invalid name")
    address = web_url(values["address"]) if values.get("address") else ""
    url = web_url(item["url"].replace("{address}", address))
    token = values.get("token") or None
    return {"name": name, "url": url, "token": token, "catalog": item["id"],
            "needs_own_space": bool(item.get("personal") or token)}


def profile_home(config: Config, agent: AgentConfig) -> Path:
    return Path(agent.space or config.hermes.home or Path.home() / ".hermes")


def listing(config: Config, agent: AgentConfig) -> List[Dict[str, Any]]:
    path = profile_home(config, agent) / "profiles" / agent.name / "config.yaml"
    try:
        data = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
    except (OSError, yaml.YAMLError):
        return []
    servers = data.get("mcp_servers") or {}
    out = []
    for name, server in servers.items() if isinstance(servers, dict) else []:
        if not isinstance(server, dict):
            continue
        url = str(server.get("url") or "")
        out.append({"name": str(name), "url": url or None, "command": server.get("command"),
                    "enabled": server.get("enabled", True) is not False, "catalog": catalog_of(url)})
    return out


def catalog_of(url: str) -> Optional[str]:
    """Which catalog entry an existing server looks like (None: set up by hand, e.g. a local command)."""
    if not url:
        return None
    for item in catalog():
        pattern = item["url"]
        if "{address}" not in pattern and url.startswith(pattern.rstrip("/")):
            return item["id"]
        prefix, _, suffix = pattern.partition("{address}")
        if prefix == "" and suffix and url.endswith(suffix):
            return item["id"]
    return "custom"


def set_env(path: Path, key: str, value: Optional[str]) -> None:
    lines = path.read_text(encoding="utf-8").splitlines() if path.exists() else []
    kept = [line for line in lines if line.split("=", 1)[0].strip() != key]
    if value is not None:
        kept.append(f"{key}={value}")
    path.write_text("\n".join(kept) + "\n", encoding="utf-8")
    path.chmod(0o600)


async def add(config: Config, agent: AgentConfig, body: Dict[str, Any], run: Callable[..., Any]) -> Dict[str, Any]:
    from .vault import CannotHold, holder

    wanted = plan(body)
    if wanted["needs_own_space"]:
        why = holder(config, agent)
        if why:
            raise CannotHold({agent.name: why})
    home = profile_home(config, agent)
    env = {"HERMES_HOME": str(home)}
    name = wanted["name"]
    if any(s["name"] == name for s in listing(config, agent)):
        raise ServiceError("a service with this name already exists")
    if wanted["token"]:
        set_env(home / "profiles" / agent.name / ".env", env_name(name), wanted["token"])
    hermes = config.hermes.bin
    await run([hermes, "-p", agent.name, "config", "set", f"mcp_servers.{name}.url", wanted["url"]], env)
    if wanted["token"]:
        await run([hermes, "-p", agent.name, "config", "set", f"mcp_servers.{name}.headers.Authorization",
                   "Bearer ${" + env_name(name) + "}"], env)
    if wanted["catalog"] != "home_assistant":
        # Hermes asks the person before each of its tools that writes (sends, creates, deletes): a page or an e-mail
        # the agent read cannot make it act through the service. Home Assistant (lights, heating) stays direct.
        await run([hermes, "-p", agent.name, "config", "set", f"mcp_servers.{name}.trust", "untrusted"], env)
    log.info("service connected", extra=fields(agent=agent.name, service=name, catalog=wanted["catalog"]))
    return {"name": name, "url": wanted["url"], "enabled": True, "catalog": wanted["catalog"]}


async def remove(config: Config, agent: AgentConfig, name: str, run: Callable[..., Any]) -> None:
    if not NAME_RE.match(name) or not any(s["name"] == name for s in listing(config, agent)):
        raise KeyError(name)
    home = profile_home(config, agent)
    await run([config.hermes.bin, "-p", agent.name, "config", "unset", f"mcp_servers.{name}"], {"HERMES_HOME": str(home)})
    set_env(home / "profiles" / agent.name / ".env", env_name(name), None)
    log.info("service removed", extra=fields(agent=agent.name, service=name))


async def test(config: Config, agent: AgentConfig, name: str, run: Callable[..., Any]) -> Dict[str, Any]:
    """`hermes mcp test`: connects and lists the tools. Its last line explains a failure (never a secret)."""
    if not NAME_RE.match(name) or not any(s["name"] == name for s in listing(config, agent)):
        raise KeyError(name)
    out = await run([config.hermes.bin, "-p", agent.name, "mcp", "test", name],
                    {"HERMES_HOME": str(profile_home(config, agent))})
    lines = [line.strip() for line in out.splitlines() if line.strip()]
    failed = next((line for line in lines if line.startswith("✗")), None)
    tools = re.search(r"Tools discovered: (\d+)", out) or re.search(r"(\d+) tools?", out)
    return {"ok": failed is None and any(line.startswith("✓") for line in lines),
            "message": (failed or "").lstrip("✗ ").strip()[:300] or None,
            "tools": int(tools.group(1)) if tools else None}
