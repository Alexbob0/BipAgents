"""This place's backups (installer/bipinstall/backup.py, every hour to the guichet), for its app: the key its phones
keep, to give it back to a new place after an outage (cloud_setup.py's restore), and when the last backup worked.

    GET  /v1/backup        {enabled, key?, last_ok?, last_error?, failed_at?}
    POST /v1/backup/now    one now (the hourly timer's service), e.g. before the person moves their agents

Only on a place of the all-included offer (its settings exist); elsewhere `enabled` is false."""
from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
from typing import Any, Dict

from fastapi import APIRouter

SETTINGS = Path(".config") / "bipagents" / "backup.json"
STATE = Path(".local") / "state" / "bipagents" / "backup.json"
UNIT = "bipagents-backup.service"


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
    out: Dict[str, Any] = {"enabled": True, "key": settings["password"]}
    out.update({k: state[k] for k in ("last_ok", "last_error", "failed_at") if state.get(k) is not None})
    return out


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
