#!/usr/bin/env python3
"""Where consecutive requests of a Hermes session stop sharing their beginning: why a model server's prompt cache
misses (it can only reuse the part of a prompt that is identical to an earlier one).

Reads the dumps Hermes writes with HERMES_DUMP_REQUESTS=1 (request_dump_<session>_<time>.json in a profile's logs/)
and prints positions, sizes, roles and tool names only, never the text of a message. A heading is named only when
Hermes' own source contains it word for word (a section of the system prompt, not a line from a discussion or a
memory); any other is shown as « (hidden) ».

    prompt-cache-diff.py <logs dir>... [--hermes-source ~/.hermes/hermes-agent] [--session <id>]
"""
import argparse
import json
import sys
from pathlib import Path
from typing import Dict, List, Optional

SETTINGS = ("model", "reasoning_effort", "temperature", "top_p", "max_tokens", "extra_body", "tool_choice")


def body(path: Path) -> dict:
    return json.loads(path.read_text())["request"]["body"]


def flat(message: dict) -> str:
    """Everything of a message that reaches the template, in a stable order."""
    return json.dumps(message, sort_keys=True, ensure_ascii=False)


def common(a: str, b: str) -> int:
    n = min(len(a), len(b))
    i = 0
    while i < n and a[i] == b[i]:
        i += 1
    return i


def tokens(chars: int) -> str:
    return f"~{chars // 4:,} tok"   # rough: 4 characters a token


def landmark(text: str, at: int, source: str) -> str:
    """The last heading before `at`, if Hermes wrote it."""
    for line in reversed(text[:at].splitlines()):
        line = line.strip()
        if line.startswith("#") or (line.startswith("<") and line.endswith(">") and " " not in line):
            return repr(line[:60]) if line in source else "(hidden)"
    return "(before any heading)"


def compare(a: dict, b: dict, source: str) -> str:
    changed = [k for k in SETTINGS if a.get(k) != b.get(k)]
    notes = [f"settings changed: {', '.join(f'{k}={json.dumps(a.get(k))[:80]}→{json.dumps(b.get(k))[:80]}' for k in changed)}"] if changed else []
    ta, tb = a.get("tools") or [], b.get("tools") or []
    names_a = [t.get("function", {}).get("name", "?") for t in ta]
    names_b = [t.get("function", {}).get("name", "?") for t in tb]
    if flat({"t": ta}) != flat({"t": tb}):
        i = next((i for i, (x, y) in enumerate(zip(ta, tb)) if flat(x) != flat(y)), min(len(ta), len(tb)))
        added = sorted(set(names_b) - set(names_a))
        gone = sorted(set(names_a) - set(names_b))
        notes.append(f"tools differ ({len(ta)}→{len(tb)}) from #{i} {names_a[i] if i < len(names_a) else '-'}"
                     + (f"; added {added[:8]}" if added else "") + (f"; removed {gone[:8]}" if gone else "")
                     + ("; same tools, other order or schema" if not added and not gone else ""))
    ma, mb = a.get("messages") or [], b.get("messages") or []
    shared = 0
    for i, (x, y) in enumerate(zip(ma, mb)):
        fx, fy = flat(x), flat(y)
        if fx == fy:
            shared += len(fx)
            continue
        content = x.get("content") if isinstance(x.get("content"), str) else fx
        other = y.get("content") if isinstance(y.get("content"), str) else fy
        at = common(content, other)
        where = landmark(content, at, source) if x.get("role") == "system" else f"a {x.get('role')} message"
        notes.append(f"messages differ at #{i} ({x.get('role')}, {tokens(len(content))}) after {tokens(at)} of it,"
                     f" under {where}; shared before it: {tokens(shared + at)} of {tokens(sum(len(flat(m)) for m in ma))}")
        break
    else:
        if len(mb) >= len(ma):
            notes.append(f"messages: same beginning, {len(mb) - len(ma)} appended ({tokens(shared)} reusable)")
        else:
            notes.append(f"messages: {len(ma) - len(mb)} fewer (history replaced, e.g. by a compression)")
    return "; ".join(notes)


def hermes_source(root: Path) -> str:
    parts = []
    for sub in ("agent", "gateway", "tools", "hermes_cli", "plugins"):
        for path in (root / sub).rglob("*.py"):
            try:
                parts.append(path.read_text(errors="ignore"))
            except OSError:
                pass
    return "\n".join(parts).replace("\\n", "\n")


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("logs", nargs="+", type=Path)
    parser.add_argument("--hermes-source", type=Path, default=Path.home() / ".hermes" / "hermes-agent")
    parser.add_argument("--session")
    args = parser.parse_args(argv)
    dumps = sorted((p for d in args.logs for p in d.glob("request_dump_*.json")), key=lambda p: p.stat().st_mtime)
    if not dumps:
        print("No request dump: is HERMES_DUMP_REQUESTS=1 set for that gateway, and was a message sent since?")
        return 1
    sessions: Dict[str, List[Path]] = {}
    for path in dumps:
        try:
            sid = json.loads(path.read_text()).get("session_id") or "?"
        except (OSError, ValueError):
            continue
        sessions.setdefault(sid, []).append(path)
    source = hermes_source(args.hermes_source)
    for sid, paths in sessions.items():
        if args.session and sid != args.session:
            continue
        print(f"== session {sid}: {len(paths)} requests")
        for before, after in zip(paths, paths[1:]):
            print(f"  {before.stem[-22:-7]} → {after.stem[-22:-7]}: {compare(body(before), body(after), source)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
