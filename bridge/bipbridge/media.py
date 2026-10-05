"""Files the agents produce and point to with a ``MEDIA:<path>`` line (the podcast mp3, a chart…).

Hermes runs in a container: the path is the agent's (``/home/hermes/simplex-files/x.mp3``). ``[media]
roots`` maps agent-side prefixes to host directories; only files under a declared root, with a known
media extension, are ever served (symlinks and ``..`` resolved first).
"""
from __future__ import annotations

import os
import re
from typing import Dict, List, Optional, Tuple

MEDIA_LINE = re.compile(r"^[ \t]*MEDIA:[ \t]*(\S+)[ \t]*$", re.MULTILINE)

CONTENT_TYPES = {
    ".mp3": "audio/mpeg", ".m4a": "audio/mp4", ".aac": "audio/aac", ".wav": "audio/wav", ".ogg": "audio/ogg",
    ".opus": "audio/ogg", ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".gif": "image/gif",
    ".webp": "image/webp", ".pdf": "application/pdf",
}


def extract_media(text: str) -> Tuple[str, List[str]]:
    """``(text without its MEDIA: lines, [paths])``."""
    paths = MEDIA_LINE.findall(text)
    if not paths:
        return text, []
    cleaned = re.sub(r"\n{3,}", "\n\n", MEDIA_LINE.sub("", text)).strip()
    return cleaned, paths


def resolve(path: str, roots: Dict[str, str]) -> Optional[str]:
    """The host file for an agent-side path, or ``None`` if it is outside every root, missing, or not media."""
    if os.path.splitext(path)[1].lower() not in CONTENT_TYPES:
        return None
    for prefix, host_dir in sorted(roots.items(), key=lambda item: -len(item[0])):
        prefix = prefix.rstrip("/")
        if path != prefix and not path.startswith(prefix + "/"):
            continue
        root = os.path.realpath(os.path.expanduser(host_dir))
        candidate = os.path.realpath(os.path.join(root, path[len(prefix):].lstrip("/")))
        if os.path.commonpath([root, candidate]) != root or not os.path.isfile(candidate):
            return None
        return candidate
    return None


def content_type(path: str) -> str:
    return CONTENT_TYPES.get(os.path.splitext(path)[1].lower(), "application/octet-stream")
