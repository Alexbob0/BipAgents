"""Audit Mistral 17/11, section 2 (Argent): the verifier's proofs (red on rc-1118 101ab5cd9), green with the fixes.
Each test states the safe behaviour. Fake shop and fake DevTools (test_card_payments.FakeChrome), real Chromium for
2.2's page; only Visa's published test number. 2.4 (a public suffix as a card's site) is bridge-rs/tests/
audit_payments.rs."""
import http.server
import functools
import json
import re
import threading
import time

import httpx

from bbkit.common import AUTH
from bbkit.mcp import give_token
from test_card_payments import ARGS, CARD, CODE, PAY, TOKEN, FakeChrome, pay, pay_bridge, tool, turn_on

TOKEN_LEA = "agent-token-for-lea-0123456789abcdef"


def card_of(bridge):
    return bridge.get("/v1/vault", headers=AUTH).json()["items"][0]["card"]


# -- 2.1 -------------------------------------------------------------------------------------------------------------

def test_2_1_a_second_confirm_never_presses_the_pay_button_again(bridges, fakes):
    chrome = FakeChrome()
    chrome.after_click = chrome.url   # the shop is slow to answer: the page stays where it is after the click
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        result, _ = pay(bridge, ARGS)
        assert result["isError"] is False, result
        first = tool(bridge, "confirm_payment", {"button": "Payer 12,50 €"})["content"][0]["text"]
        assert "has not moved on yet" in first, first
        # the agent, told nothing moved, tries again
        second = tool(bridge, "confirm_payment", {"button": "Payer 12,50 €"})["content"][0]["text"]
        assert chrome.clicks == ["https://shop.example/checkout"], (
            f"the pay button was pressed {len(chrome.clicks)} times; 2nd answer: {second}")
    finally:
        chrome.stop()


# -- 2.2 -------------------------------------------------------------------------------------------------------------

PAGE = """<!doctype html><html lang="fr"><head><meta charset="utf-8"><title>Paiement</title></head><body>
<p id="total">Total de la commande : 1 250,00 €</p>
<form onsubmit="event.preventDefault(); location.href='done.html'">
  <p><label>Numéro de carte <input id="numero" autocomplete="cc-number"></label></p>
  <p><label>Expiration <input id="exp" autocomplete="cc-exp" placeholder="MM/AA"></label></p>
  <p><label>Cryptogramme <input id="crypto" autocomplete="cc-csc"></label></p>
  <p><label>Titulaire <input id="titulaire" autocomplete="cc-name"></label></p>
  <p><button type="submit">Payer 1 250,00 €</button></p>
</form></body></html>"""


def test_2_2_the_pay_button_for_another_total_is_never_pressed_fake(bridges, fakes):
    chrome = FakeChrome()
    chrome.button = "payer 1 250,00 €"   # what findButton finds: the page's total is 100 times the approved one
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        result, asked = pay(bridge, ARGS)   # the agent says 12,50: the person approves 12.50 EUR
        assert result["isError"] is False and " 12.50 EUR on shop.example" in asked, (result, asked)
        confirmed = tool(bridge, "confirm_payment", {"button": "Payer 12,50 €"})["content"][0]["text"]
        assert chrome.clicks == [], f"approved 12.50 EUR, pressed « payer 1 250,00 € »: {confirmed}"
        # handed over to the person: the card erased, the checkout page again, a « Panier prêt » with the page's total
        assert "1250.00 EUR" in confirmed and "was not pressed" in confirmed, confirmed
        assert chrome.url == "https://shop.example/checkout" and chrome.navigated[-1] == "https://shop.example/checkout"
        assert ("F-psp", 0) in chrome.emptied and bridge.get("/v1/vault/payments", headers=AUTH).json()["active"] == []
        items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": "vie"}).json()["items"]
        carts = [i for i in items if i["run_id"].startswith("bip-cart-")]
        assert len(carts) == 1, items
        summary = carts[0]["bip_summary"]   # the agent's language: « Not paid: shop.example asks €1250.00 instead… »
        assert "1250" in summary and ("12.50" in summary or "12,50" in summary), summary
        assert "Panier prêt" in carts[0]["description"] or "1250" in carts[0]["description"], carts[0]
    finally:
        chrome.stop()


def test_2_2_the_pay_button_for_another_total_is_never_pressed_chromium(bridges, fakes, tmp_path):
    from test_card_payments_chrome import CHROME, Browser
    import pytest
    if CHROME is None:
        pytest.skip("no Chromium")
    (tmp_path / "pay.html").write_text(PAGE, encoding="utf-8")
    (tmp_path / "done.html").write_text("<!doctype html><title>Merci</title><p>Commande payée", encoding="utf-8")
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(tmp_path))
    handler.log_message = lambda *a: None
    shop = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=shop.serve_forever, daemon=True).start()
    browser = Browser(f"http://127.0.0.1:{shop.server_port}/pay.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": ["127.0.0.1"]})
        result, asked = pay(bridge, ARGS)
        assert result["isError"] is False and " 12.50 EUR on 127.0.0.1" in asked, (result, asked)
        confirmed = tool(bridge, "confirm_payment", {"button": "Payer 12,50 €"})["content"][0]["text"]
        assert "moved on" not in confirmed and [u.split("/")[-1] for u in browser.pages()] == ["pay.html"], (
            f"approved 12.50 EUR, the page's own « Payer 1 250,00 € » was pressed: {confirmed}; pages {browser.pages()}")
    finally:
        browser.stop()
        shop.shutdown()


# -- 2.3 -------------------------------------------------------------------------------------------------------------

