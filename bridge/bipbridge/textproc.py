"""Text normalization for speech and incremental sentence splitting.

The splitter works on the *raw* streamed markdown (deltas from Hermes) and only cuts where the
boundary is certain: end punctuation followed by whitespace (so ``3.5`` is never cut, since the
digit after the dot is seen before deciding), a line break, or a soft cut near ``max_chars`` at a
word boundary. Each emitted segment is then normalized (markdown removed, links reduced to their
label, list items ended with a period so they are read naturally).
"""
from __future__ import annotations

import re
import unicodedata
from typing import List, Optional

END_PUNCT = ".!?…"
# Characters that may close a sentence after its punctuation: quotes, brackets, emphasis markers.
CLOSERS = "\"'»”’)]*_"
SPACES = " \u00a0\u202f"
# Words after which a period does not end a sentence (compared lowercase, without the dot).
ABBREVIATIONS = {
    "m", "mm", "mme", "mmes", "mlle", "mlles", "dr", "pr", "st", "ste", "cf", "ex", "p", "pp", "vs",
    "env", "mr", "mrs", "ms", "jr", "sr", "fig", "chap", "av", "bd", "n°", "etc", "e.g", "i.e",
    "approx", "tél", "réf", "al",
}

_FENCE = re.compile(r"^\s{0,3}(```|~~~)")
_HTML_TAG = re.compile(r"</?[A-Za-z][^>\n]*>")
_IMAGE = re.compile(r"!\[([^\]]*)\]\([^)]*\)")
_LINK = re.compile(r"\[([^\]]+)\]\([^)]*\)")
_REF_LINK = re.compile(r"\[([^\]]+)\]\[[^\]]*\]")
_AUTOLINK = re.compile(r"<(?:https?|mailto):[^>\s]+>")
_BARE_URL = re.compile(r"\b(?:https?://|www\.)[^\s)\]>]+")
_INLINE_CODE = re.compile(r"`+([^`]*)`+")
_HEADING = re.compile(r"^\s{0,3}#{1,6}\s*")
_QUOTE = re.compile(r"^\s{0,3}(>\s?)+")
_BULLET = re.compile(r"^\s*[-*+•▪◦]\s+")
_NUMBERED = re.compile(r"^\s*\d{1,3}[.)]\s+")
_HRULE = re.compile(r"^\s*([-*_=]\s*){3,}$")
_TABLE_SEP = re.compile(r"^\s*\|?[\s:|-]+\|[\s:|-]*$")
_BOLD = re.compile(r"(\*\*|__)(.+?)\1")
_ITALIC_STAR = re.compile(r"(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])")
_ITALIC_UNDERSCORE = re.compile(r"(?<![\w_])_(?!\s)(.+?)(?<!\s)_(?![\w_])")
_STRIKE = re.compile(r"~~(.+?)~~")
_STRAY_MARKS = re.compile(r"[*`#]+|(?<!\w)_+|_+(?!\w)")
_SPACES = re.compile(r"\s+")
_SPACE_BEFORE_COMMA = re.compile(r"\s+([,.])")
_ALNUM = re.compile(r"\w", re.UNICODE)


def normalize_segment(text: str) -> str:
    """Turn one markdown segment (a sentence or a line) into plain speakable text."""
    if not text:
        return ""
    text = unicodedata.normalize("NFC", text)
    lines = []
    for line in text.splitlines():
        if _HRULE.match(line) or _TABLE_SEP.match(line) or _FENCE.match(line):
            continue
        line = _HEADING.sub("", line)
        line = _QUOTE.sub("", line)
        line = _BULLET.sub("", line)
        line = _NUMBERED.sub("", line)
        if line.count("|") >= 2:  # table row -> comma separated cells
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            line = ", ".join(c for c in cells if c)
        lines.append(line)
    text = " ".join(lines)
    text = _HTML_TAG.sub("", text)
    text = _IMAGE.sub(r"\1", text)
    text = _LINK.sub(r"\1", text)
    text = _REF_LINK.sub(r"\1", text)
    text = _AUTOLINK.sub("", text)
    text = _BARE_URL.sub("", text)
    text = _INLINE_CODE.sub(r"\1", text)
    for _ in range(2):  # nested emphasis such as ***x***
        text = _BOLD.sub(r"\2", text)
        text = _ITALIC_STAR.sub(r"\1", text)
        text = _ITALIC_UNDERSCORE.sub(r"\1", text)
    text = _STRIKE.sub(r"\1", text)
    text = _STRAY_MARKS.sub("", text)
    text = _SPACES.sub(" ", text).strip()
    text = _SPACE_BEFORE_COMMA.sub(r"\1", text)
    text = text.strip(" ,;")
    if not _ALNUM.search(text):
        return ""
    if text[-1] not in END_PUNCT + ":;,\"'»”)":
        text += "."
    return text


def _is_list_marker(buf: str, dot_index: int, at_line_start: bool) -> bool:
    """``1. `` at the start of a line is a numbered list marker, not a sentence end."""
    line_start = buf.rfind("\n", 0, dot_index) + 1
    if line_start == 0 and not at_line_start:
        return False
    prefix = buf[line_start:dot_index].strip()
    return prefix.isdigit() and len(prefix) <= 3


