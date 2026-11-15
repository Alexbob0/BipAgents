"""Hotfix of 15/11 (card-guards on production, the seventh review: a reader set by a script read the card the person
typed once they took the agent's browser). Before the person takes it (« Payer dans le navigateur de l'agent », the
Bureau), the page is reloaded through DevTools when a script of the agent's ran on its site within the hour
(bip-browser-mcp writes each one down in scripts.jsonl, browser_mcp.rs `note_script`): what the script set up there is
gone before they type. A page of a site no script ran on is left as it is; when the reload can't be done, the browser
is not taken (409 « page_not_fresh »). A real Chromium; the bridge on its DevTools directly (no container, no relay)."""
import functools
import http.server
import json
import subprocess
import tempfile
import threading
import time
from pathlib import Path

import httpx
import pytest
from websockets.sync.client import connect

from bbkit.common import AUTH
from bbkit.servers import free_port, wait_until
from test_card_payments_chrome import CHROME, tab as is_tab
from test_desktop_page import browser_bridge

pytestmark = pytest.mark.skipif(CHROME is None, reason="no Chromium on this machine")
FIXTURES = Path(__file__).parent / "fixtures" / "card_guard"


@pytest.fixture
def shop():
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(FIXTURES))
    handler.log_message = lambda *a: None
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}"
    server.shutdown()


@pytest.fixture
def chromium(shop):
    port = free_port()
    profile = tempfile.mkdtemp(prefix="bip-chrome-")
    proc = subprocess.Popen([CHROME, "--headless=new", f"--remote-debugging-port={port}", f"--user-data-dir={profile}",
                             "--no-first-run", "--no-default-browser-check", "--window-size=1366,768",
                             shop + "/later-pay.html"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    assert wait_until(lambda: page(port, "document.readyState") == "complete", timeout=30)
    yield port
    proc.terminate()
    proc.wait(timeout=10)


def page(port, expression):
    """What the page's own world says (None while it can't)."""
    try:
        tab = next(t for t in httpx.get(f"http://127.0.0.1:{port}/json/list").json() if is_tab(t))
        with connect(tab["webSocketDebuggerUrl"], max_size=None) as ws:
            ws.send(json.dumps({"id": 1, "method": "Runtime.evaluate",
                                "params": {"expression": expression, "returnByValue": True}}))
            while True:
                answer = json.loads(ws.recv(timeout=10))
                if answer.get("id") == 1:
                    return answer.get("result", {}).get("result", {}).get("value")
    except (httpx.HTTPError, StopIteration, ValueError, OSError, TimeoutError):
        return None


def scripted(folder, origin, ago=0.0):
    """A script of the agent's written down on `origin`, `ago` seconds ago (as bip-browser-mcp's `note_script`)."""
    with open(folder / "scripts.jsonl", "a") as fh:
        fh.write(json.dumps({"t": time.time() - ago, "origin": origin}) + "\n")


def take(bridge, on):
    return bridge.post("/v1/agents/vie/desktop/control", headers=AUTH, json={"take": on})


def test_the_page_a_script_ran_on_is_reloaded_before_the_person_takes_the_browser(bridges, fakes, chromium, shop):
    bridge, folder = browser_bridge(bridges, fakes, chromium)
    plant = "window.__reader = 1; setInterval(() => {}, 100); 'ok'"
    # no script of the agent's: the page is left as it is
    assert page(chromium, plant) == "ok"
    answer = take(bridge, True)
    assert answer.status_code == 200 and answer.json()["you_hold"] is True, answer.text
    assert page(chromium, "window.__reader") == 1
    take(bridge, False)
    # a script on another site, or more than an hour ago: left as it is too
    scripted(folder, "https://other.example")
    scripted(folder, shop, ago=4000)
    assert take(bridge, True).status_code == 200
    assert page(chromium, "window.__reader") == 1
    take(bridge, False)
    # a script on this site within the hour: reloaded (what it set up gone) before the person's hand
    scripted(folder, shop)
    answer = take(bridge, True)
    assert answer.status_code == 200 and answer.json()["you_hold"] is True, answer.text
    assert page(chromium, "window.__reader") is None and page(chromium, "location.pathname") == "/later-pay.html"
    take(bridge, False)
    # taken again with no script since: the same fresh document, not reloaded again
    assert page(chromium, "window.__mark = 2") == 2
    assert take(bridge, True).status_code == 200
    assert page(chromium, "window.__mark") == 2
    take(bridge, False)
    # another script meanwhile (the page couldn't say where: « unknown », any site): reloaded again
    assert page(chromium, plant) == "ok"
    scripted(folder, "unknown")
    assert take(bridge, True).status_code == 200
    assert page(chromium, "window.__reader") is None
    take(bridge, False)


def test_not_taken_when_the_page_a_script_ran_on_cant_be_reloaded(bridges, fakes, shop):
    bridge, folder = browser_bridge(bridges, fakes, free_port())   # nothing answers on its DevTools
    scripted(folder, shop)
    answer = take(bridge, True)
    assert answer.status_code == 409 and answer.json()["detail"]["code"] == "page_not_fresh", answer.text
    assert "reloaded" in answer.json()["detail"]["text"], answer.text
    state = bridge.get("/v1/agents/vie/desktop/state", headers=AUTH).json()
    assert state["you_hold"] is False
    assert not (folder / "hold.token").exists()
    # handing back still answers
    assert take(bridge, False).status_code == 200
