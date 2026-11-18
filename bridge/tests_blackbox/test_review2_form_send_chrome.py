"""Re-check of audit-security-1117 R1 (18/11, head ff31e8402): a click by text (`text_match`) on a form's send button
asks first. The stand-in clicks by text as nodriver's `tab.find(best_match=True)` does (fixtures/review2_form_send):
own text or ANY attribute value, case ignored, the top document and same-origin iframes, the closest length wins.
Each test says what should happen; the control proves the click really sends once the site is allowed.

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
FIX = HERE / "fixtures" / "review2_form_send"


@pytest.fixture(scope="module")
def browser():
    guard.FIXTURES = FIX
    session = Session(send_sites=None)
    site = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Site, directory=str(FIX / "site")))
    threading.Thread(target=site.serve_forever, daemon=True).start()
    session.base2 = f"http://127.0.0.1:{site.server_address[1]}"
    yield session
    site.shutdown()
    session.close()
    guard.FIXTURES = HERE / "fixtures" / "signup_guard"


def clicked(browser, page, text, allowed=False):
    """The posts the site got from a click by `text` on `page`, and what the agent read."""
    allow(browser, always=["127.0.0.1"] if allowed else [])
    browser.tool("navigate", url=f"{browser.base2}/{page}")
    time.sleep(0.5)
    before = len(posts())
    said = browser.tool("click_element", text_match=text)
    time.sleep(1)
    return posts()[before:], said


def test_control_each_click_really_sends_once_allowed(browser):
    for page, text in [("contact.html", "Envoyer"), ("icon.html", "Envoyer le message"), ("framed.html", "Envoyer"),
                       ("js.html", "Envoyer")]:
        sent, said = clicked(browser, page, text, allowed=True)
        assert len(sent) == 1, (page, said, sent)


@pytest.mark.parametrize("text", ["envoy", "ENVOYER", " Envoyer "])
def test_partial_or_other_case_text_asks(browser, text):
    sent, said = clicked(browser, "contact.html", text)
    assert said.startswith(ASKED) and sent == [], (said, sent)


def test_an_icon_button_named_by_aria_label_asks(browser):
    sent, said = clicked(browser, "icon.html", "Envoyer le message")
    assert said.startswith(ASKED) and sent == [], (said, sent)


def test_a_send_button_in_a_same_origin_frame_asks(browser):
    sent, said = clicked(browser, "framed.html", "Envoyer")
    assert said.startswith(ASKED) and sent == [], (said, sent)


@pytest.mark.xfail(strict=True, reason="known open: a form sent by the page's own script from a non-submit element "
                   "(div onclick / JS without a send button) is not seen by the guard; the egress proxy's first non-GET "
                   "request per host would cover it")
def test_known_open_a_script_sends_from_a_non_submit_element(browser):
    # known residual (JS without a send button): recorded, not a finding of this re-check
    sent, said = clicked(browser, "js.html", "Envoyer")
    assert said.startswith(ASKED) and sent == [], (said, sent)
