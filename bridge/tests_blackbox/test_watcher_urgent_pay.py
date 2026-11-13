"""A watcher's payment, quickly (Alex 13/11, internal/notes/watcher-urgent-pay.md): a card payment asked in a turn
nobody follows (a routine's or a watcher's run) is never a held card that ends the run and loses the cart. The bridge
answers Hermes' question itself, the payment waits as the bridge's own card (`bip-pay-<id>`), pushed urgent
(category PAYMENT, time-sensitive), and the person's code with « Oui, payer » pays it: the run, still inside
pay_with_card, goes on to type the card as always. Past the window: nothing typed, a « Panier prêt » card
(`bip-cart-`) pushed with the « trop tard » line. « Faire un essai » sends the same urgent push, marked test.

Only the payment networks' published test numbers are used (test_card_payments.py's fake shop)."""
import json
import re
import threading

import httpx

from bbkit.common import AUTH, HERMES_KEY, sse
from bbkit.servers import wait_until
from test_card_payments import CARD, CODE, NUMBER, PAY, TOKEN, FakeChrome, pay_bridge, turn_on

DEVICE = "d" * 64
NOW = "2026-11-13T20:00:00.000Z"
ARGS = {"card": "petits achats", "amount": "45,00", "currency": "eur", "merchant": "Billet Stromae"}


def with_device(bridge):
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (DEVICE, "sandbox", "[]", NOW, NOW))])


class HermesSide:
    """pay_with_card as the agent's Hermes calls it (a stream), in a thread: its question kept (`asked`), answered when
    the test says (`answer`), its result kept (`result`)."""

    def __init__(self, bridge, arguments):
        self.bridge, self.arguments = bridge, arguments
        self.asked, self.question, self.result = None, None, None
        self.got_question, self.done, self.reply = threading.Event(), threading.Event(), threading.Event()
        self.action = "accept"
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def run(self):
        body = {"jsonrpc": "2.0", "id": 7, "method": "tools/call",
                "params": {"name": "pay_with_card", "arguments": self.arguments}}
        headers = {"Authorization": f"Bearer {TOKEN}", "Accept": "application/json, text/event-stream"}
        with httpx.Client(timeout=120) as client, \
                client.stream("POST", self.bridge.url + PAY, json=body, headers=headers) as resp:
            for line in resp.iter_lines():
                if not line.startswith("data: "):
                    continue
                message = json.loads(line[6:])
                if message.get("method") == "elicitation/create":
                    self.asked, self.question = message["params"]["message"], message["id"]
                    self.got_question.set()
                    self.reply.wait(60)
                    answer = {"jsonrpc": "2.0", "id": self.question, "result": {"action": self.action, "content": {}}}
                    client.post(self.bridge.url + PAY, json=answer, headers=headers)
                elif message.get("id") == 7:
                    self.result = message["result"]
                    break
        self.done.set()

    def answer(self, action="accept"):
        self.action = action
        self.reply.set()


def routine(bridge, name):
    made = bridge.post("/v1/agents/vie/routines", headers=AUTH,
                       json={"name": name, "schedule": "every day at 09:00", "prompt": "Regarde si les billets sont en vente."})
    assert made.status_code in (200, 201), made.text
    return made.json()["job"]


def approvals(bridge):
    return bridge.get("/v1/approvals", headers=AUTH, params={"agent": "vie"}).json()["items"]


def answered(fakes, run):
    return [a["body"]["choice"] for a in fakes.hermes.approvals if a["run_id"] == run]


def unattended_ask(fakes, bridge, asked, run):
    """The routine's run, nobody following it, where Hermes puts the payment's question as an approval."""
    job = routine(bridge, "Billets Stromae")
    gate = threading.Event()
    fakes.hermes.runs[run] = [
        sse("approval.request", run_id=run, request_id="q1", choices=["once", "deny"], command=asked,
            description="Fields requested by MCP server 'bipagents_pay'", pattern_key="mcp_elicitation"),
        gate, sse("run.completed", run_id=run, output="Billets payés."),
    ]
    assert bridge.post(f"/v1/routines/{job['id']}/run", headers=AUTH).status_code == 200
    return gate


