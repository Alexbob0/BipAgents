"""The person's notes brought into an agent's knowledge (decided 2026-10-20): Google Keep has no API for personal
accounts, Apple Notes none at all, so the app sends files the person exported — Google Takeout's archive of Keep (its
JSON notes), or text and Markdown files (Apple Notes saved by a Shortcut, an Obsidian folder…). Each note becomes a page
`knowledge/notes/<source>/<slug>.md`, and `knowledge/notes/index.md` lists them all; the agent reads them with
knowledge_read and knowledge_search, and the person sees and edits them in « Sa mémoire ».

    POST /v1/agents/{id}/memory/import   {name, data (base64), source?}  ->  {notes, skipped}
"""
from __future__ import annotations

import asyncio
import base64
import binascii
import io
import json
import os
import re
import zipfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel, Field

from .agents_admin import editable, profile_folder
from .mail_scan import slug
from .mailbox import html_text

MAX_BYTES = 60 * 2**20
MAX_NOTES = 5000
NOTE_CHARS = 50_000
TEXT_SUFFIXES = (".txt", ".md", ".markdown")


class Upload(BaseModel):
    name: str = Field(..., min_length=1, max_length=300)
    data: str = Field(..., max_length=MAX_BYTES * 4 // 3 + 16)
    source: Optional[str] = Field(default=None, max_length=40)


def _keep(data: Dict[str, Any]) -> Optional[Tuple[str, str, str]]:
    """A Google Keep note from Takeout (title, Markdown, date), None when it is trashed or empty."""
    if data.get("isTrashed"):
        return None
    title = str(data.get("title") or "").strip()
    lines: List[str] = []
    if data.get("textContent"):
        lines.append(str(data["textContent"]).strip())
    for item in data.get("listContent") or []:
        if isinstance(item, dict) and item.get("text"):
            lines.append(f"- [{'x' if item.get('isChecked') else ' '}] {item['text']}")
    labels = [str(l.get("name")) for l in data.get("labels") or [] if isinstance(l, dict) and l.get("name")]
    if labels:
        lines.append("\nÉtiquettes : " + ", ".join(labels))
    if data.get("isArchived"):
        lines.append("_(archivée dans Keep)_")
    body = "\n".join(lines).strip()
    if not title and not body:
        return None
    micros = data.get("userEditedTimestampUsec") or data.get("createdTimestampUsec")
    day = datetime.fromtimestamp(int(micros) / 1e6, timezone.utc).date().isoformat() if micros else ""
    return title or body.splitlines()[0][:60], body, day


def notes_of(name: str, raw: bytes) -> Tuple[List[Tuple[str, str, str]], int]:
    """(title, Markdown, date) of each note in an upload, and how many files were left out."""
    found: List[Tuple[str, str, str]] = []
    skipped = 0

    def one(path: str, content: bytes) -> None:
        nonlocal skipped
        low = path.lower()
        try:
            if low.endswith(".json"):
                note = _keep(json.loads(content.decode("utf-8")))
                if note:
                    found.append(note)
                else:
                    skipped += 1
            elif low.endswith(TEXT_SUFFIXES):
                text = content.decode("utf-8", "replace").strip()
                title = Path(path).stem
                if text:
                    found.append((title, text, ""))
            elif low.endswith((".html", ".htm")):
                if not any(p.lower() == low[:-5] + ".json" for p in names):
                    text = re.sub(r"\n\s*\n+", "\n\n", html_text(content.decode("utf-8", "replace"))).strip()
                    if text:
                        found.append((Path(path).stem, text, ""))
            else:
                skipped += 1
        except (ValueError, UnicodeDecodeError):
            skipped += 1

    if name.lower().endswith(".zip"):
        try:
            archive = zipfile.ZipFile(io.BytesIO(raw))
        except zipfile.BadZipFile as exc:
            raise HTTPException(status_code=400, detail="not a zip archive") from exc
        names = [i.filename for i in archive.infolist() if not i.is_dir()]
        for info in archive.infolist():
            if info.is_dir() or info.file_size > 5 * 2**20 or len(found) >= MAX_NOTES:
                continue
            one(info.filename, archive.read(info))
    else:
        names = [name]
        one(name, raw)
    return found, skipped


def write_notes(folder: Path, source: str, notes: List[Tuple[str, str, str]]) -> int:
    """The notes as pages of `knowledge/notes/<source>/`, then the index of every source written again."""
    root = folder / "knowledge" / "notes"
    target = root / slug(source)
    target.mkdir(parents=True, exist_ok=True)
    used: Dict[str, int] = {}
    for title, body, day in notes:
        key = slug(title)
        used[key] = used.get(key, 0) + 1
        name = key if used[key] == 1 else f"{key}-{used[key]}"
        text = f"# {title}\n\n" + (f"_{day}_\n\n" if day else "") + body[:NOTE_CHARS] + "\n"
        path = target / f"{name}.md"
        temporary = path.with_name(path.name + ".bipagents")
        temporary.write_text(text, encoding="utf-8")
        os.replace(temporary, path)
    lines = ["# Notes", "", "Les notes importées par la personne, par source. Chaque ligne mène à une note (knowledge_read).", ""]
    for source_dir in sorted(p for p in root.iterdir() if p.is_dir()):
        pages = sorted(source_dir.glob("*.md"))
        if not pages:
            continue
        lines.append(f"## {source_dir.name} ({len(pages)})")
        for page in pages:
            first = page.read_text(encoding="utf-8").splitlines()[0].lstrip("# ").strip() if page.stat().st_size else page.stem
            lines.append(f"- [{first}]({source_dir.name}/{page.name})")
        lines.append("")
    (root / "index.md").write_text("\n".join(lines).rstrip() + "\n", encoding="utf-8")
    return len(notes)


def notes_router() -> APIRouter:
    router = APIRouter()

    @router.post("/agents/{agent_id}/memory/import")
    async def import_notes(agent_id: str, body: Upload, request: Request) -> Dict[str, Any]:
        config = request.app.state.services.config
        try:
            agent = editable(config, agent_id)
        except LookupError as exc:
            raise HTTPException(status_code=404, detail="not an agent this install manages") from exc
        try:
            raw = base64.b64decode(body.data, validate=True)
        except (binascii.Error, ValueError) as exc:
            raise HTTPException(status_code=400, detail="not base64") from exc
        if len(raw) > MAX_BYTES:
            raise HTTPException(status_code=413, detail="60 MB at most")
        found, skipped = await asyncio.to_thread(notes_of, body.name, raw)
        if not found:
            raise HTTPException(status_code=422, detail="no note found in this file")
        lowered = body.name.lower()
        source = body.source or ("keep" if lowered.startswith("takeout") or lowered.endswith(".json")
                                 else Path(body.name).stem or "notes")
        written = await asyncio.to_thread(write_notes, profile_folder(config, agent), source, found)
        return {"notes": written, "skipped": skipped}

    return router
