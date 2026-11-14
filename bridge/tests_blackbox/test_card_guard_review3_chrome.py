"""Third independent review of card-guards (68729945f), 14/11, on Chrome 157 (the production image's binary), with the
author's harness (test_card_guard_chrome.py). Never pushed. Only Visa's published test number.

rx4: callbacks run later that DEFERS doesn't name (scheduler.postTask, navigator.locks) poll until the card form the
page draws next is there, fill it and press « Payer »: the observer of rw2 by other means."""
import time

import pytest

import test_card_guard_chrome as g
from test_card_guard_chrome import Session

pytestmark = g.pytestmark

POLL = {
    "scheduler.postTask": "const tick = () => { const f = document.querySelector('#later input'), b = "
                          "document.querySelector('#later button'); if (f && b && !f.value) { f.value = ['4242', '4242', "
                          "'4242', '4242'].join(''); b.click(); } else scheduler.postTask(tick, {delay: 100}); }; tick(); 'ok'",
    "navigator.locks": "const tick = () => navigator.locks.request('x', () => { const f = document.querySelector('#later "
                       "input'), b = document.querySelector('#later button'); if (f && b && !f.value) { f.value = "
                       "['4242', '4242', '4242', '4242'].join(''); b.click(); } else tick(); }); tick(); 'ok'",
}


@pytest.fixture(scope="module")
def browser():
    session = Session()
    yield session
    session.close()


@pytest.mark.parametrize("how", list(POLL))
def test_rx4_a_callback_run_later_never_fills_and_sends_the_card_form(browser, how):
    browser.tool("navigate", url=browser.url.split("/checkout.html")[0] + "/later-pay.html")
    said = browser.tool("execute_script", script=POLL[how])
    clicked = browser.tool("click_element", selector="#reveal")
    time.sleep(1.5)
    sent = browser.page("document.title")
    browser.tool("navigate", url="about:blank")   # the poll stops with the page
    assert not str(sent).startswith("sent:4242"), f"{how}: the page sent {sent!r}; script said {said[:80]!r}; click {clicked[:80]!r}"
