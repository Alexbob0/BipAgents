"""vault-no-readback (Alex, 16/11): no tool of the agent's browser gives back the value of a password field, nor any
function of it: a field the vault filled (`vault_login`, Hermes' `browser_vault`), and what the person typed while
holding the browser. The person's own typing still goes through, and the site still gets the password.

Before: an agent misled by a page or an e-mail could read the field the vault filled with a script (V8's read-only
check lets a read through) and hand it out reversed, split, as character codes or base64, which Hermes' mask of the
exact value (patch 0013) does not catch; or read it from the page's own world (a framework's props or state), from
another frame's getter, from the « value » attribute a React page mirrors (the page readers, a selector testing it
one character at a time), or in clear once the site's eye has shown it (a screenshot, the accessibility tree).

A fake agencies' login page (fixtures/vault_readback/login.html) in a real Chromium, the vault's fill as the bridge
types it (bridge-rs/assets/login_fill.js, `detectLogin`, then DevTools' Input.insertText), behind bip-browser-mcp and
the stand-in for stealth-browser-mcp (fixtures/signup_guard/stealth_cdp.py). Then the 145 scripts real agents wrote
(bridge-rs/tests/rx_agent_scripts.json) on the sign-up bench page: each gives what V8 gives in the page's own world.

Needs a Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`): skipped without. Fake values only."""
import base64
import functools
import http.server
import json
import os
import queue
import re
import shutil
import subprocess
import sys
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
FIXTURES = HERE / "fixtures" / "vault_readback"
STAND_IN = HERE / "fixtures" / "signup_guard" / "stealth_cdp.py"
LOGIN_FILL = (HERE.parent.parent / "bridge-rs" / "assets" / "login_fill.js").read_text()
BENCH = json.loads((HERE.parent.parent / "bridge-rs" / "tests" / "rx_agent_scripts.json").read_text())
CHROME = os.environ.get("BIP_CHROME", "")
PROXY = os.environ.get("BIP_BROWSER_MCP", "")
pytestmark = pytest.mark.skipif(not (CHROME and os.path.exists(CHROME) and PROXY and os.path.exists(PROXY)),
                                reason="no Chromium or no bip-browser-mcp")

SECRET = "Zq7-Vault-pw-4471!"     # fake
IDENT = "agence-nord@example.org"
PIN = "4471-Pin-Zq9x"             # fake, typed by « the person »


def forms_of(secret):
    """What a script could make of the value to get past a mask of the exact bytes."""
    codes = [ord(c) for c in secret]
    return {
        "as is": secret, "its start": secret[:7], "its end": secret[-7:], "reversed": secret[::-1][:7],
        "spaced": " ".join(secret[:6]), "split": ",".join(secret[:6]), "listed": '","'.join(secret[:6]),
        "character codes": ",".join(map(str, codes[:5])), "codes spaced": " ".join(map(str, codes[:5])),
        "hex": "".join(f"{c:02x}" for c in codes[:6]), "base64": base64.b64encode(secret.encode()).decode()[:12],
        "url-encoded": urllib.parse.quote(secret, safe="")[:14],
    }


def leaks(text, secret=SECRET):
    """The forms of `secret` found in `text` (backslashes of JSON escaping dropped); the base64 one is also the copy the
    site itself keeps in a hidden field (fixtures/vault_readback/login.html `enc`)."""
    plain = text.replace("\\", "")
    return [name for name, form in forms_of(secret).items() if form in plain]


class Site(http.server.SimpleHTTPRequestHandler):
    posted = []

    def log_message(self, *args):
        pass

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0)).decode()
        Site.posted.append((self.path, urllib.parse.parse_qs(body)))
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers()
        self.wfile.write(b"<p>ok</p>")


class Cdp:
    """One DevTools connection to the page, beside the agent's tools (the vault, the person, the test's eyes)."""

    def __init__(self, ws_url):
        self.ws = connect(ws_url, max_size=None)
        self.n = 0

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.ws.close()

    def call(self, method, params=None):
        self.n += 1
        self.ws.send(json.dumps({"id": self.n, "method": method, "params": params or {}}))
        while True:
            answer = json.loads(self.ws.recv(timeout=20))
            if answer.get("id") == self.n:
                assert "error" not in answer, answer
                return answer.get("result", {})

    def value(self, expression, context=None):
        params = {"expression": expression, "returnByValue": True, **({"contextId": context} if context else {})}
        return self.call("Runtime.evaluate", params)["result"].get("value")


