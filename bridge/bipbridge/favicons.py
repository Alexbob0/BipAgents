"""Site icons for the apps, fetched here rather than by the phone: the links in the agents' replies show their site's
icon, and DuckDuckGo's icon service (which serves them) sees this server's address, never the phone's.

- ``GET /v1/favicon/{host}`` -> the icon (kept 30 days on disk), 404 when the site has none
"""
from __future__ import annotations

import re
import time
from pathlib import Path

import httpx
from fastapi import APIRouter, HTTPException, Request, Response

KEEP_SECONDS = 30 * 24 * 3600
MISSING_SECONDS = 24 * 3600        # a site without an icon is asked again the next day
MAX_BYTES = 256 * 1024
HOST = re.compile(r"^(?=.{1,253}$)(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z][a-z0-9-]{0,62}$")
TYPES = {b"\x89PNG": "image/png", b"GIF8": "image/gif", b"\xff\xd8\xff": "image/jpeg", b"\x00\x00\x01\x00": "image/x-icon",
         b"RIFF": "image/webp"}


def folder(config) -> Path:
    import os
    return Path(os.path.dirname(config.outbox.db_path)) / "favicons"


def image_type(data: bytes) -> str:
    for magic, kind in TYPES.items():
        if data.startswith(magic):
            return kind
    return ""


def favicons_router() -> APIRouter:
    router = APIRouter()

    @router.get("/favicon/{host}")
    async def favicon(host: str, request: Request) -> Response:
        host = host.strip().lower().rstrip(".")
        if not HOST.match(host):
            raise HTTPException(status_code=400, detail="not a site name")
        services = request.app.state.services
        cache = folder(services.config)
        icon, missing = cache / f"{host}.icon", cache / f"{host}.missing"
        now = time.time()
        if icon.exists() and now - icon.stat().st_mtime < KEEP_SECONDS:
            data = icon.read_bytes()
            return Response(data, media_type=image_type(data) or "image/x-icon",
                            headers={"Cache-Control": "private, max-age=86400"})
        if missing.exists() and now - missing.stat().st_mtime < MISSING_SECONDS:
            raise HTTPException(status_code=404, detail="no icon")
        try:
            resp = await services.http.get(f"https://icons.duckduckgo.com/ip3/{host}.ico", timeout=8.0,
                                           follow_redirects=True)
        except httpx.HTTPError:
            raise HTTPException(status_code=502, detail="icon service unreachable") from None
        data = resp.content if resp.status_code == 200 else b""
        cache.mkdir(parents=True, exist_ok=True)
        if not data or len(data) > MAX_BYTES or not image_type(data):
            missing.touch()
            raise HTTPException(status_code=404, detail="no icon")
        partial = icon.with_suffix(".part")
        partial.write_bytes(data)
        partial.replace(icon)
        missing.unlink(missing_ok=True)
        return Response(data, media_type=image_type(data), headers={"Cache-Control": "private, max-age=86400"})

    return router
