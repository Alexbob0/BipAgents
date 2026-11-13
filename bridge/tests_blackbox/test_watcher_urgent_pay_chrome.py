"""The watcher's journey in a real Chromium (Alex 13/11, internal/notes/watcher-urgent-pay.md): a shop's product page
says « Rupture de stock », then « En stock »; the watcher's turn (nobody following it) opens the checkout in the
agent's browser and asks pay_with_card; the person gets the urgent push, sends the code from the card, and the card
is typed into the real page, paid, the page moving on to its confirmation. Then the same with nobody answering: past
the window nothing is typed in the page and a « Panier prêt » card waits.

Needs a Chromium on this machine (`BIP_CHROME`…): skipped without one. Only Visa's published test number is used."""
import functools
import http.server
import re
import threading
import time

import httpx

from bbkit.common import AUTH
from bbkit.servers import wait_until
from test_card_payments import CARD, CODE, NUMBER, tool
from test_card_payments_chrome import CHROME, FIXTURES, Browser, pytestmark  # noqa: F401 (skipped without Chromium)
from test_watcher_urgent_pay import ARGS, HermesSide, answered, approvals, setup, unattended_ask, urgent_pushes


class Shop:
    """The checkout fixtures, and `/produit`: out of stock until `restock`."""

    def __init__(self):
        self.in_stock = False
        shop = self

        class Handler(http.server.SimpleHTTPRequestHandler):
            def do_GET(self):
                if self.path.startswith("/produit"):
                    word = "En stock" if shop.in_stock else "Rupture de stock"
                    body = f"<!doctype html><html lang=fr><body><h1>Billet Stromae</h1><p id=stock>{word}</p>" \
                           f"<p>45,00 €</p><a href=selects.html>Commander</a></body></html>".encode()
                    self.send_response(200)
                    self.send_header("content-type", "text/html; charset=utf-8")
                    self.send_header("content-length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                    return
                super().do_GET()

            def log_message(self, *args):
                pass

        handler = functools.partial(Handler, directory=str(FIXTURES))
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.base = f"http://127.0.0.1:{self.server.server_port}"

    def watch(self):
        """The watcher's light check (its program prints this state)."""
        return "en stock" if "En stock" in httpx.get(self.base + "/produit").text else "rupture"


def journey(bridges, fakes, window=None):
    shop = Shop()
    browser = Browser(shop.base + "/produit")
    bridge, _ = setup(bridges, fakes, window=window, chrome=browser)
    assert bridge.post("/v1/vault", headers=AUTH, json={**CARD, "label": "Billets", "sites": ["127.0.0.1"]}).status_code == 201
    # the watcher: nothing yet, then the shop restocks
    assert shop.watch() == "rupture"
    shop.in_stock = True
    assert shop.watch() == "en stock"
    # its turn: the checkout opened in the agent's browser, then pay_with_card
    browser.open(shop.base + "/selects.html")
    side = HermesSide(bridge, {**ARGS, "card": "billets"})
    assert side.got_question.wait(30), "no question"
    gate = unattended_ask(fakes, bridge, side.asked, "r1")
    assert wait_until(lambda: answered(fakes, "r1") == ["once"], timeout=20), fakes.hermes.approvals
    side.answer("accept")
    return shop, browser, bridge, side, gate


def test_in_stock_urgent_push_code_paid_in_the_real_page(bridges, fakes):
    shop, browser, bridge, side, gate = journey(bridges, fakes)
    try:
        payment = re.search(r"Card payment (pay_[0-9a-f]{12}):", side.asked).group(1)
        started = time.monotonic()
        assert wait_until(lambda: urgent_pushes(fakes), timeout=10)
        [push] = urgent_pushes(fakes)
        assert push["aps"]["interruption-level"] == "time-sensitive" and push["run_id"] == f"bip-pay-{payment}"
        # « Payer » → the card: the code (typed or remembered on the phone), « Oui, payer »
        assert [c for c in approvals(bridge) if c["run_id"] == f"bip-pay-{payment}"]
        assert bridge.post(f"/v1/vault/payments/{payment}/approve", headers=AUTH, json={"code": CODE}).status_code == 204
        yes = bridge.post("/v1/approve", headers=AUTH, json={"agent": "vie", "run_id": f"bip-pay-{payment}",
                                                             "request_id": payment, "choice": "once"})
        assert yes.status_code == 200, yes.text
        assert side.done.wait(60) and side.result["isError"] is False, side.result
        values = browser.values()["selects.html"]
        assert values["numero"] == NUMBER and values["crypto"] == CODE and values["mois"] == "12"
        confirmed = tool(bridge, "confirm_payment", {"button": "Valider le paiement"})
        assert "moved on" in confirmed["content"][0]["text"], confirmed
        assert list(browser.values()) == ["done.html"]
        print(f"\nfrom the push to the page paid: {time.monotonic() - started:.1f} s")
        gate.set()
    finally:
        browser.stop()
        shop.server.shutdown()


def test_in_stock_nobody_answers_the_cart_waits(bridges, fakes):
    shop, browser, bridge, side, gate = journey(bridges, fakes, window=3)
    try:
        assert side.done.wait(30) and side.result["isError"] is True
        assert "Panier prêt" in side.result["content"][0]["text"]
        values = browser.values()["selects.html"]
        assert values["numero"] == "" and values["crypto"] == ""   # nothing typed, the page as the agent left it
        assert wait_until(lambda: [c for c in approvals(bridge) if c["run_id"].startswith("bip-cart-")], timeout=10)
        [cart] = [c for c in approvals(bridge) if c["run_id"].startswith("bip-cart-")]
        assert f"<{shop.base}/selects.html>" in cart["command"]
        gate.set()
    finally:
        browser.stop()
        shop.server.shutdown()
