"""« Lecture » and follow-zoom against a real Chromium: the page's text from Chrome's own DOM snapshot, and where the
page has its focus read in an isolated world, both without changing the page (its scripts see nothing: a counter of
DOM changes and of focus/blur events stays at zero).

Needs a Chromium on this machine (`BIP_CHROME`, else Chrome, Chromium or Brave where they usually are): skipped
without one. The bridge talks to Chromium's DevTools directly (no container, no relay)."""
import json
import subprocess
import tempfile
import time
import urllib.parse

import httpx
import pytest
from websockets.sync.client import connect

from bbkit.common import AUTH
from bbkit.servers import free_port
from test_card_payments_chrome import CHROME
from test_desktop_page import browser_bridge

pytestmark = pytest.mark.skipif(CHROME is None, reason="no Chromium on this machine")
HTML = """<html lang="fr"><body><nav><a href="/">Accueil</a> <a href="/menu">Menu</a></nav>
<main><h1>Horaires de la piscine</h1><p>Ouverte du <a href="/jours">lundi</a> au samedi, de 9 h à 20 h.</p>
<ul><li>Bassin sportif</li><li>Pataugeoire</li></ul><div style="display:none">caché</div>
<p>Inscription aux cours de natation pour les enfants de six à dix ans, chaque mercredi après-midi.</p>
<label>Nom <input id="name" style="margin-left:400px;width:200px"></label></main>
<script>window.changes = 0; window.events = 0;
new MutationObserver(m => { window.changes += m.length; }).observe(document, {subtree: true, childList: true, attributes: true, characterData: true});
for (const e of ["focusin", "focusout", "scroll", "resize"]) addEventListener(e, () => { window.events += 1; }, true);
document.getElementById("name").focus(); window.events = 0;</script></body></html>"""


@pytest.fixture
def chromium():
    port = free_port()
    profile = tempfile.mkdtemp(prefix="bip-chrome-")
    page = "data:text/html;charset=utf-8," + urllib.parse.quote(HTML)
    proc = subprocess.Popen([CHROME, "--headless=new", f"--remote-debugging-port={port}", f"--user-data-dir={profile}",
                             "--no-first-run", "--no-default-browser-check", "--window-size=1366,768", page],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.time() + 20
    while time.time() < deadline:
        try:
            if any(t["type"] == "page" and t["url"].startswith("data:")
                   for t in httpx.get(f"http://127.0.0.1:{port}/json/list").json()):
                break
        except (httpx.HTTPError, ValueError):
            pass
        time.sleep(0.2)
    time.sleep(0.5)
    page_eval(port, "window.changes = 0; window.events = 0")   # the browser's own resizes once the window settles
    yield port
    proc.terminate()
    proc.wait(timeout=10)


def page_eval(port, expression):
    tab = next(t for t in httpx.get(f"http://127.0.0.1:{port}/json/list").json() if t["type"] == "page")
    with connect(tab["webSocketDebuggerUrl"], max_size=None) as ws:
        ws.send(json.dumps({"id": 1, "method": "Runtime.evaluate", "params": {
            "expression": expression, "returnByValue": True}}))
        while True:
            answer = json.loads(ws.recv(timeout=10))
            if answer.get("id") == 1:
                return answer["result"]["result"].get("value")


def page_counters(port):
    """What the page's own scripts saw: DOM changes and focus/scroll/resize events."""
    return page_eval(port, "[window.changes, window.events, document.activeElement.id]")


def test_a_real_page_read_and_followed_without_being_touched(bridges, fakes, chromium):
    bridge, folder = browser_bridge(bridges, fakes, chromium)
    reader = bridge.get("/v1/agents/vie/desktop/reader", headers=AUTH)
    assert reader.status_code == 200, reader.text
    body = reader.json()
    assert body["lang"] == "fr"
    texts = [(b["t"], b["text"]) for b in body["blocks"]]
    assert texts[:4] == [("h", "Horaires de la piscine"), ("p", "Ouverte du lundi au samedi, de 9 h à 20 h."),
                         ("li", "Bassin sportif"), ("li", "Pataugeoire")]
    assert not any("caché" in t or "Accueil" in t or "MutationObserver" in t for _, t in texts)

    focus = bridge.get("/v1/agents/vie/desktop/focus", headers=AUTH)
    assert focus.status_code == 200, focus.text
    target = focus.json()["target"]
    assert target["kind"] == "field" and 400 < target["x"] < 600 and 190 < target["w"] < 215
    assert focus.json()["viewport"]["w"] > 1000
    assert page_counters(chromium) == [0, 0, "name"]      # nothing changed, no event, the focus where it was