def urgent_pushes(fakes):
    return [p for p in fakes.apns.payloads() if p.get("urgent") == 1]


def setup(bridges, fakes, window=None):
    chrome = FakeChrome()

    def prepare(bridge):
        with_device(bridge)
        if window:
            bridge.env_extra["BIP_TEST_URGENT_WINDOW_SECONDS"] = str(window)

    def hermes(config):   # the agent's own Hermes runs its routines
        config["agents"]["Vie"].update(hermes_url=fakes.hermes.url, hermes_key=HERMES_KEY)

    bridge = pay_bridge(bridges, fakes, chrome, change=hermes, prepare=prepare)
    turn_on(bridge)
    assert bridge.post("/v1/vault", headers=AUTH, json=CARD).status_code == 201
    return bridge, chrome


def test_a_payment_asked_while_nobody_is_there_waits_urgent_and_is_paid_with_the_code(bridges, fakes):
    bridge, chrome = setup(bridges, fakes)
    try:
        side = HermesSide(bridge, ARGS)
        assert side.got_question.wait(30)
        payment = re.search(r"Card payment (pay_[0-9a-f]{12}):", side.asked).group(1)
        assert re.search(r" Card ref item_[0-9a-f]{12}\. Approve in the BipAgents app", side.asked)
        gate = unattended_ask(fakes, bridge, side.asked, "r1")
        # the bridge answers Hermes itself (nobody could): no held card, no « deny »
        assert wait_until(lambda: answered(fakes, "r1") == ["once"], timeout=20), fakes.hermes.approvals
        side.answer("accept")
        assert not [c for c in approvals(bridge) if c["run_id"].startswith("bip-held-")]
        assert wait_until(lambda: [c for c in approvals(bridge) if c["run_id"] == f"bip-pay-{payment}"], timeout=10)
        [card] = [c for c in approvals(bridge) if c["run_id"] == f"bip-pay-{payment}"]
        assert card["command"] == side.asked and card["choices"] == ["once", "deny"]
        assert card["bip_urgent"]["window"] == 600 and "bip_always" not in card
        # pushed once, urgent: « Payer » (PAYMENT), time-sensitive, the card in run_id; no card number anywhere
        assert wait_until(lambda: urgent_pushes(fakes), timeout=10)
        [push] = urgent_pushes(fakes)
        assert push["aps"]["category"] == "PAYMENT" and push["aps"]["interruption-level"] == "time-sensitive"
        assert push["run_id"] == f"bip-pay-{payment}" and push["pay"] == payment and push["t"] == "approval"
        assert NUMBER not in json.dumps(fakes.apns.payloads())
        # the run waits (past Hermes' own 20 s for a code): the person's « Oui » without the code is refused
        no_code = bridge.post("/v1/approve", headers=AUTH, json={"agent": "vie", "run_id": card["run_id"],
                                                                 "request_id": payment, "choice": "once"})
        assert no_code.status_code == 409, no_code.text
        assert not side.done.is_set()
        # the app: the code first (typed, or remembered on the phone: the same call), then the card's answer
        sent = bridge.post(f"/v1/vault/payments/{payment}/approve", headers=AUTH, json={"code": CODE})
        assert sent.status_code == 204, sent.text
        yes = bridge.post("/v1/approve", headers=AUTH, json={"agent": "vie", "run_id": card["run_id"],
                                                             "request_id": payment, "choice": "once"})
        assert yes.status_code == 200 and yes.json()["choice"] == "once", yes.text
        assert side.done.wait(60)
        assert side.result["isError"] is False, side.result
        assert ("F-psp", "number", NUMBER) in chrome.typed and ("F-psp", "csc", CODE) in chrome.typed
        assert not [c for c in approvals(bridge) if c["run_id"].startswith("bip-pay-")]
        outcomes = [e["outcome"] for e in bridge.get("/v1/vault/payments", headers=AUTH).json()["journal"]]
        assert "typed" in outcomes and CODE not in json.dumps(outcomes)
        gate.set()
    finally:
        chrome.stop()


