"""The card typed into real checkout pages (fixtures/checkout) in a real Chromium, through its DevTools protocol, the
way the bridge does in the agent's visible browser: a payment provider's frame on another site (an out-of-process
frame, its fields found by their autocomplete tokens, its formatting running as the card is typed), and a French form
without tokens whose expiry is two selects. Then the bridge clicks the pay button and gives the browser back.

Each payment page is first prepared the way an agent misled by a page could (security audit 2026-11-11, BV-1): its
own script in the page's main world (what the agent's `execute_script` runs), with card fields of its own found
first, a capturing listener and a copy of everything typed in `sessionStorage` and `localStorage`, another tab of the
same site. The bridge types into the page reloaded under the hold: nothing of the card ever reaches the copy, the
other tab is closed, and a payment given up leaves the tab on an empty page. A checkout that loses its card form when
reloaded, and a card form in a frame of an unknown site, end as a hand-over to the person, nothing typed.

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


# What an agent's `execute_script` could plant before pay_with_card: card fields of its own (found first), a listener
# copying every value typed or changed in the page to the site's storage (which outlives the page).
PLANT = """(() => {
  const copy = (e) => {
    const kept = JSON.parse(sessionStorage.getItem("stash") || "[]");
    kept.push(e.target.value);
    sessionStorage.setItem("stash", JSON.stringify(kept));
    localStorage.setItem("stash", JSON.stringify(kept));
  };
  for (const kind of ["input", "change", "blur", "keyup"]) document.addEventListener(kind, copy, true);
  const mine = document.createElement("div");
  mine.innerHTML = '<input id="mine-number" autocomplete="cc-number"><input id="mine-csc" autocomplete="cc-csc">';
  document.body.prepend(mine);
  // the copy works: what is typed now is kept
  const field = document.getElementById("mine-number");
  field.value = "marker";
  field.dispatchEvent(new Event("input", {bubbles: true}));
  return sessionStorage.getItem("stash");
})()"""
STASH = "JSON.stringify([sessionStorage.getItem('stash'), localStorage.getItem('stash')])"


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
                    answer = json.loads(ws.recv(timeout=10))   # a DevTools that never answers fails, never hangs
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

    def call(self, method, params=None, url=None):
        """One DevTools command, in the page whose address holds `url` when given (the browser's otherwise)."""
        with connect(self.ws_url, max_size=None) as ws:
            n = 0

            def send(method, params=None, session=None):
                nonlocal n
                n += 1
                ws.send(json.dumps({"id": n, "method": method, "params": params or {},
                                    **({"sessionId": session} if session else {})}))
                while True:
                    answer = json.loads(ws.recv(timeout=20))
                    if answer.get("id") == n:
                        assert "error" not in answer, answer
                        return answer.get("result", {})

            session = None
            if url is not None:
                pages = [t for t in send("Target.getTargets")["targetInfos"] if t["type"] == "page" and url in t["url"]]
                session = send("Target.attachToTarget", {"targetId": pages[0]["targetId"], "flatten": True})["sessionId"]
            return send(method, params, session)

    def run(self, expression, url):
        """`expression`'s value in the page's main world, as the agent's execute_script runs it."""
        got = self.call("Runtime.evaluate", {"expression": expression, "returnByValue": True, "awaitPromise": True}, url)
        return got["result"].get("value")

    def pages(self):
        return [t["url"] for t in self.call("Target.getTargets")["targetInfos"] if t["type"] == "page"]

    def open(self, url, wait=1.5):
        """The agent's tab (the only one, or the one given) sent to `url`."""
        self.call("Page.navigate", {"url": url}, url=self.pages()[0])
        time.sleep(wait)

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
        # The provider's frame is on another site: one of the card's sites here (a test page has no known provider).
        sites = ["127.0.0.1", "localhost"]
        assert bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": sites}).status_code == 201
        # The agent prepared the page, and opened another tab of the same site.
        assert browser.run(PLANT, "elements.html") == '["marker"]'
        browser.call("Target.createTarget", {"url": f"http://127.0.0.1:{shop.server_port}/done.html?other",
                                             "background": True})
        time.sleep(1)
        targets = browser.call("Target.getTargets")["targetInfos"]
        agents_tab = next(t["targetId"] for t in targets if t["type"] == "page" and "elements.html" in t["url"])
        browser.call("Target.activateTarget", {"targetId": agents_tab})   # the agent's tab is the one shown
        assert len(browser.pages()) == 2
        result, asked = pay(bridge, ARGS)
        assert result["isError"] is False, result
        assert " on 127.0.0.1 with card " in asked
        assert [u.split("?")[0].split("/")[-1] for u in browser.pages()] == ["elements.html"]   # the other tab closed
        values = browser.values()
        # Typed into the page reloaded under the hold: the agent's own fields are gone with the old document.
        assert values["elements.html"] == {"holder": "Alex Martin", "ship": ""}    # the shipping postcode left alone
        assert values["provider.html"] == {"number": "4242 4242 4242 4242", "exp": "12 / 30", "cvc": CODE,
                                           "zip": "75011"}
        confirmed = tool(bridge, "confirm_payment", {"button": "Payer 12,50 €"})
        assert "moved on" in confirmed["content"][0]["text"], confirmed
        assert list(browser.values()) == ["done.html"]
        # The agent looks at what its script kept: only what it typed itself, before the payment.
        assert json.loads(browser.run(STASH, "done.html")) == ['["marker"]', '["marker"]']
        assert NUMBER[-4:] not in json.dumps(browser.values()) and CODE not in json.dumps(browser.values())
        # « Back » never brings the page the card was typed into again (fix review 11/11, B2).
        assert back_finds_nothing(browser, "done.html")
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
        assert browser.run(PLANT, "selects.html") == '["marker"]'
        result, _ = pay(bridge, ARGS)
        assert result["isError"] is False, result
        assert browser.values()["selects.html"] == {"numero": NUMBER, "mois": "12", "annee": "2030", "crypto": CODE,
                                                    "titulaire": "Alex Martin", "nom": ""}
        # Given up: the card erased, the tab left on an empty page, the browser given back.
        assert tool(bridge, "cancel_payment")["content"][0]["text"].startswith("The payment is given up")
        assert browser.pages() == ["about:blank"]
        assert back_finds_nothing(browser, "about:blank")
        # The agent goes back to the page and looks at what its script kept: only what it typed itself.
        browser.open(f"http://127.0.0.1:{shop.server_port}/selects.html")
        assert json.loads(browser.run(STASH, "selects.html")) == ['["marker"]', '["marker"]']
        values = browser.values()["selects.html"]
        assert values["numero"] == "" and values["crypto"] == "" and values["titulaire"] == ""
    finally:
        browser.stop()
        shop.shutdown()


def back_finds_nothing(browser, shown):
    """The tab on `shown` has no history before it: « back » (the agent's go_back) stays there, and no document the
    browser shows holds the card."""
    if len(browser.call("Page.getNavigationHistory", {}, url=shown)["entries"]) != 1:
        return False
    browser.run("history.back()", shown)
    time.sleep(1.5)
    values = json.dumps(browser.values())
    return (len(browser.pages()) == 1 and shown in browser.pages()[0]
            and NUMBER[-4:] not in values and "4242 4242" not in values and CODE not in values)


def test_confirm_presses_the_card_form_s_button_never_a_link_and_back_brings_nothing(bridges, fakes):
    # Fix review 11/11, B2: confirm_payment(button="Aide") used to click the page's « Aide » link: the tab went to a
    # new document, kept as the shop's answer (not blanked), and the agent's go_back brought the filled form again.
    shop = site()
    browser = Browser(f"http://127.0.0.1:{shop.server_port}/selects.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": ["127.0.0.1"]})
        result, _ = pay(bridge, ARGS)
        assert result["isError"] is False, result
        assert browser.values()["selects.html"]["numero"] == NUMBER
        confirmed = tool(bridge, "confirm_payment", {"button": "Aide"})["content"][0]["text"]
        # Not the link: the form's own submit button (the only button of the card's form).
        assert "« valider le paiement »" in confirmed and "moved on" in confirmed, confirmed
        assert [u.split("/")[-1] for u in browser.pages()] == ["done.html"]
        assert back_finds_nothing(browser, "done.html")
    finally:
        browser.stop()
        shop.shutdown()


