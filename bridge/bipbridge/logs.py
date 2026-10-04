"""Structured JSON logging. Never pass secrets or message bodies as fields at info level."""
from __future__ import annotations

import json
import logging
import sys
import time
from typing import Any, Optional

_RESERVED = set(vars(logging.makeLogRecord({})).keys()) | {"message", "asctime", "fields"}


class JsonFormatter(logging.Formatter):
    def format(self, record: logging.LogRecord) -> str:
        payload = {
            "ts": time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(record.created))
            + f".{int(record.msecs):03d}Z",
            "level": record.levelname.lower(),
            "logger": record.name,
            "msg": record.getMessage(),
        }
        fields = getattr(record, "fields", None)
        if isinstance(fields, dict):
            payload.update(fields)
        for key, value in record.__dict__.items():
            if key not in _RESERVED and not key.startswith("_") and key not in payload:
                payload[key] = value
        if record.exc_info:
            payload["exc"] = self.formatException(record.exc_info)
        return json.dumps(payload, ensure_ascii=False, default=str)


def setup_logging(level: str = "info") -> None:
    handler = logging.StreamHandler(sys.stdout)
    handler.setFormatter(JsonFormatter())
    root = logging.getLogger()
    root.handlers[:] = [handler]
    root.setLevel(getattr(logging, level.upper(), logging.INFO))
    # Access logs would print device tokens contained in paths: keep them off.
    logging.getLogger("uvicorn.access").disabled = True
    logging.getLogger("httpx").setLevel(logging.WARNING)
    logging.getLogger("httpcore").setLevel(logging.WARNING)
    logging.getLogger("hpack").setLevel(logging.WARNING)


def redact(value: Optional[str], keep: int = 6) -> str:
    """Short, non-reversible hint for identifiers such as device tokens."""
    if not value:
        return ""
    return value[:keep] + "…" if len(value) > keep else "…"


def fields(**kwargs: Any) -> dict:
    return {"fields": kwargs}
