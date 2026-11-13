"""Fifth review of fix-signup-hold (80be896d5): the rule decided from the page, and a password never read back, in a
real Chromium behind the stand-in for stealth-browser-mcp (fixtures/review5/stealth_cdp.py, review3's). Each test says
what should happen.

Needs a Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`): skipped without."""
import base64
import time
from pathlib import Path

import pytest

import test_signup_guard_chrome as guard_chrome
from test_signup_guard_chrome import TERMS_REFUSED, Session, pytestmark  # noqa: F401

PAGES = Path(__file__).parent / "fixtures" / "review5"
TERMS_UNDONE = "Not done: that ticked the site's terms"
VAULT_PASSWORD = "Xq7!vR2m#Lp9@wT4zK8e"


@pytest.fixture(scope="module")
def browser():
    original = guard_chrome.FIXTURES
    guard_chrome.FIXTURES = PAGES
    try:
        session = Session()
    finally:
        guard_chrome.FIXTURES = original
    yield session
    session.close()


def test_a_terms_box_drawn_by_a_web_component_is_never_ticked_by_the_agent(browser):
    # its input in a shadow root, its text slotted in the page: a click on the component ticks it
    browser.tool("navigate", url=f"{browser.base}/shadow.html")
    said = browser.tool("click_element", selector="terms-box")
    ticked = browser.page("document.getElementById('cgu-box').checked")
    assert not ticked and (said.startswith(TERMS_REFUSED) or said.startswith(TERMS_UNDONE)), (ticked, said)


def test_a_terms_box_in_the_sites_own_frame_is_never_ticked_by_a_script(browser):
    # the shop's sign-up form in a frame of the shop (accept_terms looks into such frames; the browser's checks don't)
    browser.tool("navigate", url=f"{browser.base}/frame_host.html")
    time.sleep(1)
    said = browser.tool("execute_script", script="document.querySelector('#signup-frame').contentDocument"
                                                 ".querySelector('#cgu').click(); 'ok';")
    ticked = browser.page("document.querySelector('#signup-frame').contentDocument.getElementById('cgu').checked")
    assert not ticked, (ticked, said)


def test_a_custom_dropdown_opened_and_chosen_by_one_script_goes_on(browser):
    # signup.html's « Civilité » is a button and a list, not a <select> (select_option can't): open it, pick « M. »
    browser.tool("navigate", url=f"{browser.base}/signup.html")
    browser.tool("click_element", selector="#open-register")
    time.sleep(1)
    said = browser.tool("execute_script", script="document.querySelector('#civility').click(); "
                                                 "document.querySelector('#civility-options li[data-value=\"M\"]').click(); 'ok';")
    chosen = browser.page("document.getElementById('civility-value').value")
    assert chosen == "M", (chosen, said)


def test_a_password_the_vault_typed_is_never_read_back(browser):
    # the vault filled the form; a script hands its password over encoded, or through the page's title
    browser.tool("navigate", url=f"{browser.base}/pw_read.html")
    browser.page("(() => { const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; "
                 f"set.call(document.getElementById('pass1'), '{VAULT_PASSWORD}'); "
                 f"set.call(document.getElementById('pass2'), '{VAULT_PASSWORD}'); return 'filled'; }})()")
    leaks = {}
    encoded = browser.tool("execute_script", script="btoa(document.querySelector('#pass1').value)")
    if base64.b64encode(VAULT_PASSWORD.encode()).decode() in encoded:
        leaks["base64"] = encoded
    browser.tool("execute_script", script="document.title = document.querySelector('#pass1').value; 'ok'")
    read = browser.tool("get_page_content")
    if VAULT_PASSWORD in read:
        leaks["title"] = read[:200]
    assert not leaks, leaks
