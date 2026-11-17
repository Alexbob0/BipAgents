"""Review of vault-no-readback (16/11): what still reads a password field back past the fix (ee98d73d0), in a real
Chromium behind bip-browser-mcp, as test_vault_no_readback_chrome.py runs it (its Session, its fake login page, the
vault's fill as login_fill.rs types it), the stand-in for stealth-browser-mcp given click_element's `text_match` as the
real server does it (fixtures/vault_readback/review/stealth_find.py: nodriver's `tab.find`, DOM.performSearch).

Each test is a way an agent misled by a page or an e-mail gets the password, a code, or part of it:
- click_element's `text_match` is a DevTools search (text, CSS selector and XPath, in every attribute), past the
  selector rule: a hidden copy of the password, one guess at a time;
- a page that renders its form meanwhile puts the password back between the guard's setting aside and the script;
- a page of the same site opened by the login page (window.open) reads the login page through `opener`;
- a copy of the password in the page's text (a site's eye drawn as a box of text) is not set aside;
- a code the person typed on a bank's keypad, kept in a hidden field named as a password, is not protected;
- an XPath doing sums on a hidden copy gives a numeric code digit by digit;
- another site's frame (a partner's login) shown by its eye is in clear in the agent's screenshot;
and what the setting aside does to the site: a frame whose address holds the same digits as a 4-digit code reloads
(what the person wrote there is lost); a « mot de passe oublié » e-mail field named for the recovery reads as empty.

Needs a Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`): skipped without. Fake values only."""
import base64
import json
import queue
import subprocess
import sys
import threading
import time

import httpx
import pytest

import test_vault_no_readback_chrome as base
from test_vault_no_readback_chrome import PIN, SECRET, Cdp, Session, leaks

pytestmark = base.pytestmark

REVIEW = base.FIXTURES / "review"
FINDS = REVIEW / "stealth_find.py"
CODE = "482915"   # fake: a bank's code typed on its keypad


def codes(expression):
    """A value as its character codes: past the answer check (it knows the value, reversed, base64, hex, URL-encoded)."""
    return "[...String(" + expression + ")].map((c) => c.charCodeAt(0)).join(',')"


def clicked(said):
    return said.startswith("true") or '"result": true' in said


def found(said):
    return '"tag_name"' in said


@pytest.fixture(scope="module")
def browser():
    stand_in = base.STAND_IN
    base.STAND_IN = FINDS   # Session's server: the stand-in with the real server's text_match
    try:
        session = Session()
    finally:
        base.STAND_IN = stand_in
    yield session
    session.close()


