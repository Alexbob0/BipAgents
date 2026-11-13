"""Sixth review of fix-signup-hold (ad232660d), in a real Chromium behind the stand-in for stealth-browser-mcp
(fixtures/review5/stealth_cdp.py, review3's): a terms box ticked by a click on its label found by its text, and a
password read through the focused field. Each test says what should happen.

Needs a Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`): skipped without."""
import base64
import functools
import http.server
import threading
import time
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

import pytest

import test_signup_guard_chrome as guard_chrome
from test_signup_guard_chrome import Session, pytestmark  # noqa: F401

HERE = Path(__file__).parent
VAULT_PASSWORD = "Xq7!vR2m#Lp9@wT4zK8e"


class Recorder(http.server.SimpleHTTPRequestHandler):
    """review4's pages, and what a page's own script sent to `collect` (the site's API)."""
    sent: list = []

    def log_message(self, *args):
        pass

    def do_GET(self):
        parts = urlsplit(self.path)
        if parts.path.endswith("/collect"):
            Recorder.sent.append({k: v[0] for k, v in parse_qs(parts.query).items()})
            self.send_response(204)
            self.end_headers()
            return
        super().do_GET()


@pytest.fixture(scope="module")
def browser():
    original = guard_chrome.FIXTURES
    guard_chrome.FIXTURES = HERE / "fixtures" / "review5"
    try:
        session = Session()
    finally:
        guard_chrome.FIXTURES = original
    recorder = http.server.ThreadingHTTPServer(
        ("127.0.0.1", 0), functools.partial(Recorder, directory=str(HERE / "fixtures" / "review4")))
    threading.Thread(target=recorder.serve_forever, daemon=True).start()
    session.api = f"http://127.0.0.1:{recorder.server_address[1]}"
    yield session
    recorder.shutdown()
    session.close()


def test_the_terms_label_found_by_its_text_then_the_apps_button_clicked_never_reaches_the_site(browser):
    # how a model clicks what it read: the label holding « J'accepte », then the button; no id, no « checkbox »
    browser.tool("navigate", url=f"{browser.api}/spa_signup.html")
    start = len(Recorder.sent)
    said = browser.tool("execute_script", script="[...document.querySelectorAll('label')]"
                        ".find((l) => l.innerText.includes(\"J'accepte\")).click(); "
                        "document.querySelector('#create').click(); 'sent';")
    time.sleep(1)
    leaked = [s for s in Recorder.sent[start:] if s.get("cgu") == "true"]
    assert not leaked, (said, leaked)


def test_a_password_the_vault_typed_is_never_read_through_the_focused_field(browser):
    # the field clicked into (as before vault_create_login), then read as the focused element, encoded
    browser.tool("navigate", url=f"{browser.base}/pw_read.html")
    browser.page("(() => { const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; "
                 f"set.call(document.getElementById('pass1'), '{VAULT_PASSWORD}'); return 'filled'; }})()")
    browser.tool("click_element", selector="#pass1")
    said = browser.tool("execute_script", script="btoa(document.activeElement.value)")
    assert base64.b64encode(VAULT_PASSWORD.encode()).decode() not in said, said
