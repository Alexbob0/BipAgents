"""bip-browser-mcp's guard on a payment card's details (card-guards, 14/11), against a fake shop's payment step in a
real Chromium, behind the stand-in for stealth-browser-mcp (fixtures/signup_guard/stealth_cdp.py).

On 14/11 Alex's agent (Vie, his own test card, pharmaciedesdrakkars.com) could not get the vault's card fill to work
on the shop's payment page; it asked him for the card's number through a secure-field card and typed it into the form
itself. The agent never types, pastes, chooses or scripts a card's number, expiry, security code or holder: its own
card form's fields (labels, names, ids in four languages, autocomplete tokens), a payment provider's frame on another
site (`fixtures/card_guard/card.html` on `localhost`, the shop on `127.0.0.1`), a card's number in any field; what slips
past is erased after the call; a card's number never comes back in what it reads. The order's other fields (an
address, a SIRET, a loyalty card) are typed as before. Each refusal says how payments go: pay_with_card, « Oui,
payer », else « Panier prêt ».

Needs a Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`): skipped without. Only Visa's published test number
is used."""
import functools
import http.server
import json
import os
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

import httpx
import pytest
from websockets.sync.client import connect

from bbkit.servers import free_port

HERE = Path(__file__).parent
FIXTURES = HERE / "fixtures" / "card_guard"
STAND_IN = HERE / "fixtures" / "signup_guard" / "stealth_cdp.py"
CHROME = os.environ.get("BIP_CHROME", "")
PROXY = os.environ.get("BIP_BROWSER_MCP", "")
pytestmark = pytest.mark.skipif(not (CHROME and os.path.exists(CHROME) and PROXY and os.path.exists(PROXY)),
                                reason="no Chromium or no bip-browser-mcp")

NUMBER = "4242 4242 4242 4242"   # Visa's published test number
CARD_REFUSED = "Not done: this is a payment card's field"
SCRIPT_CARD_REFUSED = "Not done: a script never writes or reads a payment card's field"
PROVIDER_SCRIPT_REFUSED = "Not done: a script never sends your browser to a payment service's page"
CARD_UNDONE = "Not done: that put a payment card's details into the page"
HIDDEN = "[card number hidden]"


def serve(folder):
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(folder))
    handler.log_message = lambda *a: None
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


