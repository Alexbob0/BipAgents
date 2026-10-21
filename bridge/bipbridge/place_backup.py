"""This place's backups (installer/bipinstall/backup.py, every hour to the guichet), for its app: the key its phones
keep, to give it back to a new place after an outage (cloud_setup.py's restore), and when the last backup worked.

    GET  /v1/backup        {enabled, key?, last_ok?, last_error?, failed_at?, used_bytes?, allowance_bytes}
    POST /v1/backup/now    one now (the hourly timer's service), e.g. before the person moves their agents

Only on a place of the all-included offer (its settings exist); elsewhere `enabled` is false.

The person's space (decided 2026-10-21): 5 GB of their own things — files, photos, notes, memory, conversations — as
the last backup measured them (everything it takes, caches, code and images apart). Once full, files and notes sent
to the bridge are refused (507) until some go; the guichet keeps more for the backups' history."""
from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
from typing import Any, Dict

from fastapi import APIRouter, HTTPException

SETTINGS = Path(".config") / "bipagents" / "backup.json"
STATE = Path(".local") / "state" / "bipagents" / "backup.json"
UNIT = "bipagents-backup.service"
ALLOWANCE_BYTES = 5 * 2**30


def _home() -> Path:
    return Path(os.environ.get("HOME") or Path.home())


def _read(path: Path) -> Dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def status(home: Path) -> Dict[str, Any]:
    settings = _read(home / SETTINGS)
    if not settings.get("url") or not settings.get("password"):
        return {"enabled": False}
    state = _read(home / STATE)
    out: Dict[str, Any] = {"enabled": True, "key": settings["password"], "allowance_bytes": ALLOWANCE_BYTES}
    out.update({k: state[k] for k in ("last_ok", "last_error", "failed_at") if state.get(k) is not None})
    if isinstance(state.get("bytes"), int):
        out["used_bytes"] = state["bytes"]
    return out


def storage_full(home: Path = None) -> bool:
    """True on a place of the offer whose last backup found its person's 5 GB used up."""
    current = status(home or _home())
    return current["enabled"] and current.get("used_bytes", 0) >= current["allowance_bytes"]


def refuse_when_full() -> None:
    if storage_full():
        raise HTTPException(status_code=507, detail="storage full: this place's 5 GB are used up")


def place_backup_router() -> APIRouter:
    router = APIRouter()

    @router.get("/backup")
    async def backup_status() -> Dict[str, Any]:
        return await asyncio.to_thread(status, _home())

    @router.post("/backup/now")
    async def backup_now() -> Dict[str, Any]:
        if not status(_home())["enabled"]:
            return {"started": False}
        process = await asyncio.create_subprocess_exec("systemctl", "--user", "start", "--no-block", UNIT,
                                                       stdout=asyncio.subprocess.DEVNULL,
                                                       stderr=asyncio.subprocess.DEVNULL)
        return {"started": await process.wait() == 0}

    return router
