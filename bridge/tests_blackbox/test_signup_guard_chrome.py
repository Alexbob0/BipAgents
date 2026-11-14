"""bip-browser-mcp's guard (fix-signup-hold, 13/11: no password typed by the agent, no terms ticked by it) against real
pages in a real Chromium: its checks before a call (the target script) and after it (what slipped past undone in the
page), run there, behind a stand-in for stealth-browser-mcp with its semantics (fixtures/signup_guard/stealth_cdp.py,
written for the branch's review). Each test says what should happen.

Needs a Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`, the branch's `bip-browser-mcp`): skipped without."""
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
FIXTURES = HERE / "fixtures" / "signup_guard"
CHROME = os.environ.get("BIP_CHROME", "")
PROXY = os.environ.get("BIP_BROWSER_MCP", "")
pytestmark = pytest.mark.skipif(not (CHROME and os.path.exists(CHROME) and PROXY and os.path.exists(PROXY)),
                                reason="no Chromium or no bip-browser-mcp")

PASSWORD_REFUSED = "Not done: this is a password field"
PASSWORD_UNDONE = "Not done: that put text into a password field"
SCRIPT_PASSWORD_REFUSED = "Not done: a script never writes or reads a password field"
TERMS_REFUSED = "Not done: this box accepts the site's terms"
TERMS_UNDONE = "Not done: that ticked the site's terms"


