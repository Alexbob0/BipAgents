"""Independent review of select-option-direct (14/11, « à corriger »): the proxy's own select_option in a real Chromium,
behind the branch's stand-in for stealth-browser-mcp (fixtures/signup_guard/stealth_cdp.py), and behind a copy of
browser/hold_gate.py (its flag in the session's folder) for the person's hold and a card payment.

Each test says what should happen; the « finding » ones failed on a4c364653 (the review's scratch tests, brought into
the branch). Needs a Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`): skipped without."""
import json
import os
import queue
import subprocess
import sys
import threading
import time
from pathlib import Path

import pytest

import test_signup_guard_chrome as guard
from test_signup_guard_chrome import Session, pytestmark  # noqa: F401

ROOT = Path(__file__).resolve().parent.parent.parent
PERSON_HOLDS = "Not done: your person has taken over your browser"

LISTS_PAGE = """<!doctype html><html><head><meta charset="utf-8"><title>lists</title></head><body>
<form id="f" action="/sent.html" method="get">
  <label for="email">E-mail</label><input id="email" name="email" type="email">
  <label for="pw">Mot de passe</label><input id="pw" name="pw" type="password">
  <div id="birthday" class="birthday">
    <select id="d" name="day"><option value="">Jour</option>%DAYS%</select>
    <select id="m" name="month"><option value="">Mois</option>%MONTHS%</select>
    <select id="y" name="year"><option value="">Année</option>%YEARS%</select>
  </div>
  <label for="city">Ville</label><input id="city" name="city" type="text" list="villes">
  <datalist id="villes"><option value="Paris"><option value="Lyon"></datalist>
  <label for="bday">Date de naissance</label><input id="bday" name="bday" type="date">
  <label for="cgv">J'accepte les conditions générales de vente</label>
  <select id="cgv" name="cgv"><option value="">--</option><option value="1">Oui</option><option value="0">Non</option></select>
  <span id="pays-label">Pays</span>
  <div id="pays" role="combobox" aria-haspopup="listbox" aria-labelledby="pays-label pays" tabindex="0">Belgique</div>
  <select id="off" name="off" disabled><option value="a">A</option><option value="b">B</option></select>
  <button type="submit" id="send">Envoyer</button>
</form>
<script>
setTimeout(() => { const s = document.createElement('select'); s.id = 'late'; s.innerHTML =
  '<option value="">--</option><option value="x">X</option>'; document.getElementById('f').appendChild(s); }, 1500);
</script>
</body></html>"""

MONTHS = ["Janvier", "Février", "Mars", "Avril", "Mai", "Juin", "Juillet", "Août", "Septembre", "Octobre", "Novembre",
          "Décembre"]


def page_html():
    days = "".join(f'<option value="{d:02d}">{d:02d}</option>' for d in range(1, 32))
    months = "".join(f'<option value="{m:02d}">{name}</option>' for m, name in enumerate(MONTHS, 1))
    years = "".join(f'<option value="{y}">{y}</option>' for y in range(2026, 1899, -1))
    return LISTS_PAGE.replace("%DAYS%", days).replace("%MONTHS%", months).replace("%YEARS%", years)


@pytest.fixture(scope="module")
def browser():
    session = Session()
    (session.dir / "site" / "lists.html").write_text(page_html(), encoding="utf-8")
    yield session
    session.close()


def open_lists(browser):
    browser.tool("navigate", url=f"{browser.base}/lists.html")


def raw(browser, name, **arguments):
    answer = browser.ask("tools/call", {"name": name, "arguments": {"instance_id": "i", **arguments}})
    assert "error" not in answer, answer
    return answer["result"]


# --- findings (fail on a4c364653) -------------------------------------------------------------------------------------

@pytest.mark.parametrize("selector", ["#birthday", "select"])
def test_finding_a_selector_holding_several_lists_is_never_set_in_its_first_one_silently(browser, selector):
    # « #birthday » holds the day, the month and the year; « select » (GLM-5.3 Flash on the bench, day2 run 6 and 9)
    # matches all of them: « 03 » meant for the month lands in the day, and the answer says « Chosen in that list »
    # without naming which one
    open_lists(browser)
    said = browser.tool("select_option", selector=selector, value="03")
    day = browser.page("document.getElementById('d').value")
    assert day == "" or "#d" in said, f"day set to {day!r}, answer: {said}"
    # the list a choice went into is named
    said = browser.tool("select_option", selector="#m", value="03")
    assert said.startswith("Chosen in that list (#m)") and browser.page("document.getElementById('m').value") == "03"


