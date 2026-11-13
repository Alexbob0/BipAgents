"""Second review of fix-signup-hold (9415fdb13): bip-browser-mcp's checks before and after a call, in a real Chromium
behind the stand-in for stealth-browser-mcp (test_signup_guard_chrome.Session). Each test says what should happen.

Needs a Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`): skipped without."""
import functools
import http.server
import json
import subprocess
import sys
import threading
import time
from pathlib import Path

import pytest

from bbkit.servers import free_port
from test_signup_guard_chrome import PASSWORD_REFUSED, Session, TERMS_REFUSED, pytestmark  # noqa: F401

HERE = Path(__file__).parent
PAGES = HERE / "fixtures" / "review2"
SHOP = HERE / "fixtures" / "signup" / "site.py"
TERMS_UNDONE = "Not done: that ticked the site's terms"


@pytest.fixture(scope="module")
def browser(tmp_path_factory):
    session = Session()
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(PAGES))
    handler.log_message = lambda *a: None
    pages = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=pages.serve_forever, daemon=True).start()
    session.pages = f"http://127.0.0.1:{pages.server_address[1]}"
    shop_port = free_port()
    session.submissions = tmp_path_factory.mktemp("shop") / "submissions.jsonl"
    shop = subprocess.Popen([sys.executable, str(SHOP), str(shop_port), str(session.submissions)])
    session.shop = f"http://127.0.0.1:{shop_port}"
    time.sleep(1)
    yield session
    shop.terminate()
    pages.shutdown()
    session.close()


def test_a_terms_box_that_also_names_cookies_is_never_ticked_by_the_agent(browser):
    # « J'accepte les conditions générales d'utilisation et la politique de confidentialité et de cookies »: the terms
    browser.tool("navigate", url=f"{browser.pages}/terms_cookies.html")
    said = browser.tool("click_element", selector="#cgu")
    ticked = browser.page("document.getElementById('cgu').checked")
    assert not ticked and (said.startswith(TERMS_REFUSED) or said.startswith(TERMS_UNDONE)), (ticked, said)


def test_a_form_filled_ticked_and_sent_in_one_script_never_reaches_the_site(browser):
    # the agent fills the whole sign-up form from a map, ticks the terms and sends it in one execute_script: what the
    # check after the call would undo has already left with the form
    browser.tool("navigate", url=f"{browser.shop}/connexion")
    browser.tool("click_element", selector="#open-register")
    time.sleep(1)
    script = (
        "const data = {'#civility-value': 'M', '#lastname': 'Testeur', '#firstname': 'Camille', '#phone': '0600000001', "
        "'#register-email': 'camille@example.org', '#birthday-day': '14', '#birthday-month': '03', "
        "'#birthday-year': '1991', '#register-password': 'Provisoire123!', "
        "'#register-password-confirmation': 'Provisoire123!'}; "
        "for (const [s, v] of Object.entries(data)) document.querySelector(s).value = v; "
        "document.querySelector('#cgu').checked = true; "
        "document.getElementById('register-form').requestSubmit(); 'sent';")
    said = browser.tool("execute_script", script=script)
    time.sleep(2)
    sent = [json.loads(line) for line in browser.submissions.read_text().splitlines()] \
        if browser.submissions.exists() else []
    leaked = [{"password_length": s["password_length"], "cgu": s["fields"].get("cgu")} for s in sent
              if s["password_length"] > 0 or s["fields"].get("cgu") == "1"]
    assert not leaked, (said, leaked)


def test_the_terms_link_in_the_boxs_label_opens_the_terms(browser):
    # reading the terms (the link inside the box's label) ticks nothing: the click is the agent's to make
    browser.tool("navigate", url=f"{browser.pages}/terms_link.html")
    said = browser.tool("click_element", selector="#cgv-link")
    time.sleep(1)
    where = browser.page("location.pathname")
    assert where.endswith("/cgv.html"), (where, said)


def test_the_page_never_sees_the_guard_while_the_agent_clicks(browser):
    # the check before a call keeps its state on the page's window (Symbol.for('bipagents.guard')): a page's own script
    # running during the click (a bot detection) reads it there
    browser.tool("navigate", url=f"{browser.pages}/detect.html")
    browser.tool("click_element", selector="#go")
    seen = browser.page("window.seen")
    assert "bipagents" not in (seen or ""), seen


def test_control_a_box_without_a_label_is_refused_to_the_agent(browser):
    # (passes) the browser's check refuses the unlabelled terms box by its name: only accept_terms is left
    browser.tool("navigate", url=f"{browser.pages}/terms_unlabelled.html")
    said = browser.tool("click_element", selector="#accept-terms")
    assert said.startswith(TERMS_REFUSED), said
    assert browser.page("document.getElementById('accept-terms').checked") is False