def test_a_payment_nobody_answers_in_time_leaves_a_cart_to_buy_by_hand(bridges, fakes):
    bridge, chrome = setup(bridges, fakes, window=3)
    try:
        side = HermesSide(bridge, ARGS)
        assert side.got_question.wait(30)
        gate = unattended_ask(fakes, bridge, side.asked, "r1")
        assert wait_until(lambda: answered(fakes, "r1") == ["once"], timeout=20)
        side.answer("accept")
        assert side.done.wait(30)
        text = side.result["content"][0]["text"]
        assert side.result["isError"] is True and "10 minutes" in text and "Panier prêt" in text
        assert chrome.typed == [] and chrome.reloads == 0
        # « Panier prêt » in its place: the hand-over's text with the page the bridge read, pushed « trop tard »
        assert wait_until(lambda: [c for c in approvals(bridge) if c["run_id"].startswith("bip-cart-")], timeout=10)
        [cart] = [c for c in approvals(bridge) if c["run_id"].startswith("bip-cart-")]
        assert cart["command"].startswith(f"Payment hand-over {cart['request_id']}: 45.00 EUR on shop.example")
        assert "<https://shop.example/checkout>" in cart["command"]
        assert not [c for c in approvals(bridge) if c["run_id"].startswith("bip-pay-")]
        assert wait_until(lambda: [p for p in fakes.apns.payloads() if p.get("r") == cart["run_id"]], timeout=10)
        # put away from the app (it opened the shop itself)
        done = bridge.post("/v1/approve", headers=AUTH, json={"agent": "vie", "run_id": cart["run_id"],
                                                              "request_id": cart["request_id"], "choice": "once"})
        assert done.status_code == 200, done.text
        assert not [c for c in approvals(bridge) if c["run_id"].startswith("bip-cart-")]
        gate.set()
    finally:
        chrome.stop()


def test_a_no_ends_the_payment_and_a_test_alert_is_urgent_too(bridges, fakes):
    bridge, chrome = setup(bridges, fakes)
    try:
        side = HermesSide(bridge, ARGS)
        assert side.got_question.wait(30)
        payment = re.search(r"Card payment (pay_[0-9a-f]{12}):", side.asked).group(1)
        gate = unattended_ask(fakes, bridge, side.asked, "r1")
        assert wait_until(lambda: answered(fakes, "r1") == ["once"], timeout=20)
        side.answer("accept")
        assert wait_until(lambda: [c for c in approvals(bridge) if c["run_id"] == f"bip-pay-{payment}"], timeout=10)
        # a second payment of the same agent meanwhile: refused at once
        second = HermesSide(bridge, ARGS)
        assert second.done.wait(30) and second.result["isError"] is True
        assert "already waits" in second.result["content"][0]["text"]
        # « Non »: the app's refusal, then the card's answer
        assert bridge.post(f"/v1/vault/payments/{payment}/deny", headers=AUTH).status_code == 204
        no = bridge.post("/v1/approve", headers=AUTH, json={"agent": "vie", "run_id": f"bip-pay-{payment}",
                                                            "request_id": payment, "choice": "deny"})
        assert no.status_code == 200, no.text
        assert side.done.wait(30)
        assert side.result["isError"] is True and "did not approve" in side.result["content"][0]["text"]
        assert chrome.typed == []
        assert not [c for c in approvals(bridge) if c["run_id"].startswith(("bip-pay-", "bip-cart-"))]
        gate.set()
        # « Faire un essai »: the same urgent push, marked test, no card
        sent = bridge.post("/v1/agents/vie/urgent-test", headers=AUTH)
        assert sent.status_code == 200 and sent.json()["sent"] == 1, sent.text
        assert wait_until(lambda: [p for p in urgent_pushes(fakes) if p.get("test") == 1], timeout=10)
        [test] = [p for p in urgent_pushes(fakes) if p.get("test") == 1]
        assert test["run_id"].startswith("bip-paytest-") and test["aps"]["category"] == "PAYMENT"
    finally:
        chrome.stop()
