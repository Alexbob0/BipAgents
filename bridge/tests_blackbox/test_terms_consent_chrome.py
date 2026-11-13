"""A site's terms box, ticked with the person's « Oui » (Alex, 13/11: a card Oui / Non, no longer the person taking
over the browser): the agent's `accept_terms` on the vault's entry, against the sign-up fixture (fixtures/signup,
modelled on a French pharmacy's page: a newsletter box and a terms box in a folded panel) in a real Chromium the
bridge reaches over DevTools, as in the agent's visible browser.

- « Oui »: that one box is ticked by the bridge, nothing else;
- « Non »: nothing ticked, the agent told to stop that sign-up;
- another box (the newsletter) or another site: refused, no card;
- no answer in time: nothing ticked, the card gone, a late « Oui » refused.

Needs a Chromium on this machine (`BIP_CHROME`, else Chrome, Chromium or Brave where they usually are)."""
import functools
import http.server
import json
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from bbkit.common import AUTH
from bbkit.servers import wait_until
from test_card_payments import TOKEN, pay_bridge
from test_card_payments_chrome import Browser, pytestmark  # noqa: F401  (skipped without a Chromium)

FIXTURES = Path(__file__).parent / "fixtures" / "signup"
VAULT = "/v1/agent-tools/vault"
TERMS = "J'accepte les conditions générales de vente"


def site():
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(FIXTURES))
    handler.log_message = lambda *a: None
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def accept(bridge, arguments):
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "accept_terms", "arguments": arguments}}
    resp = bridge.post(VAULT, json=body, headers={"Authorization": f"Bearer {TOKEN}"}, timeout=120)
    return resp.json()["result"]


def terms_cards(bridge):
    items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": "vie"}).json()["items"]
    return [i for i in items if i["run_id"].startswith("bip-terms-")]


def boxes(browser):
    return json.loads(browser.run("JSON.stringify([document.getElementById('cgu').checked, "
                                  "document.getElementById('newsletter').checked])", "signup.html"))


def ask(bridge, arguments, choice):
    """accept_terms as the agent calls it; the person answers its card with `choice` (None: no answer)."""
    with ThreadPoolExecutor(1) as pool:
        call = pool.submit(accept, bridge, arguments)
        assert wait_until(lambda: terms_cards(bridge), timeout=30), "no card"
        [card] = terms_cards(bridge)
        if choice is not None:
            answered = bridge.post("/v1/approve", headers=AUTH, json={"agent": "vie", "run_id": card["run_id"],
                                                                       "choice": choice})
            assert answered.status_code == 200, answered.text
        return call.result(timeout=120), card


def test_oui_ticks_that_box_only_non_stops_and_nothing_else_is_unlocked(bridges, fakes):
    shop = site()
    base = f"http://127.0.0.1:{shop.server_port}"
    browser = Browser(f"{base}/signup.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser)
        arguments = {"site": base, "box": TERMS, "terms_url": f"{base}/cgv"}
        # « Non »: nothing ticked, the sign-up stopped
        result, card = ask(bridge, arguments, "deny")
        assert result["isError"] is True and "Stop this sign-up" in result["content"][0]["text"], result
        assert card["choices"] == ["once", "deny"] and "bip_always" not in card, card   # no « Toujours »
        assert card["description"] == f"Vie veut accepter les conditions de 127.0.0.1:{shop.server_port} : « {TERMS} »"
        assert card["command"] == f"{base}/cgv"
        assert boxes(browser) == [False, False]
        # the newsletter is no terms box: no card, nothing ticked
        other = accept(bridge, {"site": base, "box": "Je souhaite recevoir la newsletter"})
        assert other["isError"] is True and "not the site's terms" in other["content"][0]["text"], other
        # another site than the page's: no card
        elsewhere = accept(bridge, {"site": "https://other.example", "box": TERMS})
        assert elsewhere["isError"] is True and "not https://other.example" in elsewhere["content"][0]["text"]
        assert not terms_cards(bridge)
        # « Oui »: that box ticked by the bridge, the newsletter left as it was
        result, _ = ask(bridge, arguments, "once")
        assert result["isError"] is False and result["content"][0]["text"].startswith(f"Ticked « {TERMS} »"), result
        assert boxes(browser) == [True, False]
        # nothing kept unlocked: the box unticked again asks again
        browser.run("document.getElementById('cgu').checked = false; 'ok'", "signup.html")
        _, again = ask(bridge, arguments, "deny")
        assert again["run_id"] != card["run_id"] and boxes(browser) == [False, False]
    finally:
        browser.stop()
        shop.shutdown()


def test_no_answer_in_time_ticks_nothing_and_a_late_yes_is_refused(bridges, fakes):
    shop = site()
    base = f"http://127.0.0.1:{shop.server_port}"
    browser = Browser(f"{base}/signup.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser,
                            change=lambda config: config.setdefault("browser", {}).update({"consent_seconds": 3}))
        result, card = ask(bridge, {"site": base, "box": TERMS}, None)
        assert result["isError"] is True and "did not answer in time" in result["content"][0]["text"], result
        assert not terms_cards(bridge)
        late = bridge.post("/v1/approve", headers=AUTH, json={"agent": "vie", "run_id": card["run_id"], "choice": "once"})
        assert late.status_code == 404, late.text
        time.sleep(0.5)
        assert boxes(browser) == [False, False]
    finally:
        browser.stop()
        shop.shutdown()