class Session:
    """A Chromium on the shop's payment step, and the proxy over the stand-in server, as Hermes talks to it."""

    def __init__(self):
        self.dir = Path(tempfile.mkdtemp(prefix="bip-card-guard-"))
        self.shop, self.psp = serve(FIXTURES), serve(FIXTURES)
        self.url = (f"http://127.0.0.1:{self.shop.server_address[1]}/checkout.html"
                    f"?psp=http://localhost:{self.psp.server_address[1]}/card.html")
        self.port = free_port()
        profile = self.dir / "profile"
        profile.mkdir()
        self.chrome = subprocess.Popen([CHROME, "--headless=new", f"--remote-debugging-port={self.port}",
                                        f"--user-data-dir={profile}", "--no-first-run", "--disable-gpu",
                                        "--site-per-process", "about:blank"],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline, page = time.time() + 30, None
        while time.time() < deadline and page is None:
            try:
                page = next(t for t in httpx.get(f"http://127.0.0.1:{self.port}/json/list").json()
                            if t["type"] == "page" and not t["url"].startswith("chrome"))
            except (httpx.HTTPError, ValueError, StopIteration):
                time.sleep(0.3)
        assert page, "no Chromium page"
        self.page_ws = page["webSocketDebuggerUrl"]
        (self.dir / "credit.json").write_text(json.dumps({"state": "ok"}))
        config = {"start": ["true"], "ready": ["true"], "active": ["true"], "stop": ["true"],
                  "server": [sys.executable, str(STAND_IN), self.page_ws],
                  "cache": str(self.dir / "cache.json"), "busy": str(self.dir / "busy"),
                  "credit": str(self.dir / "credit.json"), "cdp_port": free_port(), "idle": 600}
        (self.dir / "config.json").write_text(json.dumps(config))
        self.proxy = subprocess.Popen([PROXY, str(self.dir / "config.json")], stdin=subprocess.PIPE,
                                      stdout=subprocess.PIPE, stderr=open(self.dir / "proxy.log", "wb"))
        self.answers = queue.Queue()
        threading.Thread(target=self._read, daemon=True).start()
        self.id = 0
        self.ask("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "hermes"}})
        self.ask("tools/list", {})

    def _read(self):
        for line in self.proxy.stdout:
            self.answers.put(json.loads(line))

    def ask(self, method, params):
        self.id += 1
        self.proxy.stdin.write((json.dumps({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params})
                                + "\n").encode())
        self.proxy.stdin.flush()
        while True:
            answer = self.answers.get(timeout=60)
            if answer.get("id") == self.id:
                return answer

    def call(self, name, **arguments):
        answer = self.ask("tools/call", {"name": name, "arguments": {"instance_id": "i", **arguments}})
        assert "error" not in answer, answer
        return answer["result"]

    def tool(self, name, **arguments):
        return "\n".join(c.get("text", "") for c in self.call(name, **arguments)["content"])

    def page(self, expression):
        """What the page holds now, read through DevTools beside the agent's server."""
        with connect(self.page_ws, max_size=None) as ws:
            ws.send(json.dumps({"id": 1, "method": "Runtime.evaluate",
                                "params": {"expression": expression, "returnByValue": True}}))
            while True:
                answer = json.loads(ws.recv(timeout=10))
                if answer.get("id") == 1:
                    return answer["result"]["result"].get("value")

    def provider(self, expression):
        """`expression` in the provider's frame (an out-of-process frame: its own target)."""
        browser = httpx.get(f"http://127.0.0.1:{self.port}/json/version").json()["webSocketDebuggerUrl"]
        with connect(browser, max_size=None) as ws:
            n = 0

            def send(method, params=None, session=None):
                nonlocal n
                n += 1
                ws.send(json.dumps({"id": n, "method": method, "params": params or {},
                                    **({"sessionId": session} if session else {})}))
                while True:
                    answer = json.loads(ws.recv(timeout=10))
                    if answer.get("id") == n:
                        return answer.get("result", {})

            frame = next(t for t in send("Target.getTargets")["targetInfos"]
                         if t["type"] == "iframe" and "card.html" in t["url"])
            session = send("Target.attachToTarget", {"targetId": frame["targetId"], "flatten": True})["sessionId"]
            got = send("Runtime.evaluate", {"expression": expression, "returnByValue": True}, session)
            return got["result"].get("value")

    def fresh(self):
        """The payment step loaded again, every field empty."""
        self.tool("navigate", url=self.url)
        deadline = time.time() + 10
        while time.time() < deadline:
            try:
                if self.provider("document.readyState") == "complete":
                    return
            except (StopIteration, KeyError):
                pass
            time.sleep(0.2)

    def close(self):
        self.proxy.stdin.close()
        self.proxy.wait(timeout=30)
        self.chrome.terminate()
        self.chrome.wait(timeout=30)
        self.shop.shutdown()
        self.psp.shutdown()
        shutil.rmtree(self.dir, ignore_errors=True)


@pytest.fixture(scope="module")
def browser():
    session = Session()
    yield session
    session.close()


def value(browser, field):
    return browser.page(f"document.getElementById({json.dumps(field)}).value")


def test_the_shops_own_card_fields_are_never_typed_into(browser):
    browser.fresh()
    for field, text in [("numero", NUMBER), ("numero", "4242"), ("crypto", "123"), ("titulaire", "Alex Martin")]:
        for tool in ("type_text", "paste_text"):
            said = browser.tool(tool, selector=f"#{field}", text=text)
            assert said.startswith(CARD_REFUSED) and value(browser, field) == "", (tool, field, said)
    # the guard reads an XPath as the real server does, and a key typed into a card field
    said = browser.tool("type_text", selector="//input[@id='crypto']", text="737")
    assert said.startswith(CARD_REFUSED) and value(browser, "crypto") == "", said
    said = browser.tool("press_key", selector="#numero", key="4")
    assert said.startswith(CARD_REFUSED) and value(browser, "numero") == "", said
    # the expiry's selects (« Mois », « Année » beside the card's number)
    for field in ("mois", "annee"):
        said = browser.tool("select_option", selector=f"#{field}", index=3)
        assert said.startswith(CARD_REFUSED) and value(browser, field) == "", (field, said)
    # what the agent reads says how payments go
    assert "pay_with_card" in said and "« Oui, payer »" in said and "« Panier prêt »" in said
    assert "never ask your person" in said


def test_a_card_number_is_never_typed_anywhere_and_the_orders_other_fields_are(browser):
    browser.fresh()
    said = browser.tool("type_text", selector="#notes", text=f"Carte {NUMBER} exp 12/30")
    assert said.startswith(CARD_REFUSED) and value(browser, "notes") == "", said
    for field, text in [("notes", "Sonner deux fois"), ("address", "1 rue de l'Essai"), ("siret", "73282932000074"),
                        ("fidelite", "2900012345678")]:
        said = browser.tool("type_text", selector=f"#{field}", text=text)
        assert value(browser, field) == text, (field, said)


def test_the_providers_frame_is_never_typed_into(browser):
    browser.fresh()
    # typing aimed at the provider's frame (its element): refused, nothing reaches its fields
    said = browser.tool("type_text", selector="#psp", text="12/30")
    assert said.startswith(CARD_REFUSED), said
    # the keyboard in the provider's card number (the frame focused): a key typed is refused
    browser.page("document.getElementById('psp').focus()")
    browser.provider("document.getElementById('number').focus()")
    said = browser.tool("press_key", key="4")
    assert said.startswith(CARD_REFUSED), said
    assert browser.provider("[...document.querySelectorAll('input')].map(e => e.value).join('')") == ""


def test_a_script_never_writes_reads_or_sends_a_cards_details(browser):
    browser.fresh()
    for script in [
        f"document.getElementById('notes').value = '{NUMBER}'",
        "document.querySelector('[name=numero]').setAttribute('value', '1'); document.getElementById('numero').value = '4'",
        "document.getElementById('crypto').value = '737'",
    ]:
        said = browser.tool("execute_script", script=script)
        assert said.startswith(SCRIPT_CARD_REFUSED) or said.startswith(CARD_UNDONE), (script, said)
    for field in ("notes", "numero", "crypto"):
        assert value(browser, field) == "", field
    # passed the look before (the number built in the page, the field by its rank): erased after
    said = browser.tool("execute_script", script="const d = ['4242', '4242', '4242', '4242'].join(''); "
                                                 "document.querySelectorAll('#order input')[0].value = d; return true")
    assert said.startswith(CARD_UNDONE) and value(browser, "notes") == "", said
    said = browser.tool("execute_script", script="document.getElementById('numero').value = ['4242', '42'].join(''); "
                                                 "return true")
    assert said.startswith(CARD_UNDONE) and value(browser, "numero") == "", said
    said = browser.tool("execute_script", script="document.getElementById('mois').selectedIndex = 5; return true")
    assert said.startswith(CARD_UNDONE) and value(browser, "mois") == "", said
    # never sent to a payment service's page by a script (only by the shop's own buttons)
    said = browser.tool("execute_script",
                        script="location.href = 'https://payment.cawl-solutions.fr/hostedcheckout/x'; return true")
    assert said.startswith(PROVIDER_SCRIPT_REFUSED), said
    assert browser.page("location.pathname") == "/checkout.html"
    # a card's number on the page never comes back in what the agent reads
    said = browser.tool("execute_script", script="return document.getElementById('info').innerText")
    assert HIDDEN in said and "4242 4242" not in said, said
    # ordinary scripts go on
    said = browser.tool("execute_script", script="document.getElementById('address').value = '2 rue du Test'; "
                                                 "return document.title")
    assert "Paiement" in said and value(browser, "address") == "2 rue du Test", said
