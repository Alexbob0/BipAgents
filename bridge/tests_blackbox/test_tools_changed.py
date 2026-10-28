"""An agent sees new tools without a restart: `initialize` hands out an `Mcp-Session-Id`, the GET opens the session's
stream, and `notifications/tools/list_changed` comes down it when the agent's tools change (here: the phone's
calendar shared with it), once per new set and never while the set stays the same (the prompt cache)."""
import json
import queue
import threading
import time

import httpx

from bbkit.common import AUTH
from bbkit.mcp import give_token
from conftest import managed_install, profile, wait_registered

TOKEN = "coach-token-" + "s" * 30
PATH = "/v1/agent-tools/mcp"


def coach(bridges, fakes):
    bridge = bridges.new()
    profile(managed_install(bridge, fakes, agents=("coach",)), "coach")
    bridge.start()
    wait_registered(bridge, ["coach"])
    give_token(bridge, "coach", TOKEN)
    return bridge


def post(bridge, method, session=None, request_id=1, params=None):
    headers = {"Authorization": f"Bearer {TOKEN}", "Accept": "application/json, text/event-stream"}
    if session:
        headers["Mcp-Session-Id"] = session
    body = {"jsonrpc": "2.0", "method": method, "params": params or {}}
    if request_id is not None:
        body["id"] = request_id
    return bridge.post(PATH, json=body, headers=headers)


def tool_names(bridge, session):
    return [t["name"] for t in post(bridge, "tools/list", session).json()["result"]["tools"]]


def open_stream(bridge, session):
    """The session's GET stream read in a thread: each `data:` line's JSON (and None for a comment line) put in a
    queue. Returns (queue, stop)."""
    lines: "queue.Queue" = queue.Queue()
    stop = threading.Event()

    def read():
        headers = {"Authorization": f"Bearer {TOKEN}", "Accept": "text/event-stream", "Mcp-Session-Id": session}
        try:
            with httpx.stream("GET", bridge.url + PATH, headers=headers, timeout=httpx.Timeout(5.0, read=60.0)) as r:
                lines.put(("status", r.status_code))
                for line in r.iter_lines():
                    if stop.is_set():
                        return
                    if line.startswith("data:"):
                        lines.put(("data", json.loads(line[5:].strip())))
                    elif line.startswith(":"):
                        lines.put(("comment", line))
        except Exception as exc:  # noqa: BLE001 - the stream closed under us at the end
            lines.put(("error", repr(exc)))

    threading.Thread(target=read, daemon=True).start()
    return lines, stop


def next_data(lines, timeout):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            kind, value = lines.get(timeout=max(0.05, end - time.monotonic()))
        except queue.Empty:
            return None
        if kind == "data":
            return value
        assert kind != "error", value
    return None


def test_a_new_tool_is_announced_on_the_sessions_stream(bridges, fakes):
    bridge = coach(bridges, fakes)
    init = post(bridge, "initialize", params={"protocolVersion": "2025-11-25"})
    assert init.json()["result"]["capabilities"] == {"tools": {"listChanged": True}}
    session = init.headers["mcp-session-id"]
    assert session and post(bridge, "initialize").headers["mcp-session-id"] != session   # one per initialize
    assert post(bridge, "notifications/initialized", session, request_id=None).status_code == 202
    assert not any(n.startswith("calendar_") for n in tool_names(bridge, session))

    lines, stop = open_stream(bridge, session)
    assert lines.get(timeout=5) == ("status", 200)
    assert next_data(lines, 6.5) is None, "unchanged tools: nothing sent"

    assert bridge.put("/v1/calendar/agents", json={"agents": ["Coach"]}, headers=AUTH).json() == {"agents": ["coach"]}
    told = next_data(lines, 8)
    assert told == {"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}
    assert next_data(lines, 6) is None, "once per new set"
    give_token(bridge, "coach", TOKEN)   # its entry was written again (a new version), with a token of its own
    assert "calendar_list" in tool_names(bridge, session)
    assert next_data(lines, 6) is None, "listed: nothing more"

    # Another agent's session, or another token: refused; DELETE ends the session and its stream.
    assert bridge.get(PATH, headers={"Authorization": "Bearer nope", "Mcp-Session-Id": session}).status_code == 401
    assert bridge.delete(PATH, headers={"Authorization": f"Bearer {TOKEN}", "Mcp-Session-Id": session}).status_code == 200
    stop.set()
    # Without a session the endpoint answers as it always did.
    get = bridge.get(PATH, headers={"Authorization": f"Bearer {TOKEN}"})
    assert get.status_code == 405 and get.headers["allow"] == "POST"


def test_a_session_from_before_a_restart_is_taken_on(bridges, fakes):
    bridge = coach(bridges, fakes)
    # The client kept a session id the bridge never gave (it restarted): its URL's version is what it listed.
    version = "0000000000"
    headers = {"Authorization": f"Bearer {TOKEN}", "Accept": "text/event-stream", "Mcp-Session-Id": "kept-session"}
    with httpx.stream("GET", bridge.url + PATH + f"?v={version}", headers=headers,
                      timeout=httpx.Timeout(5.0, read=10.0)) as r:
        assert r.status_code == 200
        for line in r.iter_lines():
            if line.startswith("data:"):
                assert json.loads(line[5:]) == {"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}
                break
