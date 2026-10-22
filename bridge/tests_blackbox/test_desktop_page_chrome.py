"""The page view (`/desktop/<ticket>/page/…`) against a real Chromium: its screencast reaches the phone as JPEG, taking
over renders the tab as on a phone (its width, a mobile user agent, touch), and handing back renders it as before.

Needs a Chromium on this machine (`BIP_CHROME`, else Chrome, Chromium or Brave where they usually are): skipped
without one. Here the bridge talks to Chromium's DevTools directly (no container, no relay)."""
import json
import subprocess
import tempfile
import time

import httpx
import pytest
from websockets.sync.client import connect

from bbkit.common import AUTH
from bbkit.servers import free_port, wait_until
from bbkit.ws import open_ws
from test_card_payments_chrome import CHROME
from test_desktop_page import browser_bridge, recv_until

pytestmark = pytest.mark.skipif(CHROME is None, reason="no Chromium on this machine")
PAGE = "data:text/html,<meta name=viewport content='width=device-width'><h1>Mairie</h1>"


@pytest.fixture
def chromium():
    port = free_port()
    profile = tempfile.mkdtemp(prefix="bip-chrome-")
    proc = subprocess.Popen([CHROME, "--headless=new", f"--remote-debugging-port={port}", f"--user-data-dir={profile}",
                             "--no-first-run", "--no-default-browser-check", "--window-size=1366,768", PAGE],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.time() + 20
    while time.time() < deadline:
        try:
            if any(t["type"] == "page" for t in httpx.get(f"http://127.0.0.1:{port}/json/list").json()):
                break
        except (httpx.HTTPError, ValueError):
            pass
        time.sleep(0.2)
    yield port
    proc.terminate()
    proc.wait(timeout=10)


def looks(port):
    """The page as its scripts see it: width, user agent, touch."""
    tab = next(t for t in httpx.get(f"http://127.0.0.1:{port}/json/list").json() if t["type"] == "page")
    with connect(tab["webSocketDebuggerUrl"], max_size=None) as ws:
        ws.send(json.dumps({"id": 1, "method": "Runtime.evaluate", "params": {
            "expression": "[innerWidth, navigator.userAgent, 'ontouchstart' in window]", "returnByValue": True}}))
        while True:
            answer = json.loads(ws.recv(timeout=10))
            if answer.get("id") == 1:
                return answer["result"]["result"]["value"]


def test_a_real_chromium_watched_then_taken_over_as_a_phone(bridges, fakes, chromium):
    bridge, folder = browser_bridge(bridges, fakes, chromium)
    assert bridge.get("/v1/agents/vie/desktop/state", headers=AUTH).json()["state"] == "active"   # a data: page
    width, agent, touch = looks(chromium)
    assert width > 1000 and "Android" not in agent and touch is False
    data = bridge.post("/v1/agents/vie/desktop", headers=AUTH).json()
    with open_ws(bridge.ws_url("/" + data["page_path"] + "ws?mobile=1&w=390&h=700&dpr=2")) as ws:
        seen = recv_until(ws, lambda m: isinstance(m, bytes))
        assert seen[-1][:2] == b"\xff\xd8"                                  # a JPEG
        frame = [m for m in seen if isinstance(m, dict) and m.get("type") == "frame"][-1]
        assert frame["w"] > 1000
        bridge.post("/v1/agents/vie/desktop/control", headers=AUTH, json={"take": True})
        recv_until(ws, lambda m: isinstance(m, dict) and m.get("mobile") is True)
        assert wait_until(lambda: looks(chromium)[0] == 390)
        width, agent, touch = looks(chromium)
        assert "Android" in agent and "Mobile" in agent and touch is True
        seen = recv_until(ws, lambda m: isinstance(m, dict) and m.get("type") == "frame" and m["w"] == 390)
        bridge.post("/v1/agents/vie/desktop/control", headers=AUTH, json={"take": False})
        width, agent, touch = looks(chromium)
        assert width > 1000 and "Android" not in agent and touch is False   # as the agent left it
