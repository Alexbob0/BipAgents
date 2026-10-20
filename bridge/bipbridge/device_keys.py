"""A key of its own for each of the person's devices, besides the bridge key: one device lost is one key removed, the
others keep working. A device the person approves from another (the guichet's accounts) gets its key from this other
device, which asks the bridge for it and sends it sealed. Only their SHA-256 is kept here.

    GET    /v1/device-keys        {devices: [{id, name, created, seen, me}]}
    POST   /v1/device-keys        {name}  ->  {id, key}          a key for another device of the person
    DELETE /v1/device-keys/{id}   that device can't come in any more

    data_dir/device-keys.json   [{id, name, hash, created, seen}]   (chmod 600)
"""
from __future__ import annotations

import hashlib
import hmac
import json
import os
import secrets
import threading
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel, Field

MAX_DEVICES = 20
SEEN_EVERY = 3600


def _hash(key: str) -> str:
    return hashlib.sha256(key.encode("utf-8")).hexdigest()


class DeviceKeys:
    def __init__(self, path: Path, clock=time.time):
        self.path, self.clock = path, clock
        self.lock = threading.Lock()

    def _read(self) -> List[Dict[str, Any]]:
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
            return [d for d in data if isinstance(d, dict) and d.get("id") and d.get("hash")] if isinstance(data, list) \
                else []
        except (OSError, ValueError):
            return []

    def _write(self, rows: List[Dict[str, Any]]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_suffix(".tmp")
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(rows, fh)
        os.replace(tmp, self.path)

    def match(self, presented: Optional[str]) -> Optional[str]:
        """The device whose key this is (its id), else None. Every key compared, in constant time."""
        if not presented:
            return None
        wanted, found = _hash(presented), None
        rows = self._read()
        for row in rows:
            if hmac.compare_digest(row["hash"], wanted):
                found = row
        if found is None:
            return None
        now = self.clock()
        if now - float(found.get("seen") or 0) > SEEN_EVERY:
            with self.lock:
                rows = self._read()
                for row in rows:
                    if row["id"] == found["id"]:
                        row["seen"] = now
                self._write(rows)
        return str(found["id"])

    def mint(self, name: str) -> Tuple[str, str]:
        with self.lock:
            rows = self._read()
            if len(rows) >= MAX_DEVICES:
                raise ValueError("too many devices: remove one first")
            device, key = secrets.token_hex(6), secrets.token_urlsafe(32)
            rows.append({"id": device, "name": " ".join(name.split())[:60] or "?", "hash": _hash(key),
                         "created": self.clock(), "seen": None})
            self._write(rows)
        return device, key

    def remove(self, device: str) -> bool:
        with self.lock:
            rows = self._read()
            kept = [r for r in rows if r["id"] != device]
            if len(kept) == len(rows):
                return False
            self._write(kept)
        return True

    def clear(self) -> None:
        with self.lock:
            self._write([])

    def listed(self, me: Optional[str]) -> List[Dict[str, Any]]:
        return [{"id": r["id"], "name": r.get("name") or "?", "created": r.get("created"), "seen": r.get("seen"),
                 "me": r["id"] == me} for r in self._read()]


def of(config) -> DeviceKeys:
    return DeviceKeys(Path(config.outbox.db_path).parent / "device-keys.json")


class NewDevice(BaseModel):
    name: str = Field(..., min_length=1, max_length=60)


def device_keys_router() -> APIRouter:
    router = APIRouter()

    @router.get("/device-keys")
    async def listed(request: Request) -> Dict[str, Any]:
        return {"devices": of(request.app.state.services.config).listed(getattr(request.state, "device", None))}

    @router.post("/device-keys")
    async def mint(body: NewDevice, request: Request) -> Dict[str, Any]:
        try:
            device, key = of(request.app.state.services.config).mint(body.name)
        except ValueError as exc:
            raise HTTPException(status_code=409, detail=str(exc)) from exc
        return {"id": device, "key": key}

    @router.delete("/device-keys/{device}")
    async def removed(device: str, request: Request) -> Dict[str, Any]:
        if not of(request.app.state.services.config).remove(device):
            raise HTTPException(status_code=404, detail="no such device")
        return {"ok": True}

    return router