class Session:
    """A Chromium, the fake site, and the proxy over the stand-in server, as Hermes talks to it."""

    def __init__(self):
        self.dir = Path(tempfile.mkdtemp(prefix="bip-vault-readback-"))
        site = self.dir / "site"
        shutil.copytree(FIXTURES, site)
        shutil.copy(HERE / "fixtures" / "signup" / "signup.html", site / "signup.html")
        self.site = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Site, directory=str(site)))
        threading.Thread(target=self.site.serve_forever, daemon=True).start()
        self.base = f"http://127.0.0.1:{self.site.server_address[1]}"
        self.port = free_port()
        profile = self.dir / "profile"
        profile.mkdir()
        self.chrome = subprocess.Popen([CHROME, "--headless=new", f"--remote-debugging-port={self.port}",
                                        f"--user-data-dir={profile}", "--no-first-run", "--disable-gpu",
                                        "--site-per-process", "--window-size=1000,900", "about:blank"],
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
                  "credit": str(self.dir / "credit.json"), "idle": 600, "cdp_port": self.port}
        (self.dir / "config.json").write_text(json.dumps(config))
        self.log = self.dir / "proxy.log"
        self.proxy = subprocess.Popen([PROXY, str(self.dir / "config.json")], stdin=subprocess.PIPE,
                                      stdout=subprocess.PIPE, stderr=open(self.log, "wb"))
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
        """What the agent reads: every text block of the answer, and the structured copy Hermes may show too."""
        result = self.call(name, **arguments)
        texts = [c.get("text", "") for c in result.get("content", []) if c.get("type") == "text"]
        return "\n".join(texts + [json.dumps(result.get("structuredContent", ""))])

    def image(self, name, **arguments):
        result = self.call(name, **arguments)
        return next((c["data"] for c in result.get("content", []) if c.get("type") == "image"), None)

    def page(self, expression):
        """What the page holds now, read through DevTools beside the agent's tools."""
        with Cdp(self.page_ws) as cdp:
            return cdp.value(expression)

    def open(self, path):
        self.tool("navigate", url=self.base + path)
        deadline = time.time() + 10
        while time.time() < deadline and self.page("document.readyState") != "complete":
            time.sleep(0.1)
        Site.posted.clear()

    def vault_fills(self, password=SECRET, identifier=IDENT):
        """The vault's fill as login_fill.rs does it: `detectLogin` in an isolated world of the page, the identifier
        then the password each prepared, focused by DevTools and typed with Input.insertText."""
        with Cdp(self.page_ws) as cdp:
            frame = cdp.call("Page.getFrameTree")["frameTree"]["frame"]["id"]
            world = cdp.call("Page.createIsolatedWorld", {"frameId": frame, "worldName": "bipagents-login"})
            context = world["executionContextId"]
            found = cdp.value(f"{LOGIN_FILL}\n;globalThis.__bipLogin.detectLogin()", context)
            assert found["password"] is True, found
            for prepare, element, text in [
                (f"globalThis.__bipLogin.prepareUser({json.dumps(identifier)})", "globalThis.__bipLogin.userEl()",
                 identifier),
                ("globalThis.__bipLogin.prepare(0)", "globalThis.__bipLogin.el(0)", password),
            ]:
                if not found.get("identifier") and "User" in prepare:
                    continue
                assert cdp.value(prepare, context) is True
                handle = cdp.call("Runtime.evaluate", {"expression": element, "contextId": context})["result"]
                cdp.call("DOM.focus", {"objectId": handle["objectId"]})
                cdp.call("Input.insertText", {"text": text})
            cdp.value("document.activeElement && document.activeElement.blur()")

    def person_types(self, selector, text):
        """Typed by the person holding the browser (a key at a time, as the live view sends them)."""
        with Cdp(self.page_ws) as cdp:
            cdp.value(f"document.querySelector({json.dumps(selector)}).focus()")
            for char in text:
                cdp.call("Input.insertText", {"text": char})
            cdp.value("document.activeElement && document.activeElement.blur()")

    def accessibility_values(self):
        with Cdp(self.page_ws) as cdp:
            nodes = cdp.call("Accessibility.getFullAXTree")["nodes"]
        return [json.dumps(n.get("value", {})) for n in nodes if n.get("role", {}).get("value") in
                ("textbox", "searchbox", "TextField")]

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


