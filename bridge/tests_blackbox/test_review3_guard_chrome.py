"""Third review of fix-signup-hold (765a2f85c): the watch on sending a form during the agent's calls, and the wider
reading of a box's text, in a real Chromium behind the stand-in for stealth-browser-mcp (fixtures/review3/stealth_cdp.py:
signup_guard's, with hold_gate.py's hold). Each test says what should happen; the person's take-over test runs last.

Needs a Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`): skipped without."""
import functools
import http.server
import json
import threading
import time
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

import pytest

import test_signup_guard_chrome as guard_chrome
from test_signup_guard_chrome import Session, pytestmark  # noqa: F401

HERE = Path(__file__).parent
PAGES = HERE / "fixtures" / "review3"
PASSWORD_UNDONE = "Not done: that put text into a password field"
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
    guard_chrome.FIXTURES = PAGES   # this review's stand-in (with the hold) and pages
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


def test_a_form_without_a_form_element_filled_and_sent_by_one_script_never_reaches_the_site(browser):
    # a single-page app's sign-up: no <form>, its button sends the fields (fetch); one script fills it from a table,
    # ticks the terms and clicks the button: no submit event for the watch to stop
    browser.tool("navigate", url=f"{browser.api}/spa_signup.html")
    script = ("const data = {'#email': 'camille@example.org', '#password': 'Provisoire123!'}; "
              "for (const [s, v] of Object.entries(data)) document.querySelector(s).value = v; "
              "document.querySelector('#cgu').checked = true; document.querySelector('#create').click(); 'sent';")
    said = browser.tool("execute_script", script=script)
    time.sleep(1)
    leaked = [s for s in Recorder.sent if s.get("password_length") != "0" or s.get("cgu") == "true"]
    assert not leaked, (said, leaked)


def test_an_older_sites_login_that_hashes_its_password_on_click_still_logs_in(browser):
    # the vault filled the password before (outside the agent's calls); the agent clicks « Se connecter »; the site's
    # button hashes the password in its field, then the form is sent
    browser.tool("navigate", url=f"{browser.base}/login_hashed.html")
    browser.page("(() => { const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; "
                 "set.call(document.getElementById('user'), 'camille'); set.call(document.getElementById('pw'), "
                 "'Secret123!'); return 'filled'; })()")
    said = browser.tool("click_element", selector="#login")
    time.sleep(1)
    where = browser.page("location.pathname")
    assert where.endswith("/welcome.html"), (where, said)


def test_the_newsletter_the_person_asked_for_is_ticked_beside_the_terms(browser):
    # two boxes without <label> in one short paragraph: the newsletter's own text is not the terms
    browser.tool("navigate", url=f"{browser.base}/two_boxes.html")
    said = browser.tool("click_element", selector="#nl")
    ticked = browser.page("document.getElementById('nl').checked")
    assert ticked and not said.startswith(TERMS_UNDONE), (ticked, said)


def test_the_page_never_sees_the_guard_while_the_agent_clicks(browser):
    # a bot detection reading, during the click, whether form.submit is still native and which names the window got
    browser.tool("navigate", url=f"{browser.base}/detect2.html")
    browser.tool("click_element", selector="#go")
    seen = json.loads(browser.page("window.seen"))
    assert seen == {"native": True, "added": []}, seen


def test_control_the_persons_login_after_an_ordinary_call_goes_through(browser):
    # (passes) the same login after an agent's call whose check after it ran: the page and the person's click work
    browser.tool("navigate", url=f"{browser.base}/login.html")
    browser.tool("type_text", selector="#user", text="camille")
    browser.page("(() => { const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; "
                 "const pw = document.getElementById('pw'); set.call(pw, 'MonVraiMotDePasse1'); "
                 "pw.dispatchEvent(new Event('input', {bubbles: true})); document.getElementById('login').click(); "
                 "return 'clicked'; })()")
    time.sleep(1)
    assert browser.page("location.pathname").endswith("/welcome.html")


def test_zz_the_persons_own_login_goes_through_after_they_took_over_during_a_call(browser):
    # the person takes over while the agent's call runs (« TAKEOVER » typed: the stand-in's hold starts then, as
    # hold_gate.py's): the check after that call is refused by the hold; the person then types their password and
    # clicks « Se connecter »
    browser.tool("navigate", url=f"{browser.base}/login.html")
    browser.tool("type_text", selector="#user", text="TAKEOVER")
    browser.page("(() => { const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; "
                 "const pw = document.getElementById('pw'); set.call(pw, 'MonVraiMotDePasse1'); "
                 "pw.dispatchEvent(new Event('input', {bubbles: true})); document.getElementById('login').click(); "
                 "return 'clicked'; })()")
    time.sleep(1)
    where = browser.page("location.pathname")
    assert where.endswith("/welcome.html"), where
