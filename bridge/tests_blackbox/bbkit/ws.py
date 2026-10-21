"""WebSocket helpers (websockets' sync client): `/v1/voice` and the desktop's stream."""
from __future__ import annotations

import json
from typing import Any, Dict, List, Optional, Tuple

from websockets.exceptions import InvalidStatus
from websockets.sync.client import ClientConnection, connect

__all__ = ["open_ws", "receive_frames", "recv_json", "InvalidStatus"]


def open_ws(url: str, headers: Optional[Dict[str, str]] = None, **kw: Any) -> ClientConnection:
    return connect(url, additional_headers=headers or {}, proxy=None, compression=None, open_timeout=10,
                   max_size=None, **kw)


def recv_json(ws: ClientConnection, timeout: float = 15.0) -> Dict[str, Any]:
    message = ws.recv(timeout=timeout)
    assert isinstance(message, str), f"expected a text frame, got {len(message)} bytes"
    return json.loads(message)


def receive_frames(ws: ClientConnection, until_type: str = "done", limit: int = 50,
                   timeout: float = 20.0) -> List[Tuple[str, Any]]:
    """("json", dict) and ("bin", bytes) frames up to the first JSON frame of type `until_type`."""
    frames: List[Tuple[str, Any]] = []
    for _ in range(limit):
        message = ws.recv(timeout=timeout)
        if isinstance(message, bytes):
            frames.append(("bin", message))
            continue
        data = json.loads(message)
        frames.append(("json", data))
        if data.get("type") == until_type:
            break
    return frames
