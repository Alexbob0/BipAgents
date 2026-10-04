"""``POST /v1/files``: store a document where the agent's container can read it.

Stored at ``{upload_dir_host}/{YYYY-MM}/{id12}-{name}``; the response gives the same file as seen
from inside the Hermes container: ``{upload_dir_container}/{YYYY-MM}/{id12}-{name}``.
"""
from __future__ import annotations

import logging
import os
import posixpath
import re
import time
import unicodedata
import uuid
from datetime import datetime, timezone
from typing import Any, Callable, Dict, Optional

from fastapi import HTTPException, Request
from starlette.datastructures import UploadFile
from starlette.types import ASGIApp, Message, Receive, Scope, Send

from .config import AgentConfig, Limits
from .logs import fields

log = logging.getLogger("bipbridge.files")

MAX_NAME_CHARS = 120
STORED_NAME = re.compile(r"^[0-9a-f]{12}-.+")
MONTH_DIR = re.compile(r"^\d{4}-\d{2}$")
_UNSAFE = re.compile(r"[^\w.\-+@()\[\],]+", re.UNICODE)
_REPEAT = re.compile(r"_{2,}")
CHUNK = 1024 * 1024


class BodyTooLarge(HTTPException):
    def __init__(self, limit: int):
        super().__init__(status_code=413, detail=f"file too large (max {limit} bytes)")


def sanitize_filename(name: Optional[str]) -> str:
    """Basename only, no control chars, shell-friendly (spaces -> ``_``), no leading dot, ≤120 chars."""
    name = unicodedata.normalize("NFC", name or "")
    name = name.replace("\\", "/").split("/")[-1]
    name = "".join(ch for ch in name if unicodedata.category(ch)[0] != "C")
    name = _UNSAFE.sub("_", name.strip())
    name = _REPEAT.sub("_", name).strip("._-")
    if not name or name in (".", ".."):
        name = "fichier"
    if len(name) > MAX_NAME_CHARS:
        stem, dot, ext = name.rpartition(".")
        if dot and 0 < len(ext) <= 16:
            name = stem[: MAX_NAME_CHARS - len(ext) - 1].rstrip("._-") + "." + ext
        else:
            name = name[:MAX_NAME_CHARS]
    return name


class UploadSizeLimit:
    """ASGI middleware: rejects ``POST /v1/files`` bodies above the limit before/while parsing."""

    def __init__(self, app: ASGIApp, limit_getter: Callable[[], int], path: str = "/v1/files"):
        self.app = app
        self.limit_getter = limit_getter
        self.path = path

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http" or scope.get("path") != self.path:
            await self.app(scope, receive, send)
            return
        # Multipart framing adds a little overhead around the file itself.
        limit = self.limit_getter() + 64 * 1024
        for key, value in scope.get("headers", []):
            if key == b"content-length":
                try:
                    if int(value) > limit:
                        await _send_413(send, self.limit_getter())
                        return
                except ValueError:
                    pass
        received = 0

        async def limited_receive() -> Message:
            nonlocal received
            message = await receive()
            if message["type"] == "http.request":
                received += len(message.get("body", b""))
                if received > limit:
                    raise BodyTooLarge(self.limit_getter())
            return message

        await self.app(scope, limited_receive, send)


async def _send_413(send: Send, limit: int) -> None:
    body = ('{"detail":"file too large (max %d bytes)"}' % limit).encode()
    await send({"type": "http.response.start", "status": 413,
                "headers": [(b"content-type", b"application/json"), (b"content-length", str(len(body)).encode()),
                            (b"connection", b"close")]})
    await send({"type": "http.response.body", "body": body})


async def store_upload(agent: AgentConfig, upload: UploadFile, limits: Limits,
                       now: Optional[datetime] = None) -> Dict[str, Any]:
    if not agent.upload_dir_host or not agent.upload_dir_container:
        raise HTTPException(status_code=409, detail="uploads not configured for this agent")
    name = sanitize_filename(upload.filename)
    month = (now or datetime.now(timezone.utc)).strftime("%Y-%m")
    host_dir = os.path.join(agent.upload_dir_host, month)
    os.makedirs(host_dir, mode=limits.upload_dir_mode, exist_ok=True)
    stored = f"{uuid.uuid4().hex[:12]}-{name}"
    host_path = os.path.join(host_dir, stored)
    fd = os.open(host_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, limits.upload_file_mode)
    size = 0
    try:
        with os.fdopen(fd, "wb") as out:
            os.fchmod(out.fileno(), limits.upload_file_mode)  # ignore umask
            while True:
                chunk = await upload.read(CHUNK)
                if not chunk:
                    break
                size += len(chunk)
                if size > limits.upload_max_bytes:
                    raise BodyTooLarge(limits.upload_max_bytes)
                out.write(chunk)
    except BaseException:
        try:
            os.unlink(host_path)
        except OSError:
            pass
        raise
    container_path = posixpath.join(agent.upload_dir_container, month, stored)
    log.info("file stored", extra=fields(agent=agent.name, size=size, month=month))
    return {"path": container_path, "filename": name, "size": size,
            "content_type": upload.content_type or "application/octet-stream"}


async def handle_upload(request: Request) -> Dict[str, Any]:
    config = request.app.state.config
    try:
        form = await request.form(max_files=1, max_fields=8)
    except BodyTooLarge:
        raise
    except HTTPException:
        raise
    except Exception as exc:
        raise HTTPException(status_code=400, detail="invalid multipart body") from exc
    try:
        agent = config.agent(form.get("agent") if isinstance(form.get("agent"), str) else None)
        if agent is None:
            raise HTTPException(status_code=404, detail="unknown agent")
        upload = form.get("file")
        if not isinstance(upload, UploadFile):
            raise HTTPException(status_code=400, detail="missing 'file' part")
        return await store_upload(agent, upload, config.limits)
    finally:
        await form.close()


def purge_uploads(root: str, retention_days: int, now: Optional[float] = None) -> int:
    """Delete stored uploads older than ``retention_days`` (only files the bridge wrote)."""
    if not root or not os.path.isdir(root):
        return 0
    cutoff = (now or time.time()) - retention_days * 86400
    removed = 0
    for month in os.listdir(root):
        month_dir = os.path.join(root, month)
        if not MONTH_DIR.match(month) or not os.path.isdir(month_dir):
            continue
        for entry in os.listdir(month_dir):
            path = os.path.join(month_dir, entry)
            if not STORED_NAME.match(entry) or not os.path.isfile(path) or os.path.islink(path):
                continue
            try:
                if os.stat(path).st_mtime < cutoff:
                    os.unlink(path)
                    removed += 1
            except OSError:
                continue
        try:
            if not os.listdir(month_dir):
                os.rmdir(month_dir)
        except OSError:
            pass
    return removed
