"""An agent's memory, seen and edited by its person in the app: what Hermes keeps across conversations
(`memories/MEMORY.md`, its notes, and `USER.md`, what it knows of the person), and its knowledge pages
(`knowledge/…`, written by long jobs such as a mailbox scan). Hermes has no API for them: the bridge reads and writes
the files as Hermes does — entries separated by « \\n§\\n », the same limits, the same lock — and Hermes takes the
change into the agent's next conversation (a conversation starts from a snapshot).

    GET    /v1/agents/{id}/memory                   {notes: {entries, limit, used}, user: {…}, pages: [{path, size, updated}]}
    PUT    /v1/agents/{id}/memory/{notes|user}      {entries: [...]}
    GET    /v1/agents/{id}/memory/pages/{path}      {path, text}
    PUT    /v1/agents/{id}/memory/pages/{path}      {text}
    DELETE /v1/agents/{id}/memory/pages/{path}

Only an agent this install manages (agents_admin.editable)."""
from __future__ import annotations

import asyncio
import fcntl
import os
import re
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Dict, Iterator, List

import yaml
from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel, Field

from .agents_admin import editable, profile_folder

DELIMITER = "\n§\n"
FILES = {"notes": ("MEMORY.md", 2200), "user": ("USER.md", 1375)}   # Hermes' defaults (memory_char_limit…)
LIMIT_KEYS = {"notes": "memory_char_limit", "user": "user_char_limit"}
PAGE_MAX = 200_000
PAGES_MAX = 2000
PAGE_SUFFIXES = (".md", ".json", ".txt")


class Entries(BaseModel):
    entries: List[str] = Field(default_factory=list, max_length=200)


class Page(BaseModel):
    text: str = Field(..., max_length=PAGE_MAX)


def _limit(folder: Path, which: str) -> int:
    """The profile's own limit when its config sets one, else Hermes' default."""
    try:
        data = yaml.safe_load((folder / "config.yaml").read_text(encoding="utf-8")) or {}
        value = (data.get("memory") or {}).get(LIMIT_KEYS[which])
        if isinstance(value, int) and value > 0:
            return value
    except (OSError, yaml.YAMLError, AttributeError):
        pass
    return FILES[which][1]


@contextmanager
def _locked(path: Path) -> Iterator[None]:
    """Hermes' own lock on a memory file (`<file>.lock`, fcntl): never a write in the middle of its."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path.with_name(path.name + ".lock"), "a+") as handle:
        fcntl.flock(handle, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(handle, fcntl.LOCK_UN)


def read_entries(folder: Path, which: str) -> Dict[str, Any]:
    path = folder / "memories" / FILES[which][0]
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        text = ""
    entries = [e.strip() for e in text.split(DELIMITER) if e.strip()]
    return {"entries": entries, "limit": _limit(folder, which), "used": len(DELIMITER.join(entries))}


def write_entries(folder: Path, which: str, entries: List[str]) -> Dict[str, Any]:
    cleaned = [re.sub(r"\n§\n|^§$", "", e.strip(), flags=re.M).strip() for e in entries]
    cleaned = [e for e in cleaned if e]
    text = DELIMITER.join(cleaned)
    limit = _limit(folder, which)
    if len(text) > limit:
        raise HTTPException(status_code=413, detail=f"{len(text)} characters for {limit} at most")
    path = folder / "memories" / FILES[which][0]
    with _locked(path):
        temporary = path.with_name(path.name + ".bipagents")
        temporary.write_text(text, encoding="utf-8")
        os.replace(temporary, path)
    return read_entries(folder, which)


def _page_path(folder: Path, relative: str) -> Path:
    root = (folder / "knowledge").resolve()
    path = (root / relative).resolve()
    if root not in path.parents or not path.name.endswith(PAGE_SUFFIXES):
        raise HTTPException(status_code=404, detail="not a knowledge page")
    return path


def pages(folder: Path) -> List[Dict[str, Any]]:
    root = folder / "knowledge"
    found = []
    for path in sorted(root.rglob("*")) if root.is_dir() else []:
        if path.is_file() and path.name.endswith(PAGE_SUFFIXES) and not path.is_symlink():
            stat = path.stat()
            found.append({"path": path.relative_to(root).as_posix(), "size": stat.st_size, "updated": stat.st_mtime})
            if len(found) >= PAGES_MAX:
                break
    return found


def memory_router() -> APIRouter:
    router = APIRouter()

    def folder_of(request: Request, agent_id: str) -> Path:
        config = request.app.state.services.config
        try:
            agent = editable(config, agent_id)
        except LookupError as exc:
            raise HTTPException(status_code=404, detail="not an agent this install manages") from exc
        return profile_folder(config, agent)

    @router.get("/agents/{agent_id}/memory")
    async def memory(agent_id: str, request: Request) -> Dict[str, Any]:
        folder = folder_of(request, agent_id)
        return await asyncio.to_thread(lambda: {"notes": read_entries(folder, "notes"), "user": read_entries(folder, "user"),
                                                "pages": pages(folder)})

    @router.put("/agents/{agent_id}/memory/{which}")
    async def put_entries(agent_id: str, which: str, body: Entries, request: Request) -> Dict[str, Any]:
        if which not in FILES:
            raise HTTPException(status_code=404, detail="notes or user")
        folder = folder_of(request, agent_id)
        return await asyncio.to_thread(write_entries, folder, which, body.entries)

    @router.get("/agents/{agent_id}/memory/pages/{path:path}")
    async def get_page(agent_id: str, path: str, request: Request) -> Dict[str, Any]:
        target = _page_path(folder_of(request, agent_id), path)
        try:
            return {"path": path, "text": await asyncio.to_thread(target.read_text, encoding="utf-8")}
        except OSError as exc:
            raise HTTPException(status_code=404, detail="no such page") from exc

    @router.put("/agents/{agent_id}/memory/pages/{path:path}")
    async def put_page(agent_id: str, path: str, body: Page, request: Request) -> Dict[str, Any]:
        target = _page_path(folder_of(request, agent_id), path)

        def write() -> None:
            target.parent.mkdir(parents=True, exist_ok=True)
            temporary = target.with_name(target.name + ".bipagents")
            temporary.write_text(body.text, encoding="utf-8")
            os.replace(temporary, target)

        await asyncio.to_thread(write)
        return {"path": path, "size": len(body.text.encode())}

    @router.delete("/agents/{agent_id}/memory/pages/{path:path}")
    async def delete_page(agent_id: str, path: str, request: Request) -> Dict[str, Any]:
        target = _page_path(folder_of(request, agent_id), path)
        if not target.is_file():
            raise HTTPException(status_code=404, detail="no such page")
        await asyncio.to_thread(target.unlink)
        return {"ok": True}

    return router