@pytest.fixture
def filled(browser):
    """The login page, its identifier and password filled by the vault."""
    browser.open("/login.html")
    browser.vault_fills()
    assert browser.page("document.querySelector('#mdp').value") == SECRET
    assert browser.page("document.querySelector('#mdp').getAttribute('value')") == SECRET   # as React mirrors it
    return browser


# The password field is the 4th input of the form (csrf, enc, login, mdp): named so, the text rules of 13/11 don't
# see a password field, nor a value read (`['val' + 'ue']`).
FIELD = "document.querySelector('form input:nth-of-type(4)')"
VALUE = FIELD + "['val' + 'ue']"
BASE64 = ("(() => { const s = " + VALUE + "; const A = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'; "
          "let o = ''; for (let i = 0; i < s.length; i += 3) { const n = (s.charCodeAt(i) << 16) | ((s.charCodeAt(i + 1) "
          "|| 0) << 8) | (s.charCodeAt(i + 2) || 0); o += A[(n >> 18) & 63] + A[(n >> 12) & 63] + (i + 1 < s.length ? "
          "A[(n >> 6) & 63] : '=') + (i + 2 < s.length ? A[n & 63] : '='); } return o; })()")
REVERSED = "String({}).split('').reverse().join('')"
READS = {
    # the plain ways: the text rules refuse them, or the field reads as empty
    "its value": "document.querySelector('input[type=password]').value",
    "by its id": "document.querySelector('#mdp').value",
    "the form's values": "[...document.querySelector('#login').elements].map((e) => e.name + '=' + e.value).join('&')",
    "FormData": "[...new FormData(document.querySelector('#login'))].map((p) => p.join('=')).join('&')",
    "URLSearchParams": "new URLSearchParams(new FormData(document.querySelector('#login'))).toString()",
    # past the text rules (and past a mask of the exact value: each one coded)
    "named otherwise": VALUE,
    "reversed": VALUE + ".split('').reverse().join('')",
    "split": VALUE + ".split('').join(' ')",
    "character codes": "[..." + VALUE + "].map((c) => c.charCodeAt(0)).join(',')",
    "hex": "[..." + VALUE + "].map((c) => c.charCodeAt(0).toString(16).padStart(2, '0')).join('')",
    "base64": BASE64,
    "its attribute": REVERSED.format(FIELD + ".getAttribute('val' + 'ue')"),
    "its default": REVERSED.format(FIELD + "['default' + 'Value']"),
    "the form's HTML": REVERSED.format("document.querySelector('form').innerHTML"),
    "a selector testing it": "document.querySelectorAll('[value^=\"Zq7\"]').length + ' ' + "
                             "document.querySelectorAll('[value*=\"4471\"]').length",
    "the framework's props": REVERSED.format("(() => { const e = " + FIELD + "; const k = Object.keys(e).find((x) => "
                                             "x.startsWith('__react')); return k && e[k]['val' + 'ue']; })()"),
    "the page's state": REVERSED.format("JSON.stringify(window.__state)"),
    "another frame's getter": REVERSED.format("Object.getOwnPropertyDescriptor(frames[0].HTMLInputElement.prototype, "
                                              "'val' + 'ue').get.call(" + FIELD + ")"),
    "the site's shown copy": REVERSED.format("document.querySelector('[name=mdp_shown]')['val' + 'ue']"),
    "the site's encoded copy": "document.querySelector('[name=enc]')['val' + 'ue']",
    "its selection": "(() => { const e = " + FIELD + "; return e.selectionStart + ':' + e.selectionEnd; })()",
}


