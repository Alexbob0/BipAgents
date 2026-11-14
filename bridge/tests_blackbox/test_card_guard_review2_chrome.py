"""Second independent review of card-guards (083b7b12f), 14/11, on Chrome 157 (the production image's binary), with the
author's harness (test_card_guard_chrome.py: the stand-in for stealth-browser-mcp behind bip-browser-mcp, the guard's
look in a world of its own through DevTools). Each test states what the branch should do; rw1 and rw2 FAIL on it, rw3 is
the UATP point to judge. Never pushed. Only published test numbers (Visa 4242…, UATP 1354 1001 4004 955)."""
import time
from pathlib import Path

import pytest

import test_card_guard_chrome as g
from test_card_guard_chrome import CARD_REFUSED, NUMBER, Session, value

pytestmark = g.pytestmark
HERE = Path(__file__).parent / "fixtures" / "card_guard"   # rw_fixtures in the review, served by the shop
UATP = "1354 1001 4004 955"   # UATP's published test number


@pytest.fixture(scope="module")
def browser():
    session = Session()
    yield session
    session.close()


def mine(browser, page):
    """One of this review's pages, served beside the author's (the shop's server serves card_guard/)."""
    return browser.url.split("/checkout.html")[0] + "/" + page


def test_rw1_a_card_field_renamed_by_a_script_never_takes_a_card_number(browser):
    # the isolated world keeps the page's scripts from changing the guard's code, not the page itself: a script (no
    # card word, no value written) gives the card field a word of another kind of number; then it takes the number
    browser.fresh()
    said = [browser.tool("execute_script", script="document.getElementById('numero').dataset.kind = 'iban'; 'ok'"),
            browser.tool("type_text", selector="#numero", text=NUMBER)]
    browser.tool("execute_script", script="document.title")   # the next step's look
    assert value(browser, "numero") == "", f"the shop's card field holds {value(browser, 'numero')!r}; {said!r}"


def test_rw2_an_observer_set_before_the_card_form_never_fills_and_sends_it(browser):
    # timers, listeners and observers are refused where the page HAS card fields; set on the step before (none yet),
    # one fills the form the page draws next and presses its « Payer » before any look of the guard
    browser.tool("navigate", url=mine(browser, "later-pay.html"))
    said = browser.tool("execute_script", script=(
        "new MutationObserver(() => { const f = document.querySelector('#later input'); const b = "
        "document.querySelector('#later button'); if (f && b && !f.value) { f.value = ['4242', '4242', '4242', "
        "'4242'].join(''); b.click(); } }).observe(document.body, {childList: true, subtree: true}); 'ok'"))
    clicked = browser.tool("click_element", selector="#reveal")
    time.sleep(1)
    sent = browser.page("document.title")
    assert not sent.startswith("sent:4242"), f"the page sent {sent!r}; the script said {said!r}, the click {clicked!r}"


def test_rw3_a_uatp_number_in_a_field_without_words(browser):
    # the point to judge: UATP left out of the number check (a man's NIR reads as one); a field with no card words
    browser.tool("navigate", url=mine(browser, "uatp.html"))
    typed = {}
    for name, number in [("Visa", NUMBER), ("UATP", UATP)]:
        said = browser.tool("type_text", selector="#f1", text=number)
        typed[name] = (browser.page("document.getElementById('f1').value"), said[:60])
        browser.page("document.getElementById('f1').value = ''")
    print(typed)
    assert typed["Visa"][0] == "" and typed["UATP"][0] == "", typed
