"""WebSocket helper (websockets' sync client): the desktop's stream."""
from __future__ import annotations

from typing import Any, Dict, Optional

from websockets.exceptions import InvalidStatus
from websockets.sync.client import ClientConnection, connect

__all__ = ["open_ws", "InvalidStatus"]


def open_ws(url: str, headers: Optional[Dict[str, str]] = None, **kw: Any) -> ClientConnection:
    return connect(url, additional_headers=headers or {}, proxy=None, compression=None, open_timeout=10,
                   max_size=None, **kw)
