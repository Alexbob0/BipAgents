"""A small TOML writer for the bridge configs the suite writes (strings, numbers, booleans, arrays, tables)."""
from __future__ import annotations

import json
import re
from typing import Any, Dict, List

_BARE = re.compile(r"^[A-Za-z0-9_-]+$")


def _key(key: str) -> str:
    return key if _BARE.match(key) else json.dumps(key, ensure_ascii=False)


def _value(value: Any) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return repr(value)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False)
    if isinstance(value, (list, tuple)):
        return "[" + ", ".join(_value(v) for v in value) + "]"
    raise TypeError(f"cannot write {value!r} in TOML")


def dumps(data: Dict[str, Any]) -> str:
    lines: List[str] = []

    def table(prefix: List[str], node: Dict[str, Any]) -> None:
        scalars = {k: v for k, v in node.items() if not isinstance(v, dict) and v is not None}
        tables = {k: v for k, v in node.items() if isinstance(v, dict)}
        if prefix and (scalars or not tables):
            lines.append("")
            lines.append("[" + ".".join(_key(p) for p in prefix) + "]")
        for key, value in scalars.items():
            lines.append(f"{_key(key)} = {_value(value)}")
        for key, value in tables.items():
            table(prefix + [key], value)

    table([], data)
    return "\n".join(lines).lstrip("\n") + "\n"
