import asyncio
import json
import os
import subprocess
import sys
import time

import pytest

from bipbridge import browser_mcp

FAKE_SERVER = r'''
import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        r = {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}},
             "serverInfo": {"name": "stealth", "version": "1"}}
    elif m["method"] == "tools/list":
        r = {"tools": [{"name": "spawn_browser", "inputSchema": {"type": "object"}}]}
    elif m["method"] == "tools/call":
        r = {"content": [{"type": "text", "text": "did " + m["params"]["name"]}]}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "error": {"code": -32601, "message": "no"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
'''

INIT = {"jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "hermes"}}}


class Out:
    def __init__(self):
        self.data = b""

    def write(self, data):
        self.data += data

    def flush(self):
        pass

    def messages(self):
        return [json.loads(line) for line in self.data.splitlines()]


@pytest.fixture
def config(tmp_path):
    (tmp_path / "server.py").write_text(FAKE_SERVER)
    log = tmp_path / "log"
    record = lambda word: ["sh", "-c", f"echo {word} >> {log}"]
    return {"start": ["sh", "-c", f"echo start >> {log}; touch {tmp_path / 'running'}"],
            "ready": ["true"],
            "active": ["test", "-e", str(tmp_path / "running")],
            "server": [sys.executable, str(tmp_path / "server.py")],
            "stop": ["sh", "-c", f"echo stop >> {log}; rm -f {tmp_path / 'running'}"],
            "cache": str(tmp_path / "cache.json"), "busy": str(tmp_path / "busy"), "cdp_port": 19602, "idle": 600}


def ran(tmp_path):
    try:
        return (tmp_path / "log").read_text().split()
    except OSError:
        return []


def test_hermes_connects_without_starting_the_browser_once_it_was_seen(config, tmp_path):
    async def scenario():
        # The first time ever: started to learn what it serves, then answered as it said.
        out = Out()
        proxy = browser_mcp.Proxy(config, out)
        await proxy.handle(INIT)
        await proxy.handle({"jsonrpc": "2.0", "method": "notifications/initialized"})
        await proxy.handle({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})
        await proxy.handle({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "navigate"}})
        first = out.messages()
        assert [m["id"] for m in first] == [0, 1, 2]
        assert first[0]["result"]["serverInfo"]["name"] == "stealth"
        assert first[1]["result"]["tools"][0]["name"] == "spawn_browser"
        assert first[2]["result"]["content"][0]["text"] == "did navigate"
        assert ran(tmp_path) == ["start"] and (tmp_path / "busy").exists()
        await proxy.stop_child()

        # Its gateway restarted: the handshake and the tool list come from what it said, nothing starts.
        (tmp_path / "log").unlink()
        out = Out()
        proxy = browser_mcp.Proxy(config, out)
        await proxy.handle(dict(INIT, id=10))
        await proxy.handle({"jsonrpc": "2.0", "id": 11, "method": "tools/list"})
        await proxy.handle({"jsonrpc": "2.0", "id": 12, "method": "ping"})
        await proxy.handle({"jsonrpc": "2.0", "id": 14, "method": "prompts/list"})   # never asked before: empty
        assert [m["id"] for m in out.messages()] == [10, 11, 12, 14] and ran(tmp_path) == [] and not proxy.running
        assert out.messages()[3]["result"] == {"prompts": []}
        assert out.messages()[1]["result"] == first[1]["result"]
        # The first tool call starts it.
        await proxy.handle({"jsonrpc": "2.0", "id": 13, "method": "tools/call", "params": {"name": "spawn_browser"}})
        assert out.messages()[-1] == {"jsonrpc": "2.0", "id": 13,
                                      "result": {"content": [{"type": "text", "text": "did spawn_browser"}]}}
        assert ran(tmp_path) == ["start"] and proxy.running
        await proxy.stop_child()

    asyncio.run(scenario())


def test_stopped_once_nobody_uses_it(config, tmp_path):
    async def scenario():
        config["idle"] = 0.4
        proxy = browser_mcp.Proxy(config, Out())
        await proxy.handle(INIT)
        watcher = asyncio.create_task(proxy.watch_idle(every=0.05))
        # The person watching its desktop (the bridge touching `busy`) keeps it on.
        for _ in range(8):
            (tmp_path / "busy").touch()
            await asyncio.sleep(0.1)
        assert proxy.running and "stop" not in ran(tmp_path)
        await asyncio.sleep(0.8)
        assert not proxy.running and ran(tmp_path) == ["start", "stop"]
        # Started by someone else (the desktop) and left unused: stopped too.
        (tmp_path / "running").touch()
        os.utime(tmp_path / "busy", (time.time() - 5, time.time() - 5))
        await asyncio.sleep(0.2)
        assert ran(tmp_path) == ["start", "stop", "stop"] and not (tmp_path / "running").exists()
        watcher.cancel()

    asyncio.run(scenario())


def test_a_long_call_or_a_devtools_connection_keeps_it_on(config, tmp_path, monkeypatch):
    table = tmp_path / "tcp"
    header = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n"
    # 127.0.0.1:47000 -> 127.0.0.1:19602 (0x4C92), established
    table.write_text(header + "   0: 0100007F:B798 0100007F:4C92 01 00000000:00000000 00:00000000 00000000  1000 0 1\n")
    monkeypatch.setattr(browser_mcp, "TCP_TABLES", (str(table),))
    assert browser_mcp.devtools_connected(19602) and not browser_mcp.devtools_connected(19604)
    table.write_text(header + "   0: 0100007F:B798 0100007F:4C92 06 00000000:00000000 00:00000000 00000000  1000 0 1\n")
    assert not browser_mcp.devtools_connected(19602)   # TIME_WAIT: gone

    async def scenario():
        config["idle"] = 0.3
        proxy = browser_mcp.Proxy(config, Out())
        await proxy.handle(INIT)
        proxy.calls = 1                       # a call under way
        os.utime(tmp_path / "busy", (time.time() - 60, time.time() - 60))
        watcher = asyncio.create_task(proxy.watch_idle(every=0.05))
        await asyncio.sleep(0.5)
        assert proxy.running and time.time() - (tmp_path / "busy").stat().st_mtime < 1   # touched for the others
        watcher.cancel()
        await proxy.stop_child()

    asyncio.run(scenario())


def test_a_browser_that_cannot_start_is_said_to_the_agent(config, tmp_path):
    config["start"] = ["false"]

    async def scenario():
        out = Out()
        (tmp_path / "cache.json").write_text(json.dumps({"initialize": {"serverInfo": {"name": "stealth"}}}))
        proxy = browser_mcp.Proxy(config, out)
        await proxy.handle(INIT)
        await proxy.handle({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "navigate"}})
        answer = out.messages()[-1]
        assert answer["id"] == 1 and "could not start" in answer["error"]["message"] and proxy.calls == 0

    asyncio.run(scenario())


def test_hermes_runs_it_over_stdio(config, tmp_path):
    path = tmp_path / "mcp.json"
    path.write_text(json.dumps(config))
    process = subprocess.Popen([sys.executable, browser_mcp.__file__, str(path)], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    lines = [INIT, {"jsonrpc": "2.0", "method": "notifications/initialized"},
             {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "take_screenshot"}}]
    out, err = process.communicate(b"".join(json.dumps(m).encode() + b"\n" for m in lines), timeout=30)
    answers = [json.loads(line) for line in out.splitlines()]
    assert [a["id"] for a in answers] == [0, 1] and answers[1]["result"]["content"][0]["text"] == "did take_screenshot"
    assert ran(tmp_path) == ["start"]   # Hermes gone: its server stops, the browser stays until unused