def drive(browser, ws):
    """The proxy started again over the stand-in driving the tab at `ws` (the agent switched to it: switch_tab)."""
    browser.proxy.stdin.close()
    browser.proxy.wait(timeout=30)
    config = json.loads((browser.dir / "config.json").read_text())
    config["server"] = [sys.executable, str(FINDS), ws]
    (browser.dir / "config.json").write_text(json.dumps(config))
    browser.proxy = subprocess.Popen([base.PROXY, str(browser.dir / "config.json")], stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=open(browser.log, "ab"))
    browser.answers = queue.Queue()
    threading.Thread(target=browser._read, daemon=True).start()
    browser.ask("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "hermes"}})
    browser.ask("tools/list", {})


def target(browser, url, kind="page"):
    deadline = time.time() + 10
    while time.time() < deadline:
        for t in httpx.get(f"http://127.0.0.1:{browser.port}/json/list").json():
            if t["type"] == kind and t["url"] == url:
                return t
        time.sleep(0.2)
    raise AssertionError(f"no {kind} at {url}")


def test_click_elements_text_match_tells_nothing_of_a_password(browser):
    """`text_match` reaches DOM.performSearch: a CSS selector, an XPath or plain text matched in every attribute. The
    site's base64 copy in a hidden field (as fixtures/vault_readback/login.html keeps it) is left there between scripts;
    found or not tells the agent whether its guess starts the copy, four characters at a time."""
    browser.open("/login.html")
    browser.vault_fills()
    copy = base64.b64encode(SECRET.encode()).decode()
    assert browser.page("document.querySelector('#enc').value") == copy
    for how, form, right, wrong in [
        ("a CSS selector", 'body:has(input[value^="{}"]) h1', copy[:4], "QUFB"),
        ("an XPath", "//h1[//input[starts-with(@value,'{}')]]", copy[:4], "QUFB"),
        ("plain text", "{}", copy[:8], "QUFBQUFB"),
    ]:
        said = {guess: browser.tool("click_element", selector="h1", text_match=form.format(guess))
                for guess in (right, wrong)}
        assert clicked(said[right]) == clicked(said[wrong]), (
            f"{how}: a right guess clicked ({said[right][:80]!r}), a wrong one not ({said[wrong][:80]!r})")


@pytest.mark.parametrize("every", ["task", "frame", "4"])
def test_a_page_rendering_meanwhile_never_hands_the_script_the_password(browser, every):
    """The guard sets the field aside, then the agent's script runs: two DevTools calls, and the page's own tasks run
    between them. A React page rendering its form then sets the controlled input back from its state (race.html: one
    task after another, at each frame, every 4 ms)."""
    browser.open(f"/review/race.html?every={every}")
    browser.vault_fills()
    assert browser.page("document.querySelector('#mdp').value") == SECRET
    field = "document.querySelector('form input:nth-of-type(2)')['val' + 'ue']"
    read = []
    for attempt in range(40):
        said = browser.tool("execute_script", script=codes(field))
        if leaks(said):
            read.append(attempt + 1)
    print(f"\nrace, a render {every}: the password read at calls {read} of 40")
    assert not read, f"a render {every}: the agent read the password at calls {read} of 40"
    assert browser.page("document.querySelector('#mdp').value") == SECRET


@pytest.fixture(params=["the page", "its frame"])
def popup(browser, request):
    """The login page filled by the vault, then a page of the same site it opened (its « Aide », window.open; or one
    its own frame opened), where the agent went (switch_tab)."""
    browser.open("/login.html")
    browser.vault_fills()
    opens = "window" if request.param == "the page" else "frames[0]"
    with Cdp(browser.page_ws) as cdp:
        cdp.call("Runtime.evaluate", {"expression": f"{opens}.open('/forgot.html', 'aide', 'popup,width=600,height=500') "
                                                    "&& true", "userGesture": True})
    aide = target(browser, browser.base + "/forgot.html")
    drive(browser, aide["webSocketDebuggerUrl"])
    yield browser
    drive(browser, browser.page_ws)
    httpx.get(f"http://127.0.0.1:{browser.port}/json/close/{aide['id']}")


def test_a_page_opened_by_the_login_page_never_reads_it(popup):
    said = popup.tool("execute_script", script="location.href + ' / ' + opener.top.document.title")
    assert "/forgot.html / Espace agences | Essai" in said, said[:200]
    for script in ["opener.top.document.querySelector('#mdp').value",
                   codes("opener.top.document.querySelector('form input:nth-of-type(4)')['val' + 'ue']")]:
        said = popup.tool("execute_script", script=script)
        assert not leaks(said), (script, leaks(said), said[:200])
    assert popup.page("document.querySelector('#mdp').value") == SECRET


def test_a_copy_of_the_password_in_the_pages_text_reads_as_empty(browser):
    """The design sets aside « une copie exacte » during a script: in an input or an attribute, not in the page's text
    (fixtures/vault_readback/review/peek.html: the site's eye drawn as a box of text over the field)."""
    browser.open("/review/peek.html")
    browser.vault_fills()
    assert browser.page("document.querySelector('#peek').textContent") == SECRET
    said = browser.tool("execute_script", script=codes("document.querySelector('form output').textContent"))
    assert not leaks(said), (leaks(said), said[:200])


@pytest.fixture
def keypad(browser):
    """A bank's keypad: the person clicks the digits of the code, the page keeps it in a hidden field « password »."""
    browser.open("/review/keypad.html")
    with Cdp(browser.page_ws) as cdp:
        for digit in CODE:
            cdp.call("Runtime.evaluate", {"expression": f"document.querySelector('[data-d=\"{digit}\"]').click()",
                                          "userGesture": True})
    assert browser.page("document.querySelector('#password').value") == CODE
    return browser


def test_a_code_typed_on_a_banks_keypad_reads_as_nothing(keypad):
    for tool, args in [("execute_script", {"script": "document.querySelector('form input')['val' + 'ue']"}),
                       ("execute_script", {"script": codes("document.querySelector('form input')['val' + 'ue']")}),
                       ("query_elements", {"selector": "form input", "visible_only": False}),
                       ("get_page_content", {"include_frames": True})]:
        said = keypad.tool(tool, **args)
        assert not leaks(said, CODE), (tool, args, leaks(said, CODE), said[:300])


def test_an_xpath_doing_sums_on_a_value_tells_nothing(keypad):
    """A hidden field's « value » attribute is its value (the guard can't empty it before a tool): an XPath dividing it
    and rounding gives a numeric code one digit per call, past the selector rule (no starts-with, no comparison)."""
    said = {digit: keypad.tool("query_elements", selector=f"//input[floor(@value div 100000)={digit}]",
                               visible_only=False) for digit in (CODE[0], "7")}
    assert found(said[CODE[0]]) == found(said["7"]), (said[CODE[0]][:120], said["7"][:120])
    said = {digit: keypad.tool("query_elements", selector=f"//input[@value mod 10 = {digit}]", visible_only=False)
            for digit in (CODE[-1], "7")}
    assert found(said[CODE[-1]]) == found(said["7"]), (said[CODE[-1]][:120], said["7"][:120])


def test_another_sites_frame_shown_by_its_eye_is_dots_in_the_agents_screenshot(browser):
    """A partner's login in a frame of another site (fixtures pin.html served as http://localhost): the person typed
    the code, then clicked its eye. The guard's masking reaches the page's own frames only."""
    port = browser.site.server_address[1]
    (browser.dir / "site" / "review" / "xframe.html").write_text(
        "<!doctype html><html lang=fr><head><meta charset=utf-8><title>Partenaire | Essai</title></head><body>"
        f"<h1>Accès partenaire</h1><iframe src=\"http://localhost:{port}/pin.html\" width=700 height=400></iframe>"
        "</body></html>")
    browser.open("/review/xframe.html")
    frame = target(browser, f"http://localhost:{port}/pin.html", kind="iframe")
    with Cdp(frame["webSocketDebuggerUrl"]) as person:
        person.value(f"document.querySelector('#code').value = {json.dumps(PIN)}; true")
        person.call("Runtime.evaluate", {"expression": "document.querySelector('#voir').click()", "userGesture": True})
        assert person.value("document.querySelector('#code').type") == "text"
    shot = browser.image("take_screenshot")
    with Cdp(frame["webSocketDebuggerUrl"]) as person, Cdp(browser.page_ws) as eyes:
        person.value("document.querySelector('#code').type = 'password'")
        masked = eyes.call("Page.captureScreenshot", {"format": "jpeg"})["data"]
        person.value("document.querySelector('#code').type = 'text'")
        clear = eyes.call("Page.captureScreenshot", {"format": "jpeg"})["data"]
    assert masked != clear
    assert shot == masked, "the agent's screenshot shows the partner's code in clear"


def test_setting_aside_never_reloads_a_frame_of_the_page(browser):
    """A 4-digit code (a year, as people choose them) set aside with every attribute holding those digits: a frame of
    the same site whose address holds them is sent to an empty address and back, and what the person wrote there goes."""
    browser.open("/review/yearpin.html")
    browser.person_types("#code", "1987")
    with Cdp(browser.page_ws) as person:
        person.value("frames[0].document.querySelector('#note').value = 'brouillon de la personne'; true")
    said = browser.tool("execute_script", script="document.title")
    assert "Code" in said, said[:200]
    time.sleep(1.0)
    assert browser.page("frames[0].document.querySelector('#note').value") == "brouillon de la personne"


def test_a_recovery_e_mail_field_reads_as_typed(browser):
    """« Mot de passe oublié », the e-mail field named for the recovery (« passwordRecoveryInput »): not a password."""
    browser.open("/review/recover.html")
    browser.tool("type_text", selector="#passwordRecoveryInput", text="agence-nord@example.org")
    # (named so in a script, the text rules of 13/11 refuse it: read by its form)
    said = browser.tool("execute_script", script="document.querySelector('form input')['val' + 'ue']")
    assert "agence-nord@example.org" in said, said[:200]
    browser.image("take_screenshot")
    assert browser.page("document.querySelector('#passwordRecoveryInput').type") == "text"


@pytest.fixture
def remembered(browser):
    """A site that keeps what is typed in its storage (« se souvenir de moi », a form-saving library): the password
    out of the page, where the setting aside doesn't reach (confirmation, 16/11)."""
    browser.open("/review/remember.html")
    browser.vault_fills()
    assert json.loads(browser.page("localStorage.getItem('memo-connexion')"))["mdp"] == SECRET
    assert browser.page("sessionStorage.getItem('memo-mdp')") == SECRET
    return browser


def test_a_script_never_reads_the_sites_storage(remembered):
    """V8 takes a storage's getItem for a side effect, the text rules refuse the rest: refused, every way tried."""
    for script in [codes("localStorage.getItem('memo-connexion')"), codes("sessionStorage.getItem('memo-mdp')"),
                   codes("JSON.stringify(Object.entries(localStorage))"), codes("localStorage['memo-connexion']"),
                   codes("sessionStorage['memo-' + 'mdp']"), codes("window['session' + 'Storage'].getItem('memo-mdp')"),
                   codes("self.sessionStorage.getItem('memo-mdp')"),
                   codes("Storage.prototype.getItem.call(sessionStorage, 'memo-mdp')"),
                   codes("JSON.stringify({...sessionStorage})")]:
        said = remembered.tool("execute_script", script=script)
        print(f"\nstorage, {script[:70]}: {said[:80]!r}")
        assert not leaks(said), (script, leaks(said), said[:200])


def test_get_instance_state_never_gives_the_sites_storage(remembered):
    """get_instance_state gives every key and value of the site's localStorage and sessionStorage (and its cookies).
    Hermes doesn't offer it to the agent today (`tools.exclude` in browsers.rs `mcp_server`, set because it hangs on
    some pages), but the proxy relays it to whoever calls it, as it never does the `/state` and `/cookies` resources
    holding the same (fix review 11/11, R5)."""
    said = remembered.tool("get_instance_state")
    assert not leaks(said), (leaks(said), said[:300])