@pytest.mark.parametrize("how", list(READS))
def test_a_script_gets_nothing_of_the_filled_password(filled, how):
    said = filled.tool("execute_script", script=READS[how])
    assert not leaks(said), f"{how}: the agent read {leaks(said)} in {said[:200]!r}"
    # not even its length (18), nor whether it starts so
    assert not re.search(r"\b18\b", said), f"{how}: its length in {said[:200]!r}"
    if how == "a selector testing it":
        assert said.startswith("Not done") or '"0 0"' in said, said[:200]
    # the field and the site's copies are as the vault left them (the attribute the page mirrored may be emptied:
    # the field keeps its value, and that is what the form sends)
    assert filled.page("document.querySelector('#mdp').value") == SECRET
    assert filled.page("document.querySelector('#mdp').getAttribute('value')") in (SECRET, "")
    assert filled.page("document.querySelector('#mirror').value") == SECRET
    assert filled.page("atob(document.querySelector('#enc').value)") == SECRET
    assert filled.page("window.__state.password") == SECRET


def test_a_script_never_puts_it_where_the_agent_reads(filled):
    for script in ["document.title = " + VALUE,
                   "document.querySelector('#info').setAttribute('data-x', " + VALUE + ")",
                   "document.querySelector('#q')['val' + 'ue'] = " + VALUE]:
        said = filled.tool("execute_script", script=script)
        assert said.startswith("Not done"), (script, said[:200])
    assert filled.page("document.title") == "Espace agences | Essai"
    assert filled.page("document.querySelector('#info').getAttribute('data-x')") is None
    assert filled.page("document.querySelector('#q').value") == ""


def test_the_sites_eye_shows_it_to_nobody_but_the_person(filled):
    filled.tool("click_element", selector="#eye")
    assert filled.page("document.querySelector('#mdp').type") == "text"   # the site showed it (the live view)
    said = filled.tool("execute_script", script=VALUE + ".split('').reverse().join('')")
    assert not leaks(said), said[:200]
    # read again as dots before the agent looks: the screenshot, the accessibility tree
    shot = filled.image("take_screenshot")
    assert shot
    assert filled.page("document.querySelector('#mdp').type") == "password"
    assert not [v for v in filled.accessibility_values() if leaks(v)]
    with Cdp(filled.page_ws) as cdp:
        masked = cdp.call("Page.captureScreenshot", {"format": "jpeg"})["data"]
        cdp.value("document.querySelector('#mdp').type = 'text'")
        clear = cdp.call("Page.captureScreenshot", {"format": "jpeg"})["data"]
        cdp.value("document.querySelector('#mdp').type = 'password'")
    assert shot == masked and shot != clear, "the agent's screenshot is not the masked page"
    # the eye again, then the page's readers
    filled.tool("click_element", selector="#eye")
    for tool, args in [("query_elements", {"selector": "input"}), ("get_element_state", {"selector": "#mdp"}),
                       ("get_page_content", {}), ("get_page_content", {"include_frames": True})]:
        said = filled.tool(tool, **args)
        assert not leaks(said), (tool, leaks(said), said[:300])
    assert filled.page("document.querySelector('#mdp').value") == SECRET


def test_a_copy_in_another_field_reads_as_empty(filled):
    filled.person_types("#q", SECRET)          # pasted by someone into the search box
    filled.person_types("#notes", SECRET)      # into a text area
    with Cdp(filled.page_ws) as cdp:           # into a field of the page's own frame
        cdp.value(f"frames[0].document.querySelector('#inner').value = {json.dumps(SECRET)}")
    for script in ["document.querySelector('#q')['val' + 'ue']",
                   "document.querySelector('textarea')['val' + 'ue'].split('').reverse().join('')",
                   "frames[0].document.querySelector('input')['val' + 'ue']"]:
        said = filled.tool("execute_script", script=script)
        assert not leaks(said), (script, said[:200])
    said = filled.tool("get_element_state", selector="#q")
    assert not leaks(said), said[:300]
    assert filled.page("document.querySelector('#q').value") == SECRET   # left where it was


