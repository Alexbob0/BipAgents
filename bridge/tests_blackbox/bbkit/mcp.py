"""The agents' MCP endpoint (`/v1/agent-tools/mcp`), as an agent's Hermes calls it."""
from __future__ import annotations

import hashlib
import json
from typing import Any, Dict, Optional

TOKEN = "agent-token-for-wellness-0123456789"


def give_token(bridge, agent: str = "wellness", token: str = TOKEN) -> None:
    """agent-tools.json as the bridge keeps it (`{"tokens": {agent: sha256(token)}}`): the agent's tools open."""
    path = bridge.state_dir / "agent-tools.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    tokens = json.loads(path.read_text())["tokens"] if path.exists() else {}
    tokens[agent] = hashlib.sha256(token.encode()).hexdigest()
    path.write_text(json.dumps({"tokens": tokens}, indent=1) + "\n")


def rpc(bridge, method: str, params: Optional[Dict[str, Any]] = None, request_id: Any = 1,
        accept: str = "application/json", token: str = TOKEN):
    body: Dict[str, Any] = {"jsonrpc": "2.0", "method": method, "params": params or {}}
    if request_id is not None:
        body["id"] = request_id
    return bridge.post("/v1/agent-tools/mcp", json=body, headers={"Authorization": f"Bearer {token}", "Accept": accept})


def call(bridge, name: str, arguments: Dict[str, Any], token: str = TOKEN) -> Dict[str, Any]:
    """A tool's result (`{"content": [{"type": "text", "text": …}], "isError": …}`)."""
    return rpc(bridge, "tools/call", {"name": name, "arguments": arguments}, token=token).json()["result"]


def text_of(result: Dict[str, Any]) -> str:
    return result["content"][0]["text"]


def media_of(result: Dict[str, Any]) -> str:
    return text_of(result).split("MEDIA:")[1].split()[0]
