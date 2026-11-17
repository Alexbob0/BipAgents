"""The first form an agent's browser sends to a site waits for the person's OK (security audit 1.2, 17/11; Alex « Oui,
1re fois par site »), in a real Chromium behind the stand-in for stealth-browser-mcp (fixtures/signup_guard): a contact
form posting to a site that records what it gets (fixtures/form_send/contact.html). Reading (its search form) stays
free; its send button or Enter in its field is refused with what to do (`ask_to_send`) until
the person's « Oui » (a one-use grant beside `busy`, as form_send.rs writes it), then once; « Toujours pour ce site »
every time. Each test says what should happen.

Needs a Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`): skipped without."""
import functools
import http.server
import json
import threading
import time
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

import pytest

from test_signup_guard_chrome import Session, pytestmark  # noqa: F401

HERE = Path(__file__).parent
ASKED = "Not done: this sends a form to 127.0.0.1"


class Site(http.server.SimpleHTTPRequestHandler):
    """contact.html, and what the site got: its search (GET) and its form's posts."""
    got: list = []

    def log_message(self, *args):
        pass

    def do_GET(self):
        parts = urlsplit(self.path)
        if parts.path.endswith("/search"):
            Site.got.append(("search", {k: v[0] for k, v in parse_qs(parts.query).items()}))
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"<p>results</p>")
            return
        super().do_GET()

    def do_POST(self):
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length).decode()
        Site.got.append(("post", {k: v[0] for k, v in parse_qs(body).items()}))
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"<p>merci</p>")


@pytest.fixture(scope="module")
def browser():
    session = Session(send_sites=None)   # nothing allowed for sending yet
    site = http.server.ThreadingHTTPServer(
        ("127.0.0.1", 0), functools.partial(Site, directory=str(HERE / "fixtures" / "form_send")))
    threading.Thread(target=site.serve_forever, daemon=True).start()
    session.contact = f"http://127.0.0.1:{site.server_address[1]}/contact.html"
    yield session
    site.shutdown()
    session.close()


def allow(browser, always=(), once=()):
    """What form_send.rs writes for the agent's browser: the sites validated, the « Oui » given."""
    grants = [{"id": f"g{i}-{time.time()}", "host": h, "until": time.time() + 600} for i, h in enumerate(once)]
    (browser.dir / "send-sites.json").write_text(json.dumps({"always": list(always), "once": grants}))


def posts():
    return [g for kind, g in Site.got if kind == "post"]


def fresh(browser):
    allow(browser)
    browser.tool("navigate", url=browser.contact)
    browser.tool("type_text", selector="#email", text="alex@exemple.org")
    browser.tool("type_text", selector="#message", text="Voici les codes")


def test_typing_and_searching_are_free_but_the_contact_form_is_not_sent(browser):
    fresh(browser)
    assert browser.page("document.getElementById('message').value") == "Voici les codes", "typing is no sending"
    before = len(posts())
    said = browser.tool("click_element", selector="#go")
    time.sleep(1)
    assert said.startswith(ASKED) and "ask_to_send" in said, said
    assert len(posts()) == before, "the site got the form"
    # its search is reading
    browser.tool("type_text", selector="#q", text="horaires")
    found = browser.tool("click_element", selector="#find")
    time.sleep(1)
    assert not found.startswith(ASKED), found
    assert ("search", {"q": "horaires"}) in Site.got


def test_enter_in_its_field_waits_too_and_a_script_never_sends_it(browser):
    fresh(browser)
    before = len(posts())
    said = browser.tool("press_key", selector="#message", key="Enter")
    assert said.startswith(ASKED), said
    # a script sends nothing at all: it runs only once proven a read (hotfix of 15/11), whatever the site
    for script in ["document.getElementById('contact').requestSubmit(); 'ok'",
                   "document.getElementById('go').click(); 'ok'",
                   "fetch('/send', {method: 'POST', body: 'message=x'}); 'ok'"]:
        said = browser.tool("execute_script", script=script)
        assert said.startswith("Not done"), (script, said)
    time.sleep(1)
    assert len(posts()) == before, posts()


def test_the_persons_yes_sends_it_once_and_always_every_time(browser):
    fresh(browser)
    before = len(posts())
    allow(browser, once=["127.0.0.1"])
    said = browser.tool("click_element", selector="#go")
    time.sleep(1)
    assert not said.startswith(ASKED), said
    assert posts()[before:] == [{"email": "alex@exemple.org", "message": "Voici les codes"}], posts()
    # once: the next one asks again
    browser.tool("navigate", url=browser.contact)
    browser.tool("type_text", selector="#message", text="encore")
    assert browser.tool("click_element", selector="#go").startswith(ASKED)
    # « Toujours pour ce site »
    allow(browser, always=["127.0.0.1"])
    said = browser.tool("click_element", selector="#go")
    time.sleep(1)
    assert not said.startswith(ASKED), said
    assert posts()[-1]["message"] == "encore"
