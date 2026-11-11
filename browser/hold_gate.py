"""The MCP server's door while the person holds the browser.

Hermes talks to the browser's MCP server over stdio (`mcp.sh`); this sits in between. While the bridge's flag
(`/tmp/bipagents-hold`, set when the person takes over the desktop in the app) is there and less than 15 minutes old,
every tool call gets « the person has the browser, wait » instead of reaching Chrome: the agent neither clicks nor
reads the page while the person types a password into it. A card payment holds it the same way (the flag then reads
« card <token> »): the bridge types the card into the page and clicks « Pay » itself, and the agent can neither read
the card's fields nor take a screenshot of them until the page has moved on. Nor, while held, read the server's
resources (the page's recorded traffic, console, state) or ask anything but its handshake and lists. Everything else
passes through, line by line (MCP's stdio framing: one JSON message per line).

It also counts the tool calls it let through that have not been answered yet, in `/tmp/bipagents-calls/<its pid>`: a
call already running when a card payment's hold starts is not refused, so the bridge waits until every gate's count
is 0 before it reloads the payment page and types the card (bridge-rs/src/card_pay.rs `calls_done`): nothing the
agent asked before the hold then runs in that page (security audit 2026-11-11, BV-1).

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


PAYING = ("A card payment is going on in this browser: the bridge typed the card into the page, and nobody may read "
          "it. Your browser tools wait until it ends: use confirm_payment (or cancel_payment) from the BipAgents tools.")
UNCOUNTED = ("This browser can't keep track of its tool calls (its /tmp is not writable), so its tools are refused: "
             "tell the person their browser must be restarted from the BipAgents app.")


def paying() -> bool:
    try:
        with open(HOLD, "rb") as flag:
            return flag.read(4) == b"card"
    except OSError:
        return False


def held() -> bool:
    try:
        return time.time() - os.path.getmtime(HOLD) < LIMIT
    except OSError:
        return False


CALLS = "/tmp/bipagents-calls"


class Calls:
    """The tool calls let through and not answered yet (by their JSON-RPC id), their number kept in this gate's file.
    One older than `LIMIT` is no longer counted (an answer lost would otherwise stop every payment). When the file
    can't be written (`counting` false), the gate refuses tool calls instead: the bridge would read a count that is
    not this gate's and type a card while a call runs (fix review 11/11, R3)."""

    def __init__(self, folder: str = CALLS) -> None:
        self.running: dict = {}
        self.lock = threading.Lock()
        self.path = os.path.join(folder, str(os.getpid()))
        self.counting = True
        try:
            os.makedirs(folder, mode=0o700, exist_ok=True)
        except OSError:
            self.path = None
            self.counting = False
        self._write()

    def _write(self) -> None:
        if self.path is None:
            return
        now = time.time()
        for key in [k for k, since in self.running.items() if now - since > LIMIT]:
            del self.running[key]
        temporary = os.path.join(os.path.dirname(self.path), "." + os.path.basename(self.path))
        try:
            with open(temporary, "w") as file:
                file.write(str(len(self.running)))
            os.replace(temporary, self.path)
        except OSError:
            self.counting = False

    def start(self, request_id) -> None:
        with self.lock:
            self.running[json.dumps(request_id)] = time.time()
            self._write()

    def end(self, request_id) -> None:
        with self.lock:
            if self.running.pop(json.dumps(request_id), None) is not None:
                self._write()

    def count(self) -> int:
        with self.lock:
            return len(self.running)

    def close(self) -> None:
        if self.path is not None:
            try:
                os.remove(self.path)
            except OSError:
                pass


def answered(calls: Calls, line: bytes) -> None:
    """A line from the server: the answer to a call ends it."""
    if b'"id"' not in line:
        return
    try:
        message = json.loads(line)
    except ValueError:
        return
    if isinstance(message, dict) and "id" in message and "method" not in message:
        calls.end(message["id"])


# What still reaches the server while the browser is held: the handshake and the lists of what it offers. Never a
# resource read (`browser://<id>/network`, `/console`, `/state`, `/cookies`: the page's traffic, its console and its
# state, a card payment's own request among them; fix review 11/11, R5), a resource list or a prompt.
HELD_PASS = {"initialize", "ping", "tools/list", "prompts/list"}


def refused_while_held(message: dict) -> bytes:
    """A request other than a tool call, while held: a JSON-RPC error."""
    return (json.dumps({"jsonrpc": "2.0", "id": message.get("id"),
                        "error": {"code": -32002, "message": PAYING if paying() else WAIT}}) + "\n").encode()


def refusal(message: dict, text: str | None = None) -> bytes:
    text = text or (PAYING if paying() else WAIT)
    return (json.dumps({"jsonrpc": "2.0", "id": message.get("id"),
                        "result": {"content": [{"type": "text", "text": text}], "isError": True}}) + "\n").encode()


def main(command: list, calls: Calls | None = None) -> int:
    child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    out = sys.stdout.buffer
    lock = threading.Lock()
    calls = calls or Calls()

    def answers() -> None:
        assert child.stdout is not None
        for line in child.stdout:
            with lock:
                out.write(line)
                out.flush()
            answered(calls, line)

    reader = threading.Thread(target=answers, daemon=True)
    reader.start()
    assert child.stdin is not None
    for line in sys.stdin.buffer:
        try:
            message = json.loads(line)
        except ValueError:
            message = None
        if isinstance(message, dict) and message.get("method") == "tools/call" and "id" in message:
            calls.start(message["id"])   # counted before the hold is read: the bridge never misses a call let through
            if not calls.counting:
                calls.end(message["id"])
                with lock:
                    out.write(refusal(message, UNCOUNTED))
                    out.flush()
                continue
            if held():
                calls.end(message["id"])
                with lock:
                    out.write(refusal(message))
                    out.flush()
                continue
        elif (isinstance(message, dict) and "id" in message and isinstance(message.get("method"), str)
              and message["method"] not in HELD_PASS and held()):
            with lock:
                out.write(refused_while_held(message))
                out.flush()
            continue
        elif isinstance(message, dict) and message.get("method") == "notifications/cancelled":
            calls.end((message.get("params") or {}).get("requestId"))   # the server drops it without an answer
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
    calls.close()
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