class Session:
    """A Chromium, the pages served, and the proxy over the stand-in server, as Hermes talks to it."""

    def __init__(self):
        self.dir = Path(tempfile.mkdtemp(prefix="bip-guard-"))
        site = self.dir / "site"
        shutil.copytree(FIXTURES, site)
        shutil.copy(HERE / "fixtures" / "signup" / "signup.html", site / "signup.html")
        handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(site))
        handler.log_message = lambda *a: None
        self.site = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        threading.Thread(target=self.site.serve_forever, daemon=True).start()
        self.base = f"http://127.0.0.1:{self.site.server_address[1]}"
        port = free_port()
        profile = self.dir / "profile"
        profile.mkdir()
        self.chrome = subprocess.Popen([CHROME, "--headless=new", f"--remote-debugging-port={port}",
                                        f"--user-data-dir={profile}", "--no-first-run", "--disable-gpu", "about:blank"],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline, page = time.time() + 30, None
        while time.time() < deadline and page is None:
            try:
                page = next(t for t in httpx.get(f"http://127.0.0.1:{port}/json/list").json() if t["type"] == "page")
            except (httpx.HTTPError, ValueError, StopIteration):
                time.sleep(0.3)
        assert page, "no Chromium page"
        self.page_ws = page["webSocketDebuggerUrl"]
        (self.dir / "credit.json").write_text(json.dumps({"state": "ok"}))
        config = {"start": ["true"], "ready": ["true"], "active": ["true"], "stop": ["true"],
                  "server": [sys.executable, str(site / "stealth_cdp.py"), self.page_ws],
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

    def tool(self, name, **arguments):
        answer = self.ask("tools/call", {"name": name, "arguments": {"instance_id": "i", **arguments}})
        assert "error" not in answer, answer
        return "\n".join(c.get("text", "") for c in answer["result"]["content"])

    def page(self, expression):
        """What the page holds now, read through DevTools beside the agent's server."""
        with connect(self.page_ws, max_size=None) as ws:
            ws.send(json.dumps({"id": 1, "method": "Runtime.evaluate",
                                "params": {"expression": expression, "returnByValue": True}}))
            while True:
                answer = json.loads(ws.recv(timeout=10))
                if answer.get("id") == 1:
                    return answer["result"]["result"].get("value")

    def close(self):
        self.proxy.stdin.close()
        self.proxy.wait(timeout=30)
        self.chrome.terminate()
        self.chrome.wait(timeout=30)
        self.site.shutdown()
        shutil.rmtree(self.dir, ignore_errors=True)


@pytest.fixture(scope="module")
def browser():
    session = Session()
    yield session
    session.close()


def test_a_delivery_address_field_kept_from_chromes_autofill_is_typed_into(browser):
    # autocomplete="new-password" on a text field (the usual way to keep Chrome's list off an address autocomplete) is
    # no password field: the agent fills the person's delivery address there
    browser.tool("navigate", url=f"{browser.base}/address.html")
    said = browser.tool("type_text", selector="#address", text="1 rue de l'Essai")
    assert browser.page("document.getElementById('address').value") == "1 rue de l'Essai", said


def test_a_consent_box_drawn_beside_its_text_is_never_ticked_by_the_agent(browser):
    # PrestaShop's sign-up consent: the real box hidden, a styled one drawn in its label; a click on the drawn one
    browser.tool("navigate", url=f"{browser.base}/terms.html")
    said = browser.tool("click_element", selector=".custom-checkbox span.box")
    ticked = browser.page("document.getElementById('psgdpr').checked")
    assert not ticked and said.startswith(TERMS_REFUSED), said


def test_the_terms_are_never_ticked_by_a_click_on_their_text(browser):
    # click_element with `text_match`: the real server clicks the element with that text, whatever the selector; the
    # check after the click unticks it
    browser.tool("navigate", url=f"{browser.base}/terms.html")
    said = browser.tool("click_element", selector="label", text_match="J'accepte les conditions générales")
    ticked = browser.page("document.getElementById('psgdpr').checked")
    assert not ticked and said.startswith(TERMS_UNDONE), said


def test_a_password_written_by_a_scripts_arguments_is_erased(browser):
    # execute_script runs `(function() { script })(args)`: the field and the password in `args`, unseen before it ran
    browser.tool("navigate", url=f"{browser.base}/signup.html")
    browser.tool("click_element", selector="#open-register")
    time.sleep(1)
    said = browser.tool("execute_script", script="const e = document.querySelector(arguments[0]); e.value = arguments[1];",
                        args=["#register-password", "Provisoire123!"])
    typed = browser.page("document.getElementById('register-password').value")
    # refused before it ran since the fifth review (the page's password field named in its args), else erased after
    assert typed == "" and (said.startswith(PASSWORD_UNDONE) or said.startswith(SCRIPT_PASSWORD_REFUSED)), (typed, said)


def test_a_password_field_rendered_a_moment_later_is_never_typed_into(browser):
    # signup.html: the sign-up panel's password fields come 600 ms after « Créer un compte »; the model's two calls in
    # one answer: open the panel, type into its password field
    browser.tool("navigate", url=f"{browser.base}/signup.html")
    browser.tool("click_element", selector="#open-register")
    said = browser.tool("type_text", selector="#register-password", text="Provisoire123!")
    typed = browser.page("document.getElementById('register-password').value")
    assert typed == "" and said.startswith(PASSWORD_REFUSED), (typed, said)


def test_a_script_setting_the_password_the_framework_way_is_refused(browser):
    browser.tool("navigate", url=f"{browser.base}/signup.html")
    browser.tool("click_element", selector="#open-register")
    time.sleep(1)
    script = ("const p = document.querySelector('#register-password'); "
              "const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; "
              "set.call(p, 'Provisoire123!'); p.dispatchEvent(new Event('input', {bubbles: true})); 'ok';")
    said = browser.tool("execute_script", script=script)
    typed = browser.page("document.getElementById('register-password').value")
    assert typed == "" and said.startswith(SCRIPT_PASSWORD_REFUSED), (typed, said)


def test_a_script_in_an_address_never_reaches_the_page(browser):
    # review of card-guards, 14/11: navigate with a `javascript:` address ran a script with none of the guard's checks
    # (navigate is no guarded tool): it wrote the password and ticked the terms, and the proxy said it was done
    browser.tool("navigate", url=f"{browser.base}/signup.html")
    browser.tool("click_element", selector="#open-register")
    time.sleep(1)
    for url in ["javascript:void(document.getElementById('register-password').value='Provisoire123!')",
                "  Java\tScript:void(document.getElementById('cgu').checked=true)"]:
        said = browser.tool("navigate", url=url)
        assert said.startswith("navigate refused") and "execute_script" in said, (url, said)
    time.sleep(0.5)
    assert browser.page("[document.getElementById('register-password').value, document.getElementById('cgu').checked, "
                        "location.pathname]") == ["", False, "/signup.html"]


def test_control_the_plain_cases_are_refused(browser):
    # the harness itself: what the branch does catch is caught here too (this one passes)
    browser.tool("navigate", url=f"{browser.base}/signup.html")
    browser.tool("click_element", selector="#open-register")
    time.sleep(1)
    assert browser.tool("type_text", selector="#register-password", text="Provisoire123!").startswith(PASSWORD_REFUSED)
    assert browser.tool("click_element", selector="#cgu").startswith(TERMS_REFUSED)
    assert browser.page("[document.getElementById('register-password').value, document.getElementById('cgu').checked]") \
        == ["", False]
    assert browser.tool("type_text", selector="#lastname", text="Testeur") == "true"