def test_finding_a_field_to_type_into_is_not_called_a_list_of_the_sites_own(browser):
    # a text field with a datalist, a date field: select_option on them is told « a list of the site's own:
    # click_element on it, then click_element on the choice », a way that leads nowhere (type_text is)
    open_lists(browser)
    for selector in ("#city", "#bday"):
        said = browser.tool("select_option", selector=selector, value="Paris" if selector == "#city" else "1991-03-14")
        assert said.startswith("Not done:"), said
        assert "list of the site's own: click_element" not in said.split(". The page's")[0], said
        assert "type_text" in said, said


@pytest.mark.parametrize("choice", [{"text": "Oui"}, {"value": "1"}])
def test_finding_a_consent_in_a_drop_down_list_is_never_given_by_the_agent(browser, choice):
    # « J'accepte les conditions générales de vente : Oui / Non » as a <select>: the box of the same label is refused
    # (TERMS_REFUSED, accept_terms); the list is chosen. Pre-existing gap of fix-signup-hold's guard (boxes only): the
    # real server's select_option sets it too, by `text` (typed: Chrome's type-ahead) or by `value` when it is the
    # page's first list set (both reproduced by the stand-in)
    open_lists(browser)
    said = browser.tool("select_option", selector="#cgv", **choice)
    assert browser.page("document.getElementById('cgv').value") == "", said
    assert said.startswith(guard.TERMS_REFUSED), said
    # « Non » gives nothing: chosen
    said = browser.tool("select_option", selector="#cgv", text="Non")
    assert browser.page("document.getElementById('cgv').value") == "0", said


# --- checks that hold (pass on a4c364653) ------------------------------------------------------------------------------

def test_refusals_answer_without_is_error_and_fit_the_schema(browser):
    open_lists(browser)
    for arguments in ({"selector": "#nothing", "value": "1"}, {"selector": "#email", "value": "1"},
                      {"selector": "#m", "value": "13"}, {"selector": "#off", "value": "b"},
                      {"selector": "//*[", "value": "1"}):
        result = raw(browser, "select_option", **arguments)
        assert "isError" not in result, (arguments, result)
        assert result.get("structuredContent") == {"result": False}, (arguments, result)
        assert result["content"][0]["text"].startswith("Not done:"), (arguments, result)
    done = raw(browser, "select_option", selector="#m", text="mars")
    assert "isError" not in done and done["structuredContent"] == {"result": True}, done
    assert browser.page("document.getElementById('m').value") == "03"


def test_the_lists_named_hold_no_field_value_nor_password(browser):
    open_lists(browser)
    browser.page("(() => { const set = (id, v) => { const e = document.getElementById(id); e.value = v; }; "
                 "set('pw', 'Zq9!secret-pw'); set('email', 'camille.secret@example.org'); set('city', 'Quimperlé'); "
                 "return true; })()")
    said = browser.tool("select_option", selector='select[name="civilite"]', text="Madame")
    assert said.startswith("Not done:") and "#m" in said, said
    for secret in ("Zq9!secret-pw", "camille.secret", "Quimperlé"):
        assert secret not in said, said
    # the site's own list labelled by itself (MUI's way): its current choice is in its label, as on the page itself
    print("\nLISTS ANSWER:", said)


def test_a_disabled_list_an_invalid_selector_and_a_late_list(browser):
    open_lists(browser)
    said = browser.tool("select_option", selector="#off", value="b")
    assert said.startswith("Not done: the site has that list"), said
    started = time.time()
    said = browser.tool("select_option", selector="//*[", value="1")
    assert said.startswith("Not done: nothing on the page") and time.time() - started < 2.5, said
    open_lists(browser)
    said = browser.tool("select_option", selector="#late", value="x")
    assert said.startswith("Chosen in that list"), said


def test_no_click_no_submit_no_box(browser):
    # the choice fires input and change only: no click, no form sent, no box ticked
    open_lists(browser)
    browser.page("window.seen = []; for (const t of ['click', 'submit', 'input', 'change']) "
                 "document.addEventListener(t, (e) => window.seen.push(t + ':' + e.target.id), true); true")
    browser.tool("select_option", selector="#y", value="1991")
    assert browser.page("window.seen") == ["input:y", "change:y"]
    assert browser.page("location.pathname") == "/lists.html"