def pay_held(bridge, token, arguments, asked, go, out):
    """pay_with_card as Hermes calls it for the agent of `token`; the question answered (code, then Hermes' yes) only
    once `go` is set; `asked` set when the question came."""
    body = {"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "pay_with_card", "arguments": arguments}}
    headers = {"Authorization": f"Bearer {token}", "Accept": "application/json, text/event-stream"}
    try:
        with httpx.Client(timeout=120) as client, client.stream("POST", bridge.url + PAY, json=body, headers=headers) as resp:
            for line in resp.iter_lines():
                if not line.startswith("data: "):
                    continue
                message = json.loads(line[6:])
                if message.get("method") == "elicitation/create":
                    out["asked"] = message["params"]["message"]
                    asked.set()
                    assert go.wait(90)
                    payment = re.search(r"Card payment (pay_[0-9a-f]{12}):", out["asked"]).group(1)
                    sent = bridge.post(f"/v1/vault/payments/{payment}/approve", headers=AUTH, json={"code": CODE})
                    out["approve"] = sent.status_code
                    answer = {"jsonrpc": "2.0", "id": message["id"], "result": {"action": "accept", "content": {}}}
                    client.post(bridge.url + PAY, json=answer, headers=headers)
                elif message.get("id") == 7:
                    out["result"] = message["result"]
                    return
    except Exception as exc:   # noqa: BLE001 - reported by the test
        out["error"] = repr(exc)


def test_2_3_two_agents_never_pay_over_the_cards_cap_together(bridges, fakes):
    vie, lea = FakeChrome(), FakeChrome()

    def change(config):
        config["agents"]["Lea"] = {**config["agents"]["Vie"],
                                   "python": config["agents"]["Vie"]["python"].replace("hermes-vie", "hermes-lea")}

    def prepare(bridge):
        folder = bridge.data / "browsers" / "lea"
        folder.mkdir(parents=True)
        (folder / "ports.json").write_text(json.dumps({"cdp": lea.port, "vnc": lea.port + 1}))

    try:
        bridge = pay_bridge(bridges, fakes, vie, change=change, prepare=prepare)
        give_token(bridge, "lea", TOKEN_LEA)
        turn_on(bridge)
        made = bridge.post("/v1/vault", headers=AUTH, json={**CARD, "agents": ["vie", "lea"], "cap_eur": "20"})
        assert made.status_code == 201, made.text
        assert card_of(bridge)["cap_cents"] == 2000
        runs = {}
        for name, token in (("vie", TOKEN), ("lea", TOKEN_LEA)):
            runs[name] = {"asked": threading.Event(), "go": threading.Event(), "out": {}}
            r = runs[name]
            r["thread"] = threading.Thread(target=pay_held, args=(bridge, token, ARGS, r["asked"], r["go"], r["out"]))
            r["thread"].start()
            # both questions are on the person's phone at once (12.50 EUR each, cap 20 EUR, nothing spent yet)
            assert r["asked"].wait(30), (name, r["out"])
        runs["vie"]["go"].set()
        runs["vie"]["thread"].join(60)
        assert runs["vie"]["out"]["result"]["isError"] is False, runs["vie"]["out"]
        assert card_of(bridge)["spent"] == {"EUR": 1250}
        runs["lea"]["go"].set()
        runs["lea"]["thread"].join(60)
        spent = card_of(bridge)["spent"]
        typed = [k for _, k, _ in lea.typed]
        assert spent.get("EUR", 0) <= 2000 and "number" not in typed, (
            f"cap 20 EUR: spent {spent}, the 2nd agent's card typed ({typed}); its answer: {runs['lea']['out']}")
    finally:
        for r in locals().get("runs", {}).values():
            r["go"].set()
        vie.stop()
        lea.stop()


# -- 2.5 -------------------------------------------------------------------------------------------------------------

def slow_hold(bridge, mark):
    """The fake podman takes 3 s to set a payment's hold (the window between the bridge's last « payments on » check
    and the payment entering `held`), and says when it starts."""
    path = bridge.fakebin / "podman"
    shebang, rest = path.read_text().split("\n", 1)
    hook = f'case "$*" in *"printf \'card"*) touch "{mark}"; sleep 3;; esac\n'
    path.write_text(shebang + "\n" + hook + rest)


def test_2_5_suspending_while_the_hold_is_set_types_nothing_and_clicks_nothing(bridges, fakes, tmp_path):
    chrome = FakeChrome()
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        mark = tmp_path / "hold-started"
        slow_hold(bridge, mark)
        suspended = {}

        def suspend():
            deadline = time.time() + 60
            while not mark.exists() and time.time() < deadline:
                time.sleep(0.05)
            r = bridge.post("/v1/vault/payments", headers=AUTH, json={"enabled": False})
            suspended.update(status=r.status_code, enabled=r.json().get("enabled"), at=time.time())

        t = threading.Thread(target=suspend)
        t.start()
        result, _ = pay(bridge, ARGS)
        t.join(30)
        assert suspended.get("status") == 200 and suspended.get("enabled") is False, suspended
        typed = [k for _, k, _ in chrome.typed]
        confirmed = tool(bridge, "confirm_payment", {"button": "Payer 12,50 €"})["content"][0]["text"]
        assert typed == [] and chrome.clicks == [], (
            f"« Suspendre tous les paiements » answered (enabled=false) during the hold; then typed {typed}, "
            f"pay answer: {result['content'][0]['text'][:120]}; confirm_payment clicked {chrome.clicks}: {confirmed[:120]}")
    finally:
        chrome.stop()
