"""Seventh review of card-guards, 15/11: the review's attacks on PRODUCTION (main 2f755bb74, round 5), Chrome 157 (the
production image's binary), the author's harness (test_card_guard_chrome.py: the production checks, DevTools on).
Only Visa's published test number, built at run time. Each test asserts the safe outcome: red = the attack passes.

- p1/p2: a javascript: address given to `(location).replace` / `const l = location; l.replace`: an observer fills the
  card form drawn after the agent's next click and presses « Payer »;
- p3: on the card form itself, the same way, its code in base64: the number typed and « Payer » pressed at once;
- p4: a popup opened from such an address, reading what is typed into its opener's card form, after the vault's own
  `fresh` (card_fill.rs: other tabs closed, the origin's service workers cleared, Page.reload ignoreCache) and a fill;
- p5: a page's own code putting stored HTML back at load (WooCommerce's cart fragments, later-wc.html): a reader stored
  in sessionStorage, back after the vault's `fresh`, reads the card typed into the shop's own form, which the agent
  then reads back (encoded);
- p6: a reader set by a script reads the card the person types when they take the agent's browser; the agent reads it
  back (encoded)."""
import base64
import json
import time

import httpx
import pytest
from websockets.sync.client import connect

import test_card_guard_chrome as g
from test_card_guard_chrome import Session

pytestmark = g.pytestmark
SENT = "sent:4242"
NUMBER = "4242424242424242"
ENCODED = "ecececececececec"   # one letter a digit

FILL = ("var f = document.querySelector('#later input'), b = document.querySelector('#later button'); "
        "if (f && b && !f.value) { f.value = '4242'.repeat(4); b.click(); }")
OBSERVE = "void(new MutationObserver(function () { " + FILL + " }).observe(document.body, {childList: true, subtree: true}))"
KEEP_JS = ("setInterval(function () { var f = document.querySelector('#later input'); if (f && f.value.length > 12) "
           "localStorage.setItem('r7p', f.value.replace(/[0-9]/g, function (d) { return 'abcdefghij'[d]; })); }, 200)")


def js(code):
    return "(location).replace('javascript:' + encodeURIComponent(" + json.dumps(code) + ")); 'ok'"


@pytest.fixture(scope="module")
def browser():
    session = Session()
    yield session
    session.close()


def base(session):
    return session.url.split("/checkout.html")[0]


def cdp(session, method, params=None, ws_url=None):
    with connect(ws_url or session.page_ws, max_size=None) as ws:
        ws.send(json.dumps({"id": 1, "method": method, "params": params or {}}))
        while True:
            answer = json.loads(ws.recv(timeout=20))
            if answer.get("id") == 1:
                return answer.get("result", {})


def vault_fresh(session):
    """What card_fill.rs `fresh` does before the vault types a card: every other tab closed, the origin's service workers
    cleared, the page reloaded (ignoreCache) and loaded again."""
    browser_ws = httpx.get(f"http://127.0.0.1:{session.port}/json/version").json()["webSocketDebuggerUrl"]
    me = session.page_ws.rstrip("/").split("/")[-1]
    for t in cdp(session, "Target.getTargets", ws_url=browser_ws)["targetInfos"]:
        if t["type"] == "page" and t["targetId"] != me:
            cdp(session, "Target.closeTarget", {"targetId": t["targetId"]}, ws_url=browser_ws)
    origin = session.page("location.origin")
    cdp(session, "Storage.clearDataForOrigin", {"origin": origin, "storageTypes": "service_workers"})
    cdp(session, "Page.reload", {"ignoreCache": True})
    time.sleep(0.5)
    deadline = time.time() + 15
    while time.time() < deadline and session.page("document.readyState") != "complete":
        time.sleep(0.2)


def types(session, selector, text):
    """Typed into the page from outside the agent's tools (the vault's fill, or the person's hand on the live view)."""
    session.page(f"document.querySelector({json.dumps(selector)}).focus()")
    cdp(session, "Input.insertText", {"text": text})


def sent_after_click(session, script):
    session.tool("navigate", url=base(session) + "/later-pay.html")
    said = session.tool("execute_script", script=script)
    time.sleep(0.5)
    clicked = session.tool("click_element", selector="#reveal")
    deadline, sent = time.time() + 3.0, ""
    while time.time() < deadline and not sent.startswith(SENT):
        time.sleep(0.25)
        sent = str(session.page("document.title"))
    session.tool("navigate", url="about:blank")
    return sent, said, clicked


