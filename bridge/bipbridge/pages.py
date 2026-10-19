"""Pages the agents make for the person: a dashboard on their blood tests, a tracker, a chart, a quiz, a small tool —
any subject, one prompt. The agent writes one HTML file (its CSS, script and data inline); the bridge keeps it in its
own folder and serves it to the app like any ``MEDIA:`` file under ``/bipagents/pages``. An agent can be talked into
writing anything (a web page or a mail it read), so the page is locked before it is kept: a Content-Security-Policy
first in its head (no network, no forms, no frames: it runs offline), Chart.js from the bridge put inline instead of a
CDN's, and the apps show it in a view with no way out (nothing loaded from the network, no navigation, no bridge to the
app). The agent's own source is kept beside it (``<id>.source``, never served) for ``read_page``.
"""
from __future__ import annotations

import json
import re
import shutil
import time
import unicodedata
from datetime import datetime, timezone
from html import escape
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

from .config import Config

PREFIX = "/bipagents/pages"     # how the app asks for them (`GET /v1/media?path=…`)
MAX_BYTES = 1_000_000           # the agent's HTML; Chart.js adds 200 KB when the page uses it
MAX_PAGES = 100                 # per agent
CHART_JS = Path(__file__).with_name("static") / "vendor" / "chart.umd.min.js"
POLICY = ("default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; "
          "font-src data:; media-src data: blob:; connect-src 'none'; form-action 'none'; base-uri 'none'; "
          "frame-src 'none'; worker-src 'none'; object-src 'none'")
HEAD = (f'<meta http-equiv="Content-Security-Policy" content="{POLICY}">'
        '<meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">'
        '<meta name="color-scheme" content="light dark">')
ID = re.compile(r"^[a-z0-9][a-z0-9-]{0,59}$")
_DOCTYPE = re.compile(r"^\s*(<!doctype[^>]*>)?", re.IGNORECASE)
_CHART = re.compile(r"<script\b[^>]*\bsrc\s*=\s*[\"']?[^\"'\s>]*chart(?:\.umd)?(?:\.min)?\.js[^\"'\s>]*[\"']?[^>]*>"
                    r"\s*</script\s*>", re.IGNORECASE)
_OUTSIDE = re.compile(r"<(script|link|img|iframe|audio|video|source)\b[^>]*\b(?:src|href)\s*=\s*[\"']?((?:https?:)?//[^\"'\s>]+)",
                      re.IGNORECASE)


def root(config: Config) -> Path:
    return Path(config.outbox.db_path).parent / "agent-pages"


def slug(title: str) -> str:
    ascii_title = unicodedata.normalize("NFKD", title).encode("ascii", "ignore").decode()
    return re.sub(r"[^a-z0-9]+", "-", ascii_title.lower()).strip("-")[:40].strip("-") or "page"


def media_path(agent: str, page: str) -> str:
    return f"{PREFIX}/{agent}/{page}.html"


def lock(source: str, title: str) -> Tuple[str, List[str]]:
    """The page as kept: the policy first in its head (before anything that could run), a whole document even when the
    agent wrote a fragment, Chart.js inline. With what it asks from the network (it will not load: the agent hears)."""
    outside = sorted({m.group(2) for m in _OUTSIDE.finditer(_CHART.sub("", source))})
    body = source
    if _CHART.search(body):
        library = CHART_JS.read_text(encoding="utf-8").replace("</script", "<\\/script")
        body = _CHART.sub(lambda _: f"<script>{library}</script>", body, count=1)
        body = _CHART.sub("", body)
    if not re.search(r"<(html|head|body)\b", body, re.IGNORECASE):
        body = f"<title>{escape(title)}</title><body>{body}</body>"
    doctype = _DOCTYPE.match(body).group(1) or "<!doctype html>"
    rest = body[_DOCTYPE.match(body).end():]
    # A <meta> before <html> opens the head: the policy is in force before the agent's first tag.
    return f"{doctype}\n{HEAD}\n{rest}", outside


def _meta_path(folder: Path, page: str) -> Path:
    return folder / f"{page}.json"


