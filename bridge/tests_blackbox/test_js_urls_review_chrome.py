"""Independent review of refuse-javascript-urls (88f6faaff), 14/11, on a real Chrome (BIP_CHROME: the production
image's Chrome 157 binary) behind the stand-in for stealth-browser-mcp, through bip-browser-mcp (BIP_BROWSER_MCP),
with test_signup_guard_chrome.py's harness. Kept as written. Only Visa's published test number.

rj1 (the branch's own target, every spelling) and rj4/rj5 (a javascript: link clicked: the click's own checks) passed
on 88f6faaff; rj2/rj3 (blob:, filesystem:) failed there. The review's rj6 (a page's javascript: link whose write comes
later) is card-guards' ground (its check before each call), not this branch's: left out here."""
import base64
import json
import re
import shutil
import time
from pathlib import Path

import pytest

import test_signup_guard_chrome as s
from test_signup_guard_chrome import PASSWORD_UNDONE, Session

pytestmark = s.pytestmark
HERE = Path(__file__).parent / "fixtures" / "js_urls_review"
NUMBER = "4242424242424242"   # Visa's published test number

SPELLINGS = [
    ("plain", "javascript:void(document.getElementById('numero').value='{v}')"),
    ("leading newline and tab", "\n\tjavascript:void(document.getElementById('numero').value='{v}')"),
    ("mixed case, leading space", " JaVaScRiPt:void(document.getElementById('numero').value='{v}')"),
    ("newline inside", "java\nscript:void(document.getElementById('numero').value='{v}')"),
    ("tab before the colon", "javascript\t:void(document.getElementById('numero').value='{v}')"),
    ("leading control characters", "\x01\x1fjavascript:void(document.getElementById('numero').value='{v}')"),
    ("percent-encoded", "java%73cript:void(document.getElementById('numero').value='{v}')"),
    ("HTML entity", "&#106;avascript:void(document.getElementById('numero').value='{v}')"),
    ("space before the colon", "JaVaScRiPt :void(document.getElementById('numero').value='{v}')"),
    ("no-break space first", " javascript:void(document.getElementById('numero').value='{v}')"),
    ("zero-width space first", "​javascript:void(document.getElementById('numero').value='{v}')"),
    ("byte order mark first", "﻿javascript:void(document.getElementById('numero').value='{v}')"),
    ("vbscript", "vbscript:document.getElementById('numero').value='{v}'"),
    ("data", "data:text/html,<script>document.title='{v}'</script>"),
]

# what a document made by the agent's script does once opened: the page's own form in a frame of the same origin
# (its card number, its password, its terms), with none of the guard's checks around it
PAYLOAD = ("<script>f.onload = () => { const d = f.contentDocument; d.getElementById('numero').value = '" + NUMBER + "'; "
           "d.getElementById('pw').value = 'Provisoire123!'; d.getElementById('cgu').checked = true; "
           "document.title = 'ran'; };</script>")
B64 = base64.b64encode(PAYLOAD.encode()).decode()
FRAME = "'<iframe id=\"f\" src=\"' + location.origin + '/jspage.html\"></iframe>' + atob('" + B64 + "')"
INSIDE = ("(() => { const f = document.getElementById('f'); const d = f && f.contentDocument; return d ? "
          "[location.protocol, document.title, d.getElementById('numero').value, d.getElementById('pw').value, "
          "d.getElementById('cgu').checked] : [location.protocol, document.title]; })()")


@pytest.fixture(scope="module")
def browser():
    session = Session()
    shutil.copy(HERE / "jspage.html", session.dir / "site" / "jspage.html")
    yield session
    session.close()


def open_page(browser):
    browser.tool("navigate", url=f"{browser.base}/jspage.html")
    deadline = time.time() + 10
    while time.time() < deadline and browser.page("document.readyState") != "complete":
        time.sleep(0.1)


def test_rj1_no_spelling_of_a_script_address_runs_in_the_page(browser):
    open_page(browser)
    ran, seen = [], []
    for i, (name, url) in enumerate(SPELLINGS):
        mark = f"S{i}"
        said = browser.tool("navigate", url=url.format(v=mark))
        time.sleep(0.4)
        got = browser.page("[document.getElementById('numero') ? document.getElementById('numero').value : null, "
                           "document.title, location.href]")
        seen.append(f"{name}: proxy said {said[:70]!r}; page {got!r}")
        if mark in (got[0], got[1]):
            ran.append(seen[-1])
        if got[0] is None or not got[2].endswith("/jspage.html"):
            open_page(browser)
        else:
            browser.page("document.getElementById('numero').value = ''")
    print("\n".join(seen))
    assert not ran, "ran in the page:\n" + "\n".join(ran)


def test_rj2_a_blob_address_made_by_a_script_never_runs(browser):
    open_page(browser)
    # the blob made by the page itself: the agent's own script, with its atob(…), is refused since card-guards'
    # second review (a way out written so its words don't show); its address is what navigate must refuse
    made = browser.page(f"(() => URL.createObjectURL(new Blob([{FRAME}], {{type: 'text/html'}})))()")
    url = re.search(r"blob:[^\"\\\s]+", made)
    assert url, f"no blob address made: {made!r}"
    said = browser.tool("navigate", url=url.group(0))
    time.sleep(2)
    got = browser.page(INSIDE)
    assert len(got) == 2 or (got[2] == "" and got[3] == "" and got[4] is False), (
        f"navigate {url.group(0)} said {said[:120]!r}; the shop's form, opened by it: {got!r}")


def test_rj3_a_filesystem_address_made_by_a_script_never_runs(browser):
    open_page(browser)
    # made by the page itself, as rj2's blob (the agent's own script, with its atob(…) and its callbacks, is refused)
    started = browser.page((
        "(() => { if (!window.webkitRequestFileSystem) return 'no filesystem API'; const h = " + FRAME + "; "
        "webkitRequestFileSystem(window.TEMPORARY, 1 << 20, (fs) => fs.root.getFile('p.html', {create: true}, (e) => "
        "e.createWriter((w) => { w.onwriteend = () => { document.title = e.toURL(); }; "
        "w.write(new Blob([h], {type: 'text/html'})); })), (err) => { document.title = 'fs error ' + err.name; }); "
        "return 'started'; })()"))
    time.sleep(1.5)
    url = browser.page("document.title")
    if not str(url).startswith("filesystem:"):
        print(f"no filesystem: address made ({started!r}, title {url!r})")
        return
    said = browser.tool("navigate", url=url)
    time.sleep(2)
    got = browser.page(INSIDE)
    assert len(got) == 2 or (got[2] == "" and got[3] == "" and got[4] is False), (
        f"navigate {url} said {said[:120]!r}; the shop's form, opened by it: {got!r}")


def test_rj4_a_javascript_link_of_the_page_clicked_is_checked_by_the_click(browser):
    open_page(browser)
    said = browser.tool("click_element", selector="#jslink")
    assert browser.page("document.getElementById('pw').value") == "" and said.startswith(PASSWORD_UNDONE), said


def test_rj5_a_javascript_link_the_agent_adds_then_clicks_is_checked_by_the_click(browser):
    open_page(browser)
    link = "<a id=\"mine\" href=\"javascript:void(document.getElementById('pw').value='Provisoire123!')\">x</a>"
    added = browser.tool("execute_script", script="document.body.insertAdjacentHTML('beforeend', atob('"
                         + base64.b64encode(link.encode()).decode() + "')); 'ok'")
    said = browser.tool("click_element", selector="#mine")
    assert browser.page("document.getElementById('pw').value") == "", (added, said)