def test_a_checkout_that_loses_its_form_when_reloaded_is_handed_over(bridges, fakes):
    shop = site()
    browser = Browser(f"http://127.0.0.1:{shop.server_port}/steps.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": ["127.0.0.1"]})
        browser.run("document.getElementById('next').click()", "steps.html")   # the agent went on to the payment step
        result, asked = pay(bridge, ARGS)
        assert asked and result["isError"] is True, result
        text = result["content"][0]["text"]
        assert "without its card form" in text and "hand_over_payment" in text
        assert browser.values()["steps.html"] == {"numero": "", "cvc": ""}
        assert browser.run("document.getElementById('card').hidden", "steps.html") is True   # reloaded, nothing typed
        assert bridge.commands.of("podman")[-1][-1].endswith("rm -f /tmp/bipagents-hold; exit 0")
    finally:
        browser.stop()
        shop.shutdown()


def test_a_card_form_in_a_frame_of_an_unknown_site_is_never_typed_into(bridges, fakes):
    shop, provider = site(), site()
    url = (f"http://127.0.0.1:{shop.server_port}/elements.html"
           f"?psp=http://localhost:{provider.server_port}/provider.html")
    browser = Browser(url)
    try:
        bridge = pay_bridge(bridges, fakes, browser)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": ["127.0.0.1"]})   # not the frame's site
        result, asked = pay(bridge, ARGS)
        assert asked is None and result["isError"] is True, result
        assert "hand_over_payment" in result["content"][0]["text"]
        values = browser.values()
        assert values["provider.html"] == {"number": "", "exp": "", "cvc": "", "zip": ""}
        assert values["elements.html"] == {"holder": "", "ship": ""}
    finally:
        browser.stop()
        shop.shutdown()
        provider.shutdown()