def _read_meta(path: Path) -> Dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def save(config: Config, agent: str, html: str, title: str = "", page: Optional[str] = None) -> Dict[str, Any]:
    """Keeps a new page, or a new version of `page` (same path: the person's link shows the new one)."""
    html = (html or "").strip()
    if not html:
        raise ValueError("the page is empty: give its whole HTML")
    size = len(html.encode("utf-8"))
    if size > MAX_BYTES:
        raise ValueError(f"the page is too big ({size // 1000} KB, at most {MAX_BYTES // 1000} KB): keep the data "
                         "to what it shows, no images in base64")
    title = " ".join((title or "").split())[:80]
    folder = root(config) / agent
    if page is not None:
        page = page.strip().lower().removesuffix(".html")
        if not ID.match(page):
            raise ValueError(f"unknown page id: {page} (see list_pages)")
    known = _read_meta(_meta_path(folder, page)) if page else {}
    if not known:
        if len(list_pages(config, agent)) >= MAX_PAGES:
            raise ValueError(f"this agent already has {MAX_PAGES} pages: update one or ask the person which to delete")
        if page is None:
            base = slug(title or "page")
            page, n = base, 2
            while _meta_path(folder, page).exists():
                page, n = f"{base}-{n}", n + 1
    title = title or str(known.get("title") or "") or page.replace("-", " ").capitalize()
    locked, outside = lock(html, title)
    folder.mkdir(parents=True, exist_ok=True)
    now = datetime.now(timezone.utc).isoformat(timespec="seconds")
    meta = {"title": title, "created": known.get("created") or now, "updated": now,
            "version": int(known.get("version") or 0) + 1, "bytes": len(locked.encode("utf-8"))}
    for name, text in ((f"{page}.source", html), (f"{page}.html", locked), (f"{page}.json", json.dumps(meta) + "\n")):
        tmp = folder / f".{name}.part"
        tmp.write_text(text, encoding="utf-8")
        tmp.replace(folder / name)
    return {"page": page, "path": media_path(agent, page), "outside": outside, **meta}


def list_pages(config: Config, agent: str) -> List[Dict[str, Any]]:
    """The agent's pages, the last changed first."""
    folder = root(config) / agent
    found = []
    for path in folder.glob("*.json") if folder.is_dir() else []:
        meta = _read_meta(path)
        page = path.stem
        if meta and ID.match(page) and (folder / f"{page}.html").is_file():
            found.append({"page": page, "path": media_path(agent, page), **meta})
    return sorted(found, key=lambda p: str(p.get("updated") or ""), reverse=True)


def source(config: Config, agent: str, page: str) -> str:
    page = page.strip().lower().removesuffix(".html")
    if not ID.match(page):
        raise ValueError(f"unknown page id: {page} (see list_pages)")
    try:
        return (root(config) / agent / f"{page}.source").read_text(encoding="utf-8")
    except OSError:
        raise ValueError(f"unknown page id: {page} (see list_pages)") from None


def delete(config: Config, agent: str, page: str) -> bool:
    page = page.strip().lower().removesuffix(".html")
    if not ID.match(page):
        return False
    folder = root(config) / agent
    removed = False
    for suffix in (".html", ".source", ".json"):
        path = folder / f"{page}{suffix}"
        if path.is_file():
            path.unlink()
            removed = True
    return removed


def forget(config: Config, agent: str) -> None:
    """A deleted agent: its pages go with it."""
    folder = root(config) / agent
    if folder.is_dir() and not folder.is_symlink():
        shutil.rmtree(folder, ignore_errors=True)


def age(meta: Dict[str, Any], now: Optional[float] = None) -> str:
    """« 3 min », « 2 h », « 5 d » since the page last changed (for the agent's list)."""
    try:
        seconds = (now or time.time()) - datetime.fromisoformat(str(meta.get("updated"))).timestamp()
    except ValueError:
        return "?"
    for unit, size in (("d", 86400), ("h", 3600), ("min", 60)):
        if seconds >= size:
            return f"{int(seconds // size)} {unit}"
    return "now"