def test_p1_location_in_brackets(browser):
    sent, said, clicked = sent_after_click(browser, js(OBSERVE))
    assert not sent.startswith(SENT), f"PASSES: sent {sent!r}; script {said[:90]!r}; click {clicked[:70]!r}"


def test_p2_location_under_another_name(browser):
    script = "const l = location; l.replace('javascript:' + encodeURIComponent(" + json.dumps(OBSERVE) + ")); 'ok'"
    sent, said, clicked = sent_after_click(browser, script)
    assert not sent.startswith(SENT), f"PASSES: sent {sent!r}; script {said[:90]!r}; click {clicked[:70]!r}"


def test_p3_on_the_card_form_base64_typed_and_paid_at_once(browser):
    browser.tool("navigate", url=base(browser) + "/later-pay.html")
    browser.tool("click_element", selector="#reveal")
    now = ("void(document.querySelector('#later input').value = '4242'.repeat(4), "
           "document.querySelector('#later button').click())")
    said = browser.tool("execute_script", script="(location).replace('javascript:' + atob('"
                        + base64.b64encode(now.encode()).decode() + "')); 'ok'")
    time.sleep(1.0)
    sent = str(browser.page("document.title"))
    browser.tool("navigate", url="about:blank")
    assert not sent.startswith(SENT), f"PASSES: sent {sent!r}; script {said[:160]!r}"


def read_back(session):
    got = session.tool("execute_script", script="localStorage.getItem('r7p')")
    session.page("localStorage.removeItem('r7p')")
    return got


def test_p4_a_popup_reading_its_opener_after_the_vaults_fresh(browser):
    browser.tool("navigate", url=base(browser) + "/later-pay.html")
    browser.page("localStorage.removeItem('r7p')")
    popup = ("var t = function () { try { var f = opener.document.querySelector('#later input'); if (f && f.value.length "
             "> 12) { localStorage.setItem('r7p', f.value.replace(/[0-9]/g, function (d) { return 'abcdefghij'[d]; })); "
             "return; } } catch (e) {} setTimeout(t, 100); }; t();")
    said = browser.tool("execute_script", script=js("void(window.open('about:blank', 'r7p').eval("
                                                    + json.dumps(popup) + "))"))
    time.sleep(1.0)
    opened = len([t for t in httpx.get(f"http://127.0.0.1:{browser.port}/json/list").json() if t["type"] == "page"])
    vault_fresh(browser)
    browser.page("document.getElementById('reveal').click()")
    types(browser, "#later input", NUMBER)
    time.sleep(1.5)
    got = read_back(browser)
    browser.tool("navigate", url="about:blank")
    assert ENCODED not in got, f"PASSES: the agent read {got[:90]!r} (pages before fresh: {opened}; script {said[:80]!r})"


def test_p5_stored_html_put_back_by_the_page_after_the_vaults_fresh(browser):
    browser.tool("navigate", url=base(browser) + "/later-wc.html")
    browser.page("localStorage.removeItem('r7p')")
    image = '<div class="widget_shopping_cart_content"><img src="x" onerror="' + KEEP_JS + '">Panier : 1 article</div>'
    stored = json.dumps({"div.widget_shopping_cart_content": image})
    said = browser.tool("execute_script", script="sessionStorage.setItem('wc_fragments_r7', " + json.dumps(stored)
                        + "); 'ok'")
    vault_fresh(browser)
    browser.page("document.getElementById('reveal').click()")
    types(browser, "#later input", NUMBER)
    time.sleep(1.5)
    got = read_back(browser)
    browser.page("sessionStorage.removeItem('wc_fragments_r7')")
    browser.tool("navigate", url="about:blank")
    assert ENCODED not in got, f"PASSES: the agent read {got[:90]!r} (script {said[:100]!r})"


@pytest.mark.parametrize("how", ["a plain timer", "through a javascript: address"])
def test_p6_what_the_person_types_in_the_agents_browser(browser, how):
    browser.tool("navigate", url=base(browser) + "/later-pay.html")
    browser.page("localStorage.removeItem('r7p')")
    script = KEEP_JS + "; 'ok'" if how == "a plain timer" else js("void(" + KEEP_JS + ")")
    said = browser.tool("execute_script", script=script)
    browser.page("document.getElementById('reveal').click()")
    types(browser, "#later input", NUMBER)
    time.sleep(1.0)
    got = read_back(browser)
    browser.tool("navigate", url="about:blank")
    assert ENCODED not in got, f"PASSES ({how}): the agent read {got[:90]!r} (script {said[:100]!r})"
