"""Provider routing at EUrouter (2026-10-26): every model call names the providers EUrouter tries first, so their
prompt cache works (unpinned, EUrouter picks one of seven providers per call and nothing is ever read from a cache).
The bridge does the same for its agents (bridge-rs/src/model_route.rs); this side writes it in the Hermes configs the
installer makes (the hub, every agent cloned from it) and in existing ones (migration provider-routing).

Hermes merges a custom endpoint's `extra_body` (a `providers:` entry whose `api` is the model's address) into every
request to it, the auxiliary ones included. It takes the FIRST matching entry: when the bridge's thinking entry is
there, it carries the routing and the routing entry is left out.
"""
from __future__ import annotations

from pathlib import Path
from typing import List, Optional
from urllib.parse import urlparse

import yaml

DEFAULT_ORDER = ["entrim", "melious"]   # bridge-rs/src/model_route.rs DEFAULT_ORDER
ROUTE_ENTRY = "bipagents-route"
THINKING_ENTRY = "bipagents-thinking"


def pins(base_url: str) -> bool:
    """EUrouter only: another router or server ignores or refuses the field."""
    try:
        host = (urlparse((base_url or "").strip()).hostname or "").lower()
    except ValueError:
        return False
    return "eurouter" in host.split(".")


def routing(order: List[str], base_url: str) -> Optional[dict]:
    """The request's `provider` object, or None (not EUrouter, or no order). Availability still wins."""
    if not order or not pins(base_url):
        return None
    return {"order": list(order), "allow_fallbacks": True}


def with_routing(data: dict, order: List[str]) -> bool:
    """`data` (a Hermes config) routed in place; whether it changed."""
    before = yaml.safe_dump(data, sort_keys=False)
    model = data.get("model") if isinstance(data.get("model"), dict) else {}
    route = routing(order, str(model.get("base_url") or "")) if model.get("provider") == "custom" else None
    providers = data.get("providers") if isinstance(data.get("providers"), dict) else {}
    thinking = providers.get(THINKING_ENTRY)
    if isinstance(thinking, dict):
        extra = thinking.setdefault("extra_body", {})
        if isinstance(extra, dict):
            if route:
                extra["provider"] = route
            else:
                extra.pop("provider", None)
        providers.pop(ROUTE_ENTRY, None)
    elif route:
        providers[ROUTE_ENTRY] = {"api": str(model["base_url"]), "extra_body": {"provider": route}}
    else:
        providers.pop(ROUTE_ENTRY, None)
    if providers:
        data["providers"] = providers
    else:
        data.pop("providers", None)
    return yaml.safe_dump(data, sort_keys=False) != before


def write(path: Path, order: List[str]) -> bool:
    """The routing in a Hermes config file; whether it changed."""
    if not path.is_file():
        return False
    data = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
    if not isinstance(data, dict) or not with_routing(data, order):
        return False
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(yaml.safe_dump(data, allow_unicode=True, sort_keys=False), encoding="utf-8")
    tmp.chmod(path.stat().st_mode & 0o777)
    tmp.replace(path)
    return True


def order_from(bridge_config: Optional[Path]) -> List[str]:
    """The bridge's `[model] provider_order` when it sets one, else the default."""
    if bridge_config is None or not bridge_config.is_file():
        return list(DEFAULT_ORDER)
    try:
        import tomllib
    except ModuleNotFoundError:   # Python < 3.11
        import tomli as tomllib   # type: ignore[no-redef]
    try:
        data = tomllib.loads(bridge_config.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return list(DEFAULT_ORDER)
    order = (data.get("model") or {}).get("provider_order")
    if not isinstance(order, list):
        return list(DEFAULT_ORDER)
    return [str(p).strip().lower() for p in order if str(p).strip()]