def test_the_page_readers_and_their_selectors(filled):
    for tool, args in [("get_page_content", {}), ("get_page_content", {"include_frames": True}),
                       ("query_elements", {"selector": "input"}), ("query_elements", {"selector": "#login *"}),
                       ("get_element_state", {"selector": "#mdp"}),
                       ("get_element_state", {"selector": "form input:nth-of-type(4)"})]:
        said = filled.tool(tool, **args)
        assert not leaks(said), (tool, args, leaks(said), said[:300])
    # a selector testing the field's value attribute (part of it: one character at a time, the agent would read it
    # all), or the whole of it
    for selector in [f'input[value="{SECRET}"]', 'input[value^="Zq7"]', "input[value*='4471']", 'input[ value $= "1!" ]', r'input[valu\65 ^="Zq"]',
                     '[VALUE^="Zq7"]', "//input[starts-with(@value,'Zq7')]", "//input[contains(@value,'4471')]",
                     "//input[substring(@value,1,1)='Z']", "//*[@*[starts-with(.,'Zq7')]]"]:
        for tool, args in [("query_elements", {}), ("wait_for_element", {"timeout": 300}),
                           ("click_element", {"timeout": 300}), ("get_element_state", {})]:
            said = filled.tool(tool, selector=selector, **args)
            found = '"tag_name"' in said or said.startswith("true") or '"result": true' in said
            assert not found, (tool, selector, said[:200])
    # what a page reads normally still comes back, a field by its own value attribute included
    said = filled.tool("query_elements", selector="input[name=login]")
    assert '"tag_name"' in said and '"autocomplete": "username"' in said, said[:300]
    said = filled.tool("query_elements", selector="input[value='c5rf-t0ken-3361']")
    assert '"name": "csrf"' in said, said[:300]


def test_the_accessibility_tree_has_only_dots(filled):
    values = filled.accessibility_values()
    assert values and not [v for v in values if leaks(v)], values


def test_a_script_never_sends_the_form_elsewhere_and_the_site_gets_the_password(filled):
    for script in ["document.querySelector('form').action = 'http://localhost:9/steal'",
                   "document.querySelector('form').setAttribute('action', 'http://localhost:9/steal')",
                   "document.querySelector('#go').formAction = 'http://localhost:9/steal'",
                   "document.querySelector('form').method = 'get'"]:
        said = filled.tool("execute_script", script=script)
        assert said.startswith("Not done"), (script, said[:200])
    assert filled.page("document.querySelector('form').getAttribute('action')") == "/session"
    assert filled.page("document.querySelector('form').method") == "post"
    assert filled.page("document.querySelector('#go').getAttribute('formaction')") is None
    # the agent's scripts ran on it: the site still gets what the vault typed
    filled.tool("execute_script", script=VALUE + ".split('').reverse().join('')")
    filled.tool("select_option", selector="#agence", value="a2")
    filled.tool("click_element", selector="#go")
    deadline = time.time() + 10
    while time.time() < deadline and not Site.posted:
        time.sleep(0.1)
    [(path, fields)] = Site.posted
    assert path == "/session"
    assert fields["login"] == [IDENT] and fields["mdp"] == [SECRET] and fields["agence"] == ["a2"]
    assert fields["csrf"] == ["c5rf-t0ken-3361"]


def test_the_page_still_reads_as_before(filled):
    said = filled.tool("execute_script", script="(() => document.title + ' / ' + document.querySelector('#info')"
                                                ".innerText + ' / ' + document.querySelector('#ident').value)()")
    assert f"Espace agences | Essai / Connectez-vous avec l'identifiant de votre agence. / {IDENT}" in said, said
    said = filled.tool("execute_script", script="[...document.querySelectorAll('input')].map((e) => e.name + ':' + "
                                                "e.type).join(',')")
    assert "login:text,mdp:password,mdp_shown:text" in said, said
    said = filled.tool("select_option", selector="#agence", value="a1")
    assert filled.page("document.querySelector('#agence').value") == "a1", said
    said = filled.tool("execute_script", script="document.querySelector('#agence').selectedOptions[0].text")
    assert "Agence Nord" in said, said
    said = filled.tool("get_page_content")
    assert "Espace agences" in said and "Agence Sud" in said, said[:300]


def test_what_the_person_types_while_holding_the_browser(browser):
    browser.open("/pin.html")
    browser.image("take_screenshot")                      # the agent looked at the page, then handed it over
    with Cdp(browser.page_ws) as cdp:                     # the person: the eye, then the code, in clear
        cdp.value("document.querySelector('#voir').click()")
    browser.person_types("#code", PIN)
    assert browser.page("document.querySelector('#code').type") == "text"
    for script in ["document.querySelector('form input')['val' + 'ue']",
                   "[...document.querySelector('form input')['val' + 'ue']].map((c) => c.charCodeAt(0)).join(',')",
                   "document.querySelector('form').innerHTML"]:
        said = browser.tool("execute_script", script=script)
        assert not leaks(said, PIN), (script, said[:200])
    assert browser.page("document.querySelector('#code').type") == "password"
    assert not [v for v in browser.accessibility_values() if leaks(v, PIN)]
    # what the person typed is still there, and the site gets it
    assert browser.page("document.querySelector('#code').value") == PIN
    browser.tool("click_element", selector="#ok")
    deadline = time.time() + 10
    while time.time() < deadline and not Site.posted:
        time.sleep(0.1)
    assert Site.posted == [("/acces", {"code": [PIN]})]


