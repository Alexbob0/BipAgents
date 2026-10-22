"""The card typed into real checkout pages (fixtures/checkout) in a real Chromium, through its DevTools protocol, the
way the bridge does in the agent's visible browser: a payment provider's frame on another site (an out-of-process
frame, its fields found by their autocomplete tokens, its formatting running as the card is typed), and a French form
without tokens whose expiry is two selects. Then the bridge clicks the pay button and gives the browser back.

Needs a Chromium on this machine (`BIP_CHROME`, else Chrome, Chromium or Brave where they usually are): skipped
without one. Only Visa's published test number is used."""
import functools
import http.server
import json
import os
import shutil
import subprocess
import tempfile
import threading
import time
from pathlib import Path

import httpx
import pytest
from websockets.sync.client import connect

from bbkit.common import AUTH
from bbkit.servers import free_port
from test_card_payments import ARGS, CARD, CODE, NUMBER, pay, pay_bridge, tool, turn_on

FIXTURES = Path(__file__).parent / "fixtures" / "checkout"
CANDIDATES = [os.environ.get("BIP_CHROME", ""),
              "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
              "/Applications/Chromium.app/Contents/MacOS/Chromium",
              "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
              shutil.which("chromium") or "", shutil.which("google-chrome") or ""]
CHROME = next((c for c in CANDIDATES if c and os.path.exists(c)), None)
pytestmark = pytest.mark.skipif(CHROME is None, reason="no Chromium on this machine")


def site():
    """The fixtures served on a port of their own."""
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(FIXTURES))
    handler.log_message = lambda *a: None
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


class Browser:
    def __init__(self, url):
        self.port = free_port()
        self.profile = tempfile.mkdtemp(prefix="bip-chrome-")
        self.proc = subprocess.Popen(
            [CHROME, "--headless=new", f"--remote-debugging-port={self.port}", f"--user-data-dir={self.profile}",
             "--no-first-run", "--no-default-browser-check", "--site-per-process", "--disable-gpu", url],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline = time.time() + 20
        while time.time() < deadline:
            try:
                self.ws_url = httpx.get(f"http://127.0.0.1:{self.port}/json/version").json()["webSocketDebuggerUrl"]
                break
            except (httpx.HTTPError, ValueError, KeyError):
                time.sleep(0.2)
        time.sleep(1.5)   # the page and its frames loaded

    def values(self):
        """Each input's value, by target (page, iframe), read through DevTools."""
        out = {}
        with connect(self.ws_url, max_size=None) as ws:
            n = 0

            def call(method, params=None, session=None):
                nonlocal n
                n += 1
                ws.send(json.dumps({"id": n, "method": method, "params": params or {},
                                    **({"sessionId": session} if session else {})}))
                while True:
                    answer = json.loads(ws.recv())
                    if answer.get("id") == n:
                        return answer.get("result", {})

            for target in call("Target.getTargets")["targetInfos"]:
                if target["type"] not in ("page", "iframe"):
                    continue
                session = call("Target.attachToTarget", {"targetId": target["targetId"], "flatten": True})["sessionId"]
                got = call("Runtime.evaluate", {"returnByValue": True, "expression":
                           "Object.fromEntries([...document.querySelectorAll('input,select')].map(e => [e.id, e.value]))"},
                           session)
                out[target["url"].split("?")[0].split("/")[-1]] = got["result"]["value"]
        return out

    def stop(self):
        self.proc.terminate()
        self.proc.wait(10)
        shutil.rmtree(self.profile, ignore_errors=True)


def test_a_providers_frame_on_another_site(bridges, fakes):
    shop, provider = site(), site()
    url = (f"http://127.0.0.1:{shop.server_port}/elements.html"
           f"?psp=http://localhost:{provider.server_port}/provider.html")
    browser = Browser(url)
    try:
        bridge = pay_bridge(bridges, fakes, browser)
        turn_on(bridge)
        assert bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": []}).status_code == 201
        result, asked = pay(bridge, ARGS)
        assert result["isError"] is False, result
        assert " on 127.0.0.1 with card " in asked
        values = browser.values()
        assert values["elements.html"] == {"holder": "Alex Martin", "ship": ""}    # the shipping postcode left alone
        assert values["provider.html"] == {"number": "4242 4242 4242 4242", "exp": "12 / 30", "cvc": CODE,
                                           "zip": "75011"}
        confirmed = tool(bridge, "confirm_payment", {"button": "Payer 12,50 €"})
        assert "moved on" in confirmed["content"][0]["text"], confirmed
        assert list(browser.values()) == ["done.html"]
    finally:
        browser.stop()
        shop.shutdown()
        provider.shutdown()


def test_a_french_form_with_selects(bridges, fakes):
    shop = site()
    browser = Browser(f"http://127.0.0.1:{shop.server_port}/selects.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": ["127.0.0.1"]})
        result, _ = pay(bridge, ARGS)
        assert result["isError"] is False, result
        assert browser.values()["selects.html"] == {"numero": NUMBER, "mois": "12", "annee": "2030", "crypto": CODE,
                                                    "titulaire": "Alex Martin", "nom": ""}
        # Given up: the card erased from the page, the browser given back.
        assert tool(bridge, "cancel_payment")["content"][0]["text"].startswith("The payment is given up")
        values = browser.values()["selects.html"]
        assert values["numero"] == "" and values["crypto"] == "" and values["titulaire"] == ""
    finally:
        browser.stop()
        shop.shutdown()
