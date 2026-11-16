"""Seventh review of card-guards, 15/11: new ways against the production hotfix (hotfix-card-guards-main 23785ed96), on
Chrome 157 (the production image's binary), the author's harness (production checks, DevTools on). Only Visa's
published test number, built at run time. Red = the attack passes.

- h1: the Location taken from the document's only own property (its descriptor, the document reached as
  `body.getRootNode()`), its `replace` called on what the script calls a string (an own object's `toString`), a
  javascript: address in pieces the guard can't put together: an observer fills the card form drawn after the agent's
  next click and presses « Payer »;
- h2: the same Location's own `href` setter called with `.call`;
- h3: a cookie written through Document.prototype's setter (outlives every reload);
- n1/n2: what must still work: a purchase step with type_text and click_element only; a script that only reads, on a
  page without card fields."""
import json
import time

import pytest

import test_card_guard_chrome as g
from test_card_guard_chrome import Session

pytestmark = g.pytestmark
SENT = "sent:4242"
FILL = ("var f = document.querySelector('#later input'), b = document.querySelector('#later button'); "
        "if (f && b && !f.value) { f.value = '4242'.repeat(4); b.click(); }")
OBSERVE = "void(new MutationObserver(function () { " + FILL + " }).observe(document.body, {childList: true, subtree: true}))"
DOC = ("const d = document.body.getRootNode(); "
       "const L = Object.values(Object.getOwnPropertyDescriptors(d)).find((x) => x.get).get.call(d); "
       "const c = " + json.dumps(OBSERVE) + "; ")
ADDRESS = "'jav' + ['a'].pop() + 'script:' + encodeURIComponent(c)"
WAYS = {
    "h1 toString's replace": DOC + "const o = {toString() { return L; }}; o.toString().replace(" + ADDRESS + "); 'ok'",
    "h2 href's setter": DOC + "Object.values(Object.getOwnPropertyDescriptors(L)).find((x) => x.set).set.call(L, "
                        + ADDRESS + "); 'ok'",
}


@pytest.fixture(scope="module")
def browser():
    session = Session()
    yield session
    session.close()


def base(session):
    return session.url.split("/checkout.html")[0]


@pytest.mark.parametrize("way", list(WAYS))
def test_h_location_without_its_name_never_runs_code_later(browser, way):
    browser.tool("navigate", url=base(browser) + "/later-pay.html")
    said = browser.tool("execute_script", script=WAYS[way])
    time.sleep(0.5)
    clicked = browser.tool("click_element", selector="#reveal")
    deadline, sent = time.time() + 3.0, ""
    while time.time() < deadline and not sent.startswith(SENT):
        time.sleep(0.25)
        sent = str(browser.page("document.title"))
    browser.tool("navigate", url="about:blank")
    assert not sent.startswith(SENT), f"PASSES ({way}): sent {sent!r}; script {said[:120]!r}; click {clicked[:70]!r}"


def test_h3_a_cookie_written_through_the_prototypes_setter(browser):
    browser.tool("navigate", url=base(browser) + "/later-pay.html")
    said = browser.tool("execute_script", script="Object.getOwnPropertyDescriptor(Document.prototype, 'cookie').set"
                                                 ".call(document, 'r7h3=1; path=/'); 'ok'")
    kept = str(browser.page("document.cookie"))
    browser.page("document.cookie = 'r7h3=; max-age=0; path=/'")
    browser.tool("navigate", url="about:blank")
    assert "r7h3=1" not in kept, f"PASSES: the cookie was written ({kept!r}); script {said[:100]!r}"


def test_n1_a_purchase_step_with_type_text_and_click_element_still_works(browser):
    browser.tool("navigate", url=browser.url)
    said = browser.tool("type_text", selector="#address", text="2 rue du Test")
    assert "Not done" not in said and browser.page("document.getElementById('address').value") == "2 rue du Test", said
    said = browser.tool("type_text", selector="#notes", text="Sonner deux fois")
    assert "Not done" not in said, said
    said = browser.tool("select_option", selector="#mois", value="03")
    assert said.startswith("Not done"), f"a card's expiry chosen: {said[:100]!r}"
    browser.tool("navigate", url=base(browser) + "/later-pay.html")
    said = browser.tool("click_element", selector="#reveal")
    assert "Not done" not in said and browser.page("!!document.querySelector('#later input')") is True, said
    browser.tool("navigate", url="about:blank")


def test_n2_a_script_that_only_reads_still_runs_where_no_card_field_is(browser):
    browser.tool("navigate", url=base(browser) + "/later-pay.html")
    said = browser.tool("execute_script", script="[...document.querySelectorAll('button')].map((b) => b.innerText)"
                                                 ".concat([location.href.length > 0, document.title])")
    assert "Payer par carte" in said and "Commande" in said, said
    said = browser.tool("execute_script", script="(() => { const t = document.body.innerText; const i = t.search(/total/i);"
                                                 " return i < 0 ? 'not found' : t.slice(Math.max(0, i - 50), i + 80); })()")
    assert "12,50" in said, said
    browser.tool("navigate", url="about:blank")
