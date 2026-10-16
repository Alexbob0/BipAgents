"""The MCP server's door while the person holds the browser.

Hermes talks to the browser's MCP server over stdio (`mcp.sh`); this sits in between. While the bridge's flag
(`/tmp/bipagents-hold`, set when the person takes over the desktop in the app) is there and less than 15 minutes old,
every tool call gets « the person has the browser, wait » instead of reaching Chrome: the agent neither clicks nor
reads the page while the person types a password into it. Everything else passes through, line by line (MCP's
stdio framing: one JSON message per line).

    python3.11 -I hold_gate.py <the server's command…>
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import time

HOLD = "/tmp/bipagents-hold"
LIMIT = 15 * 60   # a hold the app never gave back (closed, crashed) lapses
WAIT = ("The person has taken over this browser in the BipAgents app (to log in, or to do something themselves). "
        "Do not use the browser until they hand it back: tell them you are waiting, or go on with something else.")


def held() -> bool:
    try:
        return time.time() - os.path.getmtime(HOLD) < LIMIT
    except OSError:
        return False


def refusal(message: dict) -> bytes:
    return (json.dumps({"jsonrpc": "2.0", "id": message.get("id"),
                        "result": {"content": [{"type": "text", "text": WAIT}], "isError": True}}) + "\n").encode()


def main(command: list) -> int:
    child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    out = sys.stdout.buffer
    lock = threading.Lock()

    def answers() -> None:
        assert child.stdout is not None
        for line in child.stdout:
            with lock:
                out.write(line)
                out.flush()

    reader = threading.Thread(target=answers, daemon=True)
    reader.start()
    assert child.stdin is not None
    for line in sys.stdin.buffer:
        try:
            message = json.loads(line)
        except ValueError:
            message = None
        if isinstance(message, dict) and message.get("method") == "tools/call" and "id" in message and held():
            with lock:
                out.write(refusal(message))
                out.flush()
            continue
        try:
            child.stdin.write(line)
            child.stdin.flush()
        except BrokenPipeError:
            break
    try:
        child.stdin.close()
    except BrokenPipeError:
        pass
    code = child.wait()
    reader.join(timeout=5)   # its last answers out before this process ends
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
