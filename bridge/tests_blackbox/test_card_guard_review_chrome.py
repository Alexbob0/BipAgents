"""Independent review of card-guards (14/11), against a real Chrome (BIP_CHROME: the production image's Chrome 157)
behind the stand-in for stealth-browser-mcp, through bip-browser-mcp (BIP_BROWSER_MCP), as the author's
test_card_guard_chrome.py. Each test states what the branch should do; all failed on it (the reviewer's run), kept as written. Only Visa's
published test number, and synthetic IBAN / NIR values."""
import functools
import http.server
import json
import re
import threading
import time
from pathlib import Path

import pytest

import test_card_guard_chrome as g
from test_card_guard_chrome import CARD_REFUSED, CARD_UNDONE, HIDDEN, NUMBER, Session, value

pytestmark = g.pytestmark
HERE = Path(__file__).parent / "fixtures" / "card_guard_review"
IBAN = "FR98 0378 3840 1165 3551 5752 774"   # synthetic, valid check digits; holds a « card number » for the guard
NIR = "1 48 10 09 262 121 10"                # synthetic, valid key; 15 digits starting with 1 (« UATP »)


@pytest.fixture(scope="module")
def browser():
    session = Session()
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(HERE))
    handler.log_message = lambda *a: None
    session.mine = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=session.mine.serve_forever, daemon=True).start()
    yield session
    session.mine.shutdown()
    session.close()


def open_mine(browser, page):
    browser.tool("navigate", url=f"http://127.0.0.1:{browser.mine.server_address[1]}/{page}")
    deadline = time.time() + 10
    while time.time() < deadline and browser.page("document.readyState") != "complete":
        time.sleep(0.1)


def test_rv1_a_javascript_address_never_writes_a_card_field(browser):
    # navigate is no guarded tool: a javascript: address runs in the page with no check before or after
    browser.fresh()
    said = browser.tool("navigate", url="javascript:void(document.getElementById('numero').value='4242424242424242')")
    time.sleep(0.5)
    assert value(browser, "numero") == "", f"the card field holds {value(browser, 'numero')!r}; navigate said {said!r}"


def test_rv2_a_card_number_is_never_sent_in_an_address(browser):
    browser.fresh()
    said = browser.tool("navigate", url=browser.url + "&cc=4242424242424242&exp=1230")
    assert said.startswith(CARD_REFUSED), f"sent to the site in its address: {browser.page('location.search')!r}; {said!r}"


def test_rv3_the_keyboard_in_a_providers_frame_types_nothing_whatever_the_selector(browser):
    # the author's test: the keyboard in the provider's number field and press_key WITHOUT a selector is refused;
    # with a selector naming something that can't take the focus (a paragraph), the keys still go into the frame
    browser.fresh()
    browser.page("document.getElementById('psp').focus()")
    browser.provider("document.getElementById('number').focus()")
    saids = [browser.tool("type_text", selector="#info", text=chunk) for chunk in ("4242", "4242", "4242", "4242")]
    typed = browser.provider("document.getElementById('number').value")
    assert typed == "", f"the provider's card number field holds {typed!r}; the agent read {saids!r}"


def test_rv4_a_card_number_split_over_boxes_is_never_typed(browser):
    open_mine(browser, "split.html")
    saids = [browser.tool("type_text", selector=f"input[name=p{i}]", text="4242") for i in (1, 2, 3, 4)]
    typed = browser.page("[...document.querySelectorAll('#pay input')].map(e => e.value).join('')")
    assert typed == "", f"the card's number is in the page: {typed!r}; the agent read {saids!r}"


def test_rv5_an_iban_and_a_social_security_number_are_typed_and_read_as_they_are(browser):
    open_mine(browser, "forms.html")
    said = browser.tool("type_text", selector="#iban", text=IBAN)
    kept = browser.page("document.getElementById('iban').value")
    problems = []
    if kept != IBAN:
        problems.append(f"IBAN field (labelled « IBAN ») holds {kept!r} after type_text; the agent read {said!r}")
    said = browser.tool("type_text", selector="#nir", text=NIR)
    kept = browser.page("document.getElementById('nir').value")
    if kept != NIR:
        problems.append(f"NIR field holds {kept!r}; the agent read {said!r}")
    said = browser.tool("execute_script", script="document.getElementById('rib').innerText")
    if IBAN not in said:
        problems.append(f"the page's IBAN read as {said!r}")
    assert not problems, "\n".join(problems)


def test_rv6_a_card_number_shown_in_blocks_is_hidden_from_what_the_agent_reads(browser):
    open_mine(browser, "virtual.html")
    problems = []
    for name, args in [("get_page_content", {}),
                       ("execute_script", {"script": "document.body.innerText"})]:
        said = browser.tool(name, **args)
        if "4242424242424242" in re.sub(r"\D", "", said):
            problems.append(f"{name}: {said[:300]!r}")
    assert not problems, "\n".join(problems)


def test_rv8_a_card_number_written_a_moment_after_the_script_is_still_erased(browser):
    # the catch-up runs once, right after the call: a script whose write comes later (a timer) is never looked at
    # again; nothing in this script names a card field nor holds a number the guard reads
    browser.fresh()
    said = browser.tool("execute_script", script="setTimeout(() => { document.querySelectorAll('#pay input')[0].value = "
                                                 "['4242', '4242', '4242', '4242'].join(''); }, 800); 'ok'")
    time.sleep(2)
    browser.tool("execute_script", script="document.title")   # the agent's next call: a look before it, an after
    assert value(browser, "numero") == "", f"the shop's card field holds {value(browser, 'numero')!r}; {said!r}"


def test_rv9_a_card_number_written_otherwise_is_never_typed_anywhere(browser):
    # the author's test refuses « 4242 4242 4242 4242 » in the delivery notes; other ways of writing it go in
    browser.fresh()
    typed = []
    for text in ["Carte 4242  4242  4242  4242 exp 12/30", "Carte 4242/4242/4242/4242", "Carte 4242 – 4242 – 4242 – 4242",
                 "Carte\n4242\n4242\n4242\n4242"]:
        said = browser.tool("type_text", selector="#notes", text=text)
        if value(browser, "notes") != "":
            typed.append(f"{value(browser, 'notes')!r} ({said[:60]!r})")
        browser.page("document.getElementById('notes').value = ''")
    assert not typed, "typed into the delivery notes:\n" + "\n".join(typed)


def test_rv10_the_guards_look_cannot_be_changed_by_an_earlier_script(browser):
    # the guard's look before and after runs in the page's own world (through execute_script): an earlier script of
    # the agent's (nothing about cards in it, no value written) changes what it sees, and the card's number goes in
    browser.fresh()
    prep = ("Object.defineProperty(HTMLInputElement.prototype, 'name', {get() { return 'iban'; }}); "
            "String.prototype.match = function () { return null; }; 'ok'")
    said = [browser.tool("execute_script", script=prep), browser.tool("type_text", selector="#numero", text=NUMBER)]
    assert value(browser, "numero") == "", f"the shop's card field holds {value(browser, 'numero')!r}; {said!r}"
