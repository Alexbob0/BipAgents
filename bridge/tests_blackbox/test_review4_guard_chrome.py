"""Fourth review of fix-signup-hold (81c8cbdbf): the script rule (« sets a password or ticks a box, and clicks or
sends ») and the text read after a box without a label, in a real Chromium behind the stand-in for stealth-browser-mcp
(fixtures/review4/stealth_cdp.py, review3's). Each test says what should happen.

Needs a Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`): skipped without."""
import functools
import http.server
import threading
import time
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

import pytest

import test_signup_guard_chrome as guard_chrome
from test_signup_guard_chrome import TERMS_REFUSED, Session, pytestmark  # noqa: F401

HERE = Path(__file__).parent
PAGES = HERE / "fixtures" / "review4"
TERMS_UNDONE = "Not done: that ticked the site's terms"


class Recorder(http.server.SimpleHTTPRequestHandler):
    """The pages, and what a page's own script sent to `collect` (the site's API)."""
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
    guard_chrome.FIXTURES = PAGES
    try:
        session = Session()
    finally:
        guard_chrome.FIXTURES = original
    recorder = http.server.ThreadingHTTPServer(("127.0.0.1", 0),
                                               functools.partial(Recorder, directory=str(PAGES)))
    threading.Thread(target=recorder.serve_forever, daemon=True).start()
    session.api = f"http://127.0.0.1:{recorder.server_address[1]}"
    yield session
    recorder.shutdown()
    session.close()


def sent_since(start):
    return [s for s in Recorder.sent[start:] if s.get("password_length") != "0" or s.get("cgu") == "true"]


def test_the_terms_box_clicked_then_the_apps_button_clicked_in_one_script_never_reaches_the_site(browser):
    # ticking a box is also a click on it: « box.click(); button.click() »
    browser.tool("navigate", url=f"{browser.api}/spa_signup.html")
    start = len(Recorder.sent)
    said = browser.tool("execute_script", script="document.querySelector('#cgu').click(); "
                                                 "document.querySelector('#create').click(); 'sent';")
    time.sleep(1)
    assert not sent_since(start), (said, sent_since(start))


def test_a_password_field_named_pass1_filled_and_sent_by_one_script_never_reaches_the_site(browser):
    # WordPress names its password fields pass1 and pass2: no « password » word in the script
    browser.tool("navigate", url=f"{browser.api}/spa_signup_pass1.html")
    start = len(Recorder.sent)
    script = ("const data = {'#email': 'camille@example.org', '#pass1': 'Provisoire123!', '#pass2': 'Provisoire123!'}; "
              "for (const [s, v] of Object.entries(data)) document.querySelector(s).value = v; "
              "document.querySelector('#create').click(); 'sent';")
    said = browser.tool("execute_script", script=script)
    time.sleep(1)
    assert not sent_since(start), (said, sent_since(start))


def test_a_terms_box_followed_by_its_untied_label_is_never_ticked_by_the_agent(browser):
    # the box, then its text in a <label> tied to nothing, two such boxes in one block
    browser.tool("navigate", url=f"{browser.base}/unassociated_labels.html")
    said = browser.tool("click_element", selector="#opt2")
    ticked = browser.page("document.getElementById('opt2').checked")
    assert not ticked and (said.startswith(TERMS_REFUSED) or said.startswith(TERMS_UNDONE)), (ticked, said)


# --- what the fourth review's fix keeps going, and a password read back -----------------------------------------------

def test_a_shop_filter_and_a_reset_e_mail_sent_by_a_script_go_on(browser):
    # no password field, no terms box: the script runs and the site gets what it asked for
    browser.tool("navigate", url=f"{browser.api}/shop_filters.html")
    start = len(Recorder.sent)
    said = browser.tool("execute_script", script="document.querySelector('#filter-bio').checked = true; "
                                                 "document.querySelector('#apply-filters').click(); 'ok';")
    said_reset = browser.tool("execute_script", script="document.querySelector('#forgot-password-email').value = "
                                                       "'camille@example.org'; "
                                                       "document.querySelector('#send-reset-link').click(); 'ok';")
    time.sleep(1)
    got = Recorder.sent[start:]
    assert {"filter_bio": "true"} in got and {"reset_for": "camille@example.org"} in got, (said, said_reset, got)


def test_a_script_reading_the_forms_values_never_hands_back_a_password(browser):
    # the vault typed it; a script listing every field's value would give it to the model
    browser.tool("navigate", url=f"{browser.api}/login_filled.html")
    password = browser.page("document.getElementById('pass1').value")
    listed = browser.tool("execute_script",
                          script="JSON.stringify([...document.querySelectorAll('input')].map((e) => e.value))")
    assert password and password not in listed and "Vx9" not in listed, listed
    assert listed.startswith(guard_chrome.SCRIPT_PASSWORD_REFUSED), listed
    # read another way the script's words don't show: its answer is looked at in the page
    said = browser.tool("execute_script",
                        script="JSON.stringify([...document.getElementById('login').children].map((e) => e.value || ''))")
    assert password not in said and "Vx9" not in said, said
    assert said.startswith("Not shown: that answer held what a password field holds"), said
    # the other fields can still be read
    email = browser.tool("execute_script", script="document.getElementById('email').value")
    assert "camille@example.org" in email, email
