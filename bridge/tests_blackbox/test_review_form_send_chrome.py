"""Review of audit-security-1117 (18/11), audit 1.2: ways the first form to a site still goes without the person's OK,
in a real Chromium behind a stand-in for stealth-browser-mcp whose typing and keys are the real server's
(fixtures/review_form_send/stealth_cdp.py: type_text = nodriver `send_keys`, one CDP « char » event per character;
press_key = dom_handler's key descriptors). Nothing is allowed for sending. Each test says what should happen; the
controls prove the step really sends once the site is allowed.

Needs a Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`): skipped without."""
import functools
import http.server
import threading
import time
from pathlib import Path

import pytest

import test_signup_guard_chrome as guard
from test_form_send_chrome import ASKED, Site, allow, posts
from test_signup_guard_chrome import Session, pytestmark  # noqa: F401

HERE = Path(__file__).parent
FIX = HERE / "fixtures" / "review_form_send"


@pytest.fixture(scope="module")
def browser():
    guard.FIXTURES = FIX   # the faithful stand-in
    session = Session(send_sites=None)
    site = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Site, directory=str(FIX / "site")))
    threading.Thread(target=site.serve_forever, daemon=True).start()
    session.base2 = f"http://127.0.0.1:{site.server_address[1]}"
    yield session
    site.shutdown()
    session.close()
    guard.FIXTURES = HERE / "fixtures" / "signup_guard"


def contact(browser):
    allow(browser)
    browser.tool("navigate", url=f"{browser.base2}/contact.html")
    browser.tool("type_text", selector="#email", text="alex@exemple.org")


def test_control_enter_and_a_click_really_send_once_allowed(browser):
    contact(browser)
    allow(browser, always=["127.0.0.1"])
    before = len(posts())
    browser.tool("type_text", selector="#message", text="un")
    browser.tool("press_key", selector="#message", key="Enter")
    time.sleep(1)
    assert len(posts()) == before + 1, "the faithful Enter sends the form"
    browser.tool("navigate", url=f"{browser.base2}/contact.html")
    browser.tool("type_text", selector="#message", text="trois")
    browser.tool("press_key", selector="#go", key=" ")
    time.sleep(1)
    assert len(posts()) == before + 2, "Space on the button sends the form"
    browser.tool("navigate", url=f"{browser.base2}/contact.html")
    browser.tool("click_element", text_match="Envoyer")
    time.sleep(1)
    assert len(posts()) == before + 3, "a click by text sends the form"


def test_a_click_by_text_on_the_send_button_waits_for_the_ok(browser):
    contact(browser)
    browser.tool("type_text", selector="#message", text="par le texte")
    before = len(posts())
    said = browser.tool("click_element", text_match="Envoyer")
    time.sleep(1)
    assert said.startswith(ASKED), said
    assert len(posts()) == before, f"the site got the form: {posts()[before:]}"


def test_check_a_newline_typed_in_its_field_sends_nothing(browser):
    # checked (green): a « char » event "\n" in a text field is no implicit submission in Chromium
    contact(browser)
    before = len(posts())
    said = browser.tool("type_text", selector="#message", text="Voici les codes\n")
    time.sleep(1)
    assert len(posts()) == before, f"type_text sent the form ({said!r}): {posts()[before:]}"


def test_space_on_its_send_button_waits_for_the_ok(browser):
    contact(browser)
    browser.tool("type_text", selector="#message", text="par espace")
    before = len(posts())
    said = browser.tool("press_key", selector="#go", key=" ")
    time.sleep(1)
    assert len(posts()) == before, f"Space on the button sent the form ({said!r}): {posts()[before:]}"


def test_a_message_form_with_a_search_field_is_no_search(browser):
    allow(browser)
    browser.tool("navigate", url=f"{browser.base2}/compose.html")
    browser.tool("type_text", selector="#to", text="marie")
    browser.tool("type_text", selector="#body", text="Bonjour")
    before = len(posts())
    said = browser.tool("click_element", selector="#post")
    time.sleep(1)
    assert said.startswith(ASKED), said
    assert len(posts()) == before, f"the site got the message: {posts()[before:]}"