def test_fields_that_only_speak_of_passwords_read_as_typed(browser):
    """A « mot de passe oublié » e-mail, a hint, an address saying autocomplete="new-password" (sites do, to keep the
    browser's suggestions off): typed and read back as before, never masked; the same e-mail on the login page after."""
    browser.open("/forgot.html")
    for selector, text in [("#forgot-password-email", IDENT), ("#password-hint", "le nom du chat"),
                           ("#adresse", "1 rue de l'Essai")]:
        browser.tool("type_text", selector=selector, text=text)
        said = browser.tool("execute_script", script=f"document.querySelector({json.dumps(selector)}).value")
        assert text in said, (selector, said[:200])
    browser.image("take_screenshot")
    assert browser.page("[...document.querySelectorAll('input')].map((e) => e.type).join(',')") == "email,text,text"
    browser.open("/login.html")
    browser.person_types("#ident", IDENT)
    said = browser.tool("execute_script", script="document.querySelector('#ident').value")
    assert IDENT in said, said[:200]
    assert browser.page("document.querySelector('#ident').type") == "text"


def direct(cdp, script):
    """The script as V8 runs it in the page's own world with no side effect allowed (what read_only did before)."""
    got = cdp.call("Runtime.evaluate", {"expression": script, "throwOnSideEffect": True, "returnByValue": True,
                                        "silent": True, "timeout": 5000})
    if "exceptionDetails" in got:
        return ("threw", got["exceptionDetails"].get("exception", {}).get("className", ""))
    return ("value", got["result"].get("value"))


def test_the_bench_of_agents_scripts_reads_as_before(browser):
    """The 145 scripts real agents wrote, on the sign-up bench page with its login password filled by the vault and
    the sign-up's typed: what each gives through the proxy against what V8 gives in the page's own world."""
    browser.open("/signup.html")
    browser.vault_fills(SECRET, "client@example.org")
    with Cdp(browser.page_ws) as cdp:
        cdp.value("[...document.querySelectorAll('button, a')].find((b) => /cr[ée]er un compte/i.test(b.textContent))"
                  ".click()")
    time.sleep(1.0)
    browser.person_types("#register-password", PIN)
    counts = {"same": 0, "refused, as V8 refuses it": 0, "refused by the text rules": 0, "differ": 0}
    differ, refused, times = [], [], []
    with Cdp(browser.page_ws) as cdp:
        for script in BENCH:
            before = direct(cdp, script)
            started = time.time()
            result = browser.call("execute_script", script=script)
            times.append(time.time() - started)
            text = "\n".join(c.get("text", "") for c in result.get("content", []))
            assert not leaks(text) and not leaks(text, PIN), (script[:120], text[:200])
            if text.startswith("Not done"):
                if before[0] == "threw":
                    counts["refused, as V8 refuses it"] += 1
                else:
                    counts["refused by the text rules"] += 1
                    refused.append((script[:90], text[:70]))
                continue
            answer = result.get("structuredContent") or {}
            got = ("value", answer.get("result")) if answer.get("success") else ("threw", "")
            if got == before or got[0] == before[0] == "threw":
                counts["same"] += 1
            elif before[0] == "value" and (leaks(json.dumps(before[1])) or leaks(json.dumps(before[1]), PIN)):
                counts["same"] += 1   # it read a password: now empty, as meant
            else:
                counts["differ"] += 1
                differ.append((script[:100], f"{before} -> {got}"[:200]))
    times.sort()
    print(f"\nbench: {len(BENCH)} scripts: {counts}; per call median {times[len(times) // 2] * 1000:.0f} ms, "
          f"p90 {times[int(len(times) * 0.9)] * 1000:.0f} ms")
    for row in refused:
        print("   refused by the text rules:", row)
    for row in differ:
        print("   differs:", row)
    assert counts["differ"] == 0, differ