# --- the person's hold and a card payment (hold_gate.py in front of the stand-in) --------------------------------------

class HeldSession(Session):
    """A Session whose server is behind a copy of hold_gate.py, its flag and counts in the session's folder."""

    def __init__(self):  # noqa: D107 - same as Session's, the server's command changed
        import functools
        import http.server
        import shutil
        import tempfile
        import httpx
        from bbkit.servers import free_port
        self.dir = Path(tempfile.mkdtemp(prefix="bip-sod-hold-"))
        site = self.dir / "site"
        shutil.copytree(guard.FIXTURES, site)
        (site / "lists.html").write_text(page_html(), encoding="utf-8")
        self.flag = self.dir / "hold"
        gate = (ROOT / "browser" / "hold_gate.py").read_text()
        gate = gate.replace('HOLD = "/tmp/bipagents-hold"', f'HOLD = {json.dumps(str(self.flag))}')
        gate = gate.replace('CALLS = "/tmp/bipagents-calls"', f'CALLS = {json.dumps(str(self.dir / "calls"))}')
        assert str(self.flag) in gate and str(self.dir / "calls") in gate
        (self.dir / "hold_gate.py").write_text(gate)
        handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(site))
        handler.log_message = lambda *a: None
        self.site = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        threading.Thread(target=self.site.serve_forever, daemon=True).start()
        self.base = f"http://127.0.0.1:{self.site.server_address[1]}"
        port = free_port()
        profile = self.dir / "profile"
        profile.mkdir()
        self.chrome = subprocess.Popen([guard.CHROME, "--headless=new", f"--remote-debugging-port={port}",
                                        f"--user-data-dir={profile}", "--no-first-run", "--disable-gpu",
                                        "about:blank"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline, page = time.time() + 30, None
        while time.time() < deadline and page is None:
            try:
                page = next(t for t in httpx.get(f"http://127.0.0.1:{port}/json/list").json() if t["type"] == "page")
            except (httpx.HTTPError, ValueError, StopIteration):
                time.sleep(0.3)
        assert page, "no Chromium page"
        self.page_ws = page["webSocketDebuggerUrl"]
        (self.dir / "credit.json").write_text(json.dumps({"state": "ok"}))
        config = {"start": ["true"], "ready": ["true"], "active": ["true"], "stop": ["true"],
                  "server": [sys.executable, str(self.dir / "hold_gate.py"), sys.executable,
                             str(site / "stealth_cdp.py"), self.page_ws],
                  "cache": str(self.dir / "cache.json"), "busy": str(self.dir / "busy"),
                  "credit": str(self.dir / "credit.json"), "cdp_port": free_port(), "idle": 600}
        (self.dir / "config.json").write_text(json.dumps(config))
        # these pages' own site may get the forms (form_send.rs, audit 1.2: its own test asks otherwise)
        (self.dir / "send-sites.json").write_text(json.dumps({"always": ["127.0.0.1", "localhost"], "once": []}))
        self.proxy = subprocess.Popen([guard.PROXY, str(self.dir / "config.json")], stdin=subprocess.PIPE,
                                      stdout=subprocess.PIPE, stderr=open(self.dir / "proxy.log", "wb"))
        self.answers = queue.Queue()
        threading.Thread(target=self._read, daemon=True).start()
        self.id = 0
        self.ask("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "hermes"}})
        self.ask("tools/list", {})


def test_the_persons_hold_and_a_card_payment_keep_the_lists_as_they_are():
    session = HeldSession()
    try:
        session.tool("navigate", url=f"{session.base}/lists.html")
        session.flag.write_text("person")
        result = raw(session, "select_option", selector="#y", value="1991")
        assert "isError" not in result and result["content"][0]["text"].startswith(PERSON_HOLDS), result
        assert session.page("document.getElementById('y').value") == ""
        session.flag.write_text("card tok")
        result = raw(session, "select_option", selector="#y", value="1991")
        assert "isError" not in result and result["content"][0]["text"].startswith("A card payment"), result
        assert session.page("document.getElementById('y').value") == ""
        os.remove(session.flag)
        said = session.tool("select_option", selector="#y", value="1991")
        assert said.startswith("Chosen in that list") and session.page("document.getElementById('y').value") == "1991"
        log = (session.dir / "proxy.log").read_text()
        print("\nPROXY LOG:", log[-1500:])
    finally:
        session.close()
