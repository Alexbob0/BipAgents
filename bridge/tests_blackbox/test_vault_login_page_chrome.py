"""`vault_login`'s script (bridge-rs/assets/login_fill.js, `detectLogin`) on real pages in a real Chromium, run the way
the bridge runs it (login_fill.rs `find_login`, `fill_identifier`, `fill`): in an isolated world of the page's frame,
the identifier then the password typed with DevTools' `Input.insertText` into the fields it picked, each focused
first. Then the form is sent, and what the site received is checked.

The extranet page mirrors the one Alex could not log in to (16/11): `<input name="login" type="text">` without id nor
label, `<input name="mdp" id="mdp" type="password">`, a relative action. Fake values only.

Needs a Chromium on this machine (`BIP_CHROME`, else Chrome, Chromium or Brave where they usually are): skipped
without one."""
import functools
import http.server
import json
import os
import shutil
import subprocess
import tempfile
import threading
import time
import urllib.parse
from pathlib import Path

import httpx
import pytest
from websockets.sync.client import connect

from bbkit.servers import free_port

HERE = Path(__file__).parent
FIXTURES = HERE / "fixtures" / "vault_login"
SCRIPT = (HERE.parent.parent / "bridge-rs" / "assets" / "login_fill.js").read_text()
CANDIDATES = [os.environ.get("BIP_CHROME", ""),
              "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
              "/Applications/Chromium.app/Contents/MacOS/Chromium",
              "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
              shutil.which("chromium") or "", shutil.which("google-chrome") or ""]
CHROME = next((c for c in CANDIDATES if c and os.path.exists(c)), None)
pytestmark = pytest.mark.skipif(CHROME is None, reason="no Chromium on this machine")

IDENTIFIER = "agence-a@example.org"
PASSWORD = "Fake-A-pw-7781!"


class Site(http.server.SimpleHTTPRequestHandler):
    posted = []

    def log_message(self, *args):
        pass

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0)).decode()
        Site.posted.append((self.path, urllib.parse.parse_qs(body)))
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"<p>ok</p>")


class Browser:
    def __init__(self):
        self.dir = Path(tempfile.mkdtemp(prefix="bip-vault-login-"))
        handler = functools.partial(Site, directory=str(FIXTURES))
        self.site = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        threading.Thread(target=self.site.serve_forever, daemon=True).start()
        self.base = f"http://127.0.0.1:{self.site.server_address[1]}"
        port = free_port()
        self.chrome = subprocess.Popen([CHROME, "--headless=new", f"--remote-debugging-port={port}",
                                        f"--user-data-dir={self.dir / 'profile'}", "--no-first-run", "--disable-gpu",
                                        "about:blank"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline, page = time.time() + 30, None
        while time.time() < deadline and page is None:
            try:
                page = next(t for t in httpx.get(f"http://127.0.0.1:{port}/json/list").json() if t["type"] == "page")
            except (httpx.HTTPError, ValueError, StopIteration):
                time.sleep(0.3)
        assert page, "no Chromium page"
        self.ws = connect(page["webSocketDebuggerUrl"], max_size=None)
        self.id = 0
        self.call("Page.enable")

    def call(self, method, params=None):
        self.id += 1
        self.ws.send(json.dumps({"id": self.id, "method": method, "params": params or {}}))
        while True:
            message = json.loads(self.ws.recv())
            if message.get("id") == self.id:
                assert "error" not in message, message
                return message.get("result", {})

    def open(self, path):
        Site.posted.clear()
        self.call("Page.navigate", {"url": self.base + path})
        deadline = time.time() + 10
        while time.time() < deadline:
            if self.main("document.readyState") == "complete":
                break
            time.sleep(0.1)
        frame = self.call("Page.getFrameTree")["frameTree"]["frame"]["id"]
        world = self.call("Page.createIsolatedWorld", {"frameId": frame, "worldName": "bipagents-login"})
        self.context = world["executionContextId"]
        return self.isolated(f"{SCRIPT}\n;globalThis.__bipLogin.detectLogin()")

    def isolated(self, expression, by_value=True):
        result = self.call("Runtime.evaluate", {"expression": expression, "contextId": self.context,
                                                "returnByValue": by_value})
        return result["result"].get("value") if by_value else result["result"].get("objectId")

    def main(self, expression):
        return self.call("Runtime.evaluate", {"expression": expression, "returnByValue": True})["result"].get("value")

    def type_into(self, element_expression, prepare, text):
        """As login_fill.rs: prepared (focused, emptied), the element focused by DevTools, then typed."""
        assert self.isolated(prepare) is True
        handle = self.isolated(element_expression, by_value=False)
        self.call("DOM.focus", {"objectId": handle})
        self.call("Input.insertText", {"text": text})

    def close(self):
        self.ws.close()
        self.chrome.terminate()
        self.site.shutdown()
        shutil.rmtree(self.dir, ignore_errors=True)


@pytest.fixture
def browser():
    b = Browser()
    yield b
    b.close()


def test_the_extranet_form_gets_its_identifier_and_password(browser):
    found = browser.open("/V5/connexion-agence-a.html")
    assert found == {"focused": False, "password": True, "identifier": True, "count": 1}
    # the identifier field is « login » (not the newsletter's e-mail after the form), the password « mdp »
    assert browser.isolated("globalThis.__bipLogin.userEl().name") == "login"
    assert browser.isolated("globalThis.__bipLogin.el(0).id") == "mdp"
    browser.type_into("globalThis.__bipLogin.userEl()", f"globalThis.__bipLogin.prepareUser({json.dumps(IDENTIFIER)})",
                      IDENTIFIER)
    browser.type_into("globalThis.__bipLogin.el(0)", "globalThis.__bipLogin.prepare(0)", PASSWORD)
    assert browser.main("document.querySelector('input[name=login]').value") == IDENTIFIER
    assert browser.main("document.getElementById('mdp').value") == PASSWORD
    assert browser.main("document.querySelector('input[name=newsletter]').value") == ""
    # the identifier there already: not typed again
    assert browser.isolated(f"globalThis.__bipLogin.prepareUser({json.dumps(IDENTIFIER)})") is False
    # the form sent as the site expects it
    browser.main("document.querySelector('input[type=submit]').click()")
    deadline = time.time() + 10
    while time.time() < deadline and not Site.posted:
        time.sleep(0.1)
    [(path, fields)] = Site.posted
    assert path == "/login_externe.php"
    assert fields["login"] == [IDENTIFIER] and fields["mdp"] == [PASSWORD] and fields["groupe"] == ["g1", "g2"]


def test_a_first_step_asks_for_the_identifier_alone(browser):
    found = browser.open("/two-step.html")
    assert found == {"focused": False, "password": False, "identifier": True, "count": 0}
    assert browser.isolated("globalThis.__bipLogin.userEl().id") == "ap_email"


def test_a_sign_up_form_is_no_login_form(browser):
    found = browser.open("/signup.html")
    assert found["password"] is False and found["identifier"] is False


def test_an_unlabelled_form_its_only_field_before_the_password(browser):
    found = browser.open("/plain.html")
    assert found == {"focused": False, "password": True, "identifier": True, "count": 1}
    assert browser.isolated("globalThis.__bipLogin.userEl().name") == "f1"
    # the identifier setter never writes a password field
    assert browser.isolated("(() => { const p = globalThis.__bipLogin; p.user = p.el(0); return p.setUser('x'); })()") is False
    assert browser.main("document.querySelector('input[name=f2]').value") == ""
