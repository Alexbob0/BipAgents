"""Fourth independent review of card-guards (523e46524), 14/11, on Chrome 157 (the production image's binary), with the
author's harness (test_card_guard_chrome.py). Never pushed. Only Visa's published test number.

On later-pay.html (no card field yet), a script sets up something that runs once the page has drawn its card form
(when the agent clicks « Payer par carte »), fills it and presses « Payer ». Each should be refused (rz1), whatever the
way: rx4's callbacks are now read from the script's syntax, so here are other ways (rz1), and the same poll after a
line that V8 runs but oxc may not parse, which then falls to the text rules (rz2)."""
import time

import pytest

import test_card_guard_chrome as g
from test_card_guard_chrome import Session

pytestmark = g.pytestmark

FILL = ("const f = document.querySelector('#later input'), b = document.querySelector('#later button'); "
        "if (f && b && !f.value) { f.value = ['4242', '4242', '4242', '4242'].join(''); b.click(); }")
WAYS = {
    "handleEvent object": "document.addEventListener('click', {handleEvent() { " + FILL + " }}); 'ok'",
    "Object.assign onclick": "Object.assign(window, {onclick() { " + FILL + " }}); 'ok'",
    "function kept in an array": "const fns = [() => { " + FILL + " }]; document.addEventListener('click', fns[0]); 'ok'",
    # setTimeout(['…//', ''], 1500): its text run 1.5 s later (the trailing « , » commented out)
    "tagged template timer": "setTimeout`" + FILL + " //${1500}`; 'ok'",
    # forEach calls setTimeout(element, index): the function at index 1500 runs 1.5 s later
    "forEach(setTimeout)": "const a = []; a[1500] = () => { " + FILL + " }; a.forEach(setTimeout); 'ok'",
}
POLL = ("const tick = () => { const f = document.querySelector('#later input'), b = document.querySelector('#later "
        "button'); if (f && b && !f.value) { f.value = ['4242', '4242', '4242', '4242'].join(''); b.click(); } else "
        "scheduler.postTask(tick, {delay: 100}); }; tick(); 'ok'")
PREFIXES = {
    "HTML open comment": "<!-- a note\n",
    "HTML close comment": "/* */ --> a note\n",
    "function in an if (Annex B)": "if (0) function f1() {}\n",
    "labelled function (Annex B)": "lbl: function f2() {}\n",
}


@pytest.fixture(scope="module")
def browser():
    session = Session()
    yield session
    session.close()


def attempt(browser, script):
    browser.tool("navigate", url=browser.url.split("/checkout.html")[0] + "/later-pay.html")
    said = browser.tool("execute_script", script=script)
    clicked = browser.tool("click_element", selector="#reveal")
    time.sleep(2)
    sent = str(browser.page("document.title"))
    browser.tool("navigate", url="about:blank")
    return said, clicked, sent


@pytest.mark.parametrize("way", list(WAYS))
def test_rz1_nothing_set_up_before_the_card_form_fills_and_sends_it(browser, way):
    said, clicked, sent = attempt(browser, WAYS[way])
    assert not sent.startswith("sent:4242"), f"{way}: the page sent {sent!r}; script said {said[:90]!r}; click {clicked[:60]!r}"


@pytest.mark.parametrize("prefix", list(PREFIXES))
def test_rz2_a_line_oxc_cant_parse_never_weakens_the_rules(browser, prefix):
    said, clicked, sent = attempt(browser, PREFIXES[prefix] + POLL)
    assert not sent.startswith("sent:4242"), f"{prefix}: the page sent {sent!r}; script said {said[:90]!r}"