def _is_abbreviation(buf: str, dot_index: int) -> bool:
    start = dot_index
    while start > 0 and (buf[start - 1].isalnum() or buf[start - 1] in "°."):
        start -= 1
    word = buf[start:dot_index].lower()
    if not word:
        return False
    if len(word) == 1 and buf[dot_index - 1].isupper():  # initials: "J. Dupont"
        return True
    return word in ABBREVIATIONS


class SentenceSplitter:
    """Incremental splitter: ``feed(delta)`` returns the sentences completed so far, ``flush()``
    returns the rest. Returned sentences are already normalized for speech."""

    def __init__(self, max_chars: int = 180):
        self.max_chars = max(40, int(max_chars))
        self._buf = ""
        self._in_fence = False
        self._at_line_start = True

    def feed(self, delta: str) -> List[str]:
        if not delta:
            return []
        self._buf += delta.replace("\r\n", "\n").replace("\r", "\n")
        out: List[str] = []
        while True:
            seg = self._next_segment()
            if seg is None:
                break
            norm = normalize_segment(seg)
            if norm:
                out.append(norm)
        return out

    def flush(self) -> List[str]:
        rest, self._buf = self._buf, ""
        in_fence, self._in_fence = self._in_fence, False
        self._at_line_start = True
        if in_fence or not rest.strip():
            return []
        norm = normalize_segment(rest)
        return [norm] if norm else []

    # -- internals -------------------------------------------------------------------------

    def _consume(self, end: int) -> str:
        seg = self._buf[:end]
        rest = self._buf[end:].lstrip(" \t")
        if rest.startswith("\n"):
            rest = rest[1:]
            self._at_line_start = True
        else:
            self._at_line_start = False
        self._buf = rest
        return seg

    def _next_segment(self):
        buf = self._buf
        if not buf:
            return None
        nl = buf.find("\n")
        # Inside a code fence: drop whole lines until the closing fence.
        if self._in_fence:
            if nl < 0:
                return None
            line = buf[:nl]
            self._buf = buf[nl + 1:]
            self._at_line_start = True
            if _FENCE.match(line):
                self._in_fence = False
            return ""
        if self._at_line_start:
            head = buf.lstrip(" ")
            if head.startswith("```") or head.startswith("~~~"):
                if nl < 0:
                    return None
                self._buf = buf[nl + 1:]
                self._in_fence = True
                self._at_line_start = True
                return ""
            if len(head) < 3 and nl < 0 and set(head) <= set("`~ "):
                return None  # might become a fence
        limit = nl if nl >= 0 else len(buf)
        cut = self._punctuation_boundary(buf, limit)
        if cut is not None:
            return self._consume(cut)
        if limit > self.max_chars:
            cut = self._soft_cut(buf[:limit], complete=nl >= 0)
            if cut is not None:
                return self._consume(cut)
        if nl >= 0:
            seg = buf[:nl]
            self._buf = buf[nl + 1:]
            self._at_line_start = True
            return seg
        return None

    def _punctuation_boundary(self, buf: str, limit: int):
        i = 0
        while i < limit:
            ch = buf[i]
            if ch in END_PUNCT:
                j = i
                while j + 1 < len(buf) and buf[j + 1] in END_PUNCT:
                    j += 1
                k = j + 1
                while True:
                    while k < len(buf) and buf[k] in CLOSERS:
                        k += 1
                    # French spacing before a closing guillemet: "oui. »"
                    if k < len(buf) and buf[k] in SPACES and k + 1 < len(buf) and buf[k + 1] == "»":
                        k += 2
                        continue
                    break
                if k >= len(buf):
                    return None  # cannot decide yet: need the next character
                if buf[k] in SPACES and k + 1 >= len(buf) and buf.count("«", 0, k) > buf.count("»", 0, k):
                    return None  # an open « may still be closed by " »"
                if buf[k].isspace():
                    if ch == "." and j == i and (
                        _is_abbreviation(buf, i) or _is_list_marker(buf, i, self._at_line_start)
                    ):
                        i = k
                        continue
                    return k
                i = k
                continue
            i += 1
        return None

    def _soft_cut(self, buf: str, complete: bool) -> Optional[int]:
        """Cut point near ``max_chars`` at a word boundary (prefer after a comma/semicolon)."""
        window = buf[: self.max_chars]
        floor = self.max_chars // 3
        for sep in (", ", "; ", ": ", " – ", " — ", " - "):
            idx = window.rfind(sep)
            if idx >= floor:
                return idx + len(sep.rstrip())
        idx = window.rfind(" ")
        if idx >= floor:
            return idx
        # One huge token (URL...): cut at the next whitespace, once it is known.
        nxt = buf.find(" ", self.max_chars)
        if nxt > 0:
            return nxt
        if complete or len(buf) > 4 * self.max_chars:
            return len(buf)
        return None


def split_sentences(text: str, max_chars: int = 180) -> List[str]:
    splitter = SentenceSplitter(max_chars)
    return splitter.feed(text) + splitter.flush()


def normalize_for_speech(text: str, max_chars: int = 180) -> str:
    """Whole-text normalization (used for outbox audio): sentences joined by spaces."""
    return " ".join(split_sentences(text, max_chars))
