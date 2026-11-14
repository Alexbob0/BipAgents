"""Third review of card-guards (68729945f): my rj2/rj3 (refuse-js review) made their blob:/filesystem: document with an
atob script, now refused first; here the PAGE makes it (as a site's own script may), and navigate must still refuse
it. Should pass. Never pushed."""
import time

import test_signup_guard_chrome as s
from test_js_urls_review_chrome import B64, FRAME, INSIDE, browser, open_page   # noqa: F401 (the fixture)

pytestmark = s.pytestmark


def test_rx5_a_blob_address_the_page_made_never_runs(browser):
    open_page(browser)
    url = browser.page(f"URL.createObjectURL(new Blob([{FRAME}], {{type: 'text/html'}}))")
    said = browser.tool("navigate", url=url)
    time.sleep(1.5)
    got = browser.page(INSIDE)
    assert said.startswith("navigate refused") and (len(got) == 2 or got[2] == ""), (url, said[:100], got)


def test_rx6_a_filesystem_address_the_page_made_never_runs(browser):
    open_page(browser)
    browser.page("(() => { if (!window.webkitRequestFileSystem) { document.title = 'none'; return; } const h = " + FRAME
                 + "; webkitRequestFileSystem(window.TEMPORARY, 1 << 20, (fs) => fs.root.getFile('q.html', {create: true}, "
                 "(e) => e.createWriter((w) => { w.onwriteend = () => { document.title = e.toURL(); }; "
                 "w.write(new Blob([h], {type: 'text/html'})); }))); })()")
    time.sleep(1.5)
    url = browser.page("document.title")
    if not str(url).startswith("filesystem:"):
        return
    said = browser.tool("navigate", url=url)
    time.sleep(1.5)
    got = browser.page(INSIDE)
    assert said.startswith("navigate refused") and (len(got) == 2 or got[2] == ""), (url, said[:100], got)
