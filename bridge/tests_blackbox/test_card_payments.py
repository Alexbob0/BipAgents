"""Payment cards in the agents' vault and the agents' card payments (`/v1/vault`, `/v1/vault/payments`,
`/v1/agent-tools/pay`): the card kept encrypted by the bridge only, never shown back, no security code kept; payments
off until the person turns them on, never on a place of the offer; each payment approved through an MCP elicitation
and the app's approval with the code; the card typed by the bridge into a fake Chrome (its DevTools protocol, a page
with a payment provider's out-of-process frame) on the page's real site only, the browser held meanwhile.

Only the payment networks' published test numbers are used."""
import asyncio
import json
import re
import threading
import time

import httpx
import websockets
from cryptography.fernet import Fernet
from websockets.asyncio.server import serve

from bbkit.common import AUTH
from bbkit.mcp import give_token
from bbkit.servers import free_port

NUMBER = "4242424242424242"   # Visa's published test number
CODE = "737"
CARD = {"kind": "card", "label": "Petits achats", "number": "4242 4242 4242 4242", "expiry": "12/30",
        "name": "Alex Martin", "postal_code": "75011", "sites": ["shop.example"], "agents": ["vie"]}
TOKEN = "agent-token-for-vie-0123456789abcdef"
PAY = "/v1/agent-tools/pay"


class FakeChrome:
    """Chrome's DevTools as the bridge uses them: one page on `url`, its main frame with the cardholder's name and a
    pay button, a payment provider's out-of-process frame (its own session) with the number, expiry and code. It
    answers the card script's calls by name and records what was typed where."""

    def __init__(self, url="https://shop.example/checkout"):
        self.url = url
        self.port = free_port()
        self.typed = []        # (frame, kind, text)
        self.headers = []      # x-bip-card of each connection
        self.clicks = []
        self.after_click = "https://shop.example/done"
        self.bank_asks = False
        self.fields = {"F-main": [{"i": 0, "kind": "name", "tag": "input", "fmt": "", "empty": True}],
                       "F-psp": [{"i": 0, "kind": "number", "tag": "input", "fmt": "", "empty": True},
                                 {"i": 1, "kind": "exp", "tag": "input", "fmt": "MM/YY", "empty": True},
                                 {"i": 2, "kind": "csc", "tag": "input", "fmt": "", "empty": True}]}
        self.values = {}       # (frame, i) -> text
        self.focused = {}      # session -> (frame, i)
        self.contexts = {}     # context id -> frame
        self.loop = asyncio.new_event_loop()
        self.ready = threading.Event()
        threading.Thread(target=self._run, daemon=True).start()
        self.ready.wait(5)

    def _run(self):
        asyncio.set_event_loop(self.loop)

        async def main():
            self.done = asyncio.get_running_loop().create_future()
            async with serve(self._session, "127.0.0.1", self.port, process_request=self._http):
                self.ready.set()
                await self.done

        self.loop.run_until_complete(main())

    def _http(self, connection, request):
        self.headers.append(request.headers.get("x-bip-card"))
        if request.path.startswith("/json/version"):
            body = json.dumps({"webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/browser/abc"})
            return connection.respond(200, body)
        return None

    SESSIONS = {"S-page": "F-main", "S-psp": "F-psp"}

    async def _session(self, ws):
        async for raw in ws:
            message = json.loads(raw)
            method, params, session = message["method"], message.get("params", {}), message.get("sessionId")
            for event in self._events(method, session):
                await ws.send(json.dumps(event))
            try:
                result = self._answer(method, params, session)
                await ws.send(json.dumps({"id": message["id"], "result": result, **({"sessionId": session} if session else {})}))
            except LookupError as exc:
                await ws.send(json.dumps({"id": message["id"], "error": {"code": -32000, "message": str(exc)}}))

    def _events(self, method, session):
        if method == "Target.setAutoAttach" and session == "S-page":
            return [{"method": "Target.attachedToTarget", "sessionId": "S-page",
                     "params": {"sessionId": "S-psp", "targetInfo": {"type": "iframe", "url": "https://js.psp.example/f"}}}]
        return []

    def _answer(self, method, params, session):
        if method == "Target.getTargets":
            return {"targetInfos": [{"targetId": "T1", "type": "page", "url": self.url},
                                    {"targetId": "T0", "type": "service_worker", "url": "https://x.example/sw.js"}]}
        if method == "Target.attachToTarget":
            return {"sessionId": "S-page"}
        if method == "Page.getFrameTree":
            if session == "S-psp":
                return {"frameTree": {"frame": {"id": "F-psp", "url": "https://js.psp.example/f"}}}
            return {"frameTree": {"frame": {"id": "F-main", "url": self.url},
                                  "childFrames": [{"frame": {"id": "F-psp", "url": "https://js.psp.example/f"}}]}}
        if method == "Page.createIsolatedWorld":
            if self.SESSIONS.get(session) != params["frameId"]:
                raise LookupError("No frame for given id found")
            context = len(self.contexts) + 1
            self.contexts[context] = params["frameId"]
            return {"executionContextId": context}
        if method == "Runtime.evaluate":
            return self._evaluate(params, session)
        if method == "DOM.focus":
            _, context, i = params["objectId"].split("-")
            self.focused[session] = (self.contexts[int(context)], int(i))
            return {}
        if method == "Input.insertText":
            frame, i = self.focused[session]
            self.values[(frame, i)] = params["text"]
            kind = self.fields[frame][i]["kind"]
            self.typed.append((frame, kind, params["text"]))
            return {}
        return {}

    def _evaluate(self, params, session):
        expression, context = params["expression"], params.get("contextId")
        frame = self.contexts.get(context)
        value = None
        call = re.search(r"__bipPay\.(\w+)\(([^)]*)\)\s*$", expression)
        if expression == "document.visibilityState":
            value = "visible"
        elif call:
            name, args = call.group(1), call.group(2)
            first = int(args.split(",")[0]) if args and args.split(",")[0].strip().isdigit() else None
            if name == "detect":
                value = self.fields[frame]
            elif name == "el":
                return {"result": {"type": "object", "objectId": f"obj-{context}-{first}"}}
            elif name == "holds":
                text = self.values.get((frame, first), "")
                value = re.sub(r"\D", "", text) if "true" in args else text.strip().lower()
            elif name == "set":
                self.values[(frame, first)] = ""
                value = True
            elif name == "findButton":
                value = "payer 12,50 €"
            elif name == "clickButton":
                self.clicks.append(self.url)
                self.url = self.after_click
                value = True
            elif name == "bankAsks":
                value = self.bank_asks
            else:
                value = True
        return {"result": {"type": "object" if isinstance(value, list) else "string", "value": value}}

    def stop(self):
        self.loop.call_soon_threadsafe(lambda: self.done.done() or self.done.set_result(None))


def pay_bridge(bridges, fakes, chrome=None, change=None, place=None):
    bridge = bridges.new(place or "bridge")
    if place:   # a place of the all-included offer: its guichet token
        settings = bridge.home / ".config" / "bipagents" / "backup.json"
        settings.parent.mkdir(parents=True, exist_ok=True)
        settings.write_text(json.dumps({"url": "https://guichet.test/v1/backup", "token": "t1"}))
    bridge.config["agents"]["Vie"] = {"hermes_url": fakes.down.url, "hermes_key": "k" * 32, "isolated": True,
                                      "python": f"{bridge.fakebin}/podman exec -i hermes-vie python",
                                      "hermes_home": "/home/hermes/.hermes"}
    bridge.config["hermes"] = {"isolated": True, "bin": str(bridge.fakebin / "hermes"), "hub_url": fakes.hermes.url}
    if change:
        change(bridge.config)
    if chrome:
        folder = bridge.data / "browsers" / "vie"
        folder.mkdir(parents=True)
        (folder / "ports.json").write_text(json.dumps({"cdp": chrome.port, "vnc": chrome.port + 1}))
    bridge.start()
    give_token(bridge, "vie", TOKEN)
    return bridge


def turn_on(bridge):
    on = bridge.post("/v1/vault/payments", headers=AUTH, json={"enabled": True})
    assert on.status_code == 200, on.text
    return on.json()


def tool(bridge, name, arguments=None):
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments or {}}}
    resp = bridge.post(PAY, json=body, headers={"Authorization": f"Bearer {TOKEN}"}, timeout=60)
    return resp.json()["result"]


def pay(bridge, arguments, hermes="accept", app=CODE):
    """pay_with_card as Hermes calls it (a stream): the elicitation answered by Hermes (`hermes`), the app sending the
    code first (`app`: the code, "deny", or None). Returns (result, the question's message or None)."""
    body = {"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "pay_with_card", "arguments": arguments}}
    headers = {"Authorization": f"Bearer {TOKEN}", "Accept": "application/json, text/event-stream"}
    asked = None
    with httpx.Client(timeout=90) as client, client.stream("POST", bridge.url + PAY, json=body, headers=headers) as resp:
        for line in resp.iter_lines():
            if not line.startswith("data: "):
                continue
            message = json.loads(line[6:])
            if message.get("method") == "elicitation/create":
                asked = message["params"]["message"]
                payment = re.search(r"Card payment (pay_[0-9a-f]{12}):", asked).group(1)
                if app == "deny":
                    assert bridge.post(f"/v1/vault/payments/{payment}/deny", headers=AUTH).status_code == 204
                elif app:
                    sent = bridge.post(f"/v1/vault/payments/{payment}/approve", headers=AUTH, json={"code": app})
                    assert sent.status_code == 204, sent.text
                answer = {"jsonrpc": "2.0", "id": message["id"], "result": {"action": hermes, "content": {}}}
                assert client.post(bridge.url + PAY, json=answer, headers=headers).status_code == 202
            elif message.get("id") == 7:
                return message["result"], asked
    raise AssertionError("no result")


ARGS = {"card": "petits achats", "amount": "12,50", "currency": "eur", "merchant": "Billet Paris-Lyon"}


def test_cards_wait_for_the_persons_switch_and_never_keep_the_code(bridges, fakes):
    bridge = pay_bridge(bridges, fakes)
    listing = bridge.get("/v1/vault", headers=AUTH).json()
    assert listing["cards"] is False and listing["cards_allowed"] is True
    off = bridge.post("/v1/vault", headers=AUTH, json=CARD)
    assert off.status_code == 409 and off.json() == {"detail": {"code": "cards_off"}}
    assert turn_on(bridge)["enabled"] is True
    with_code = bridge.post("/v1/vault", headers=AUTH, json={**CARD, "csc": "123"})
    assert with_code.status_code == 400 and "never kept" in with_code.json()["detail"]
    bad = bridge.post("/v1/vault", headers=AUTH, json={**CARD, "number": "4242424242424241"})
    assert bad.status_code == 400 and bad.json() == {"detail": "card number is not valid"}
    expired = bridge.post("/v1/vault", headers=AUTH, json={**CARD, "expiry": "01/20"})
    assert expired.status_code == 400 and expired.json() == {"detail": "this card has expired"}
    hermes_payment = bridge.post("/v1/vault", headers=AUTH, json={"kind": "payment", "label": "V", "agents": []})
    assert hermes_payment.status_code == 400
    made = bridge.post("/v1/vault", headers=AUTH, json=CARD)
    assert made.status_code == 201, made.text
    item = made.json()
    assert item["card"] == {"last4": "4242", "brand": "visa", "exp_month": 12, "exp_year": 2030, "single_use": False,
                            "used": False, "sites": ["shop.example"], "spent": {}, "payments": []}
    assert item["agents"] == ["vie"] and NUMBER not in made.text
    assert bridge.commands.of("podman") == []                     # never written into the agent's own vault
    listed = bridge.get("/v1/vault", headers=AUTH)
    assert NUMBER not in listed.text and listed.json()["items"] == [item]
    folder = bridge.state_dir / "vault"
    assert NUMBER.encode() not in (folder / "vault.json.enc").read_bytes()
    records = json.loads(Fernet((folder / "vault.key").read_bytes().strip()).decrypt((folder / "vault.json.enc").read_bytes()))
    assert records[0]["secret"] == {"number": NUMBER, "exp_month": 12, "exp_year": 2030, "name": "Alex Martin",
                                    "postal_code": "75011"}
    assert records[0]["agents"] == {"vie": "bridge"}
    # Suspended: no new card, nothing paid; the cards kept stay editable.
    assert bridge.post("/v1/vault/payments", headers=AUTH, json={"enabled": False}).json()["enabled"] is False
    assert bridge.post("/v1/vault", headers=AUTH, json=CARD).status_code == 409
    assert bridge.patch(f"/v1/vault/{item['id']}", headers=AUTH, json={"label": "Courses"}).status_code == 200
    assert "Payments by the agents are off" in tool(bridge, "list_cards")["content"][0]["text"]
    journal = bridge.get("/v1/vault/payments", headers=AUTH).json()["journal"]
    assert [e["outcome"] for e in journal] == ["payments_suspended", "payments_on"]


def test_never_on_a_place_of_the_offer_nor_when_the_install_forbids_it(bridges, fakes):
    bridge = pay_bridge(bridges, fakes, change=lambda c: c.update({"vault": {"cards": "off"}}))
    assert bridge.get("/v1/vault/payments", headers=AUTH).json()["allowed"] is False
    refused = bridge.post("/v1/vault/payments", headers=AUTH, json={"enabled": True})
    assert refused.status_code == 409 and refused.json() == {"detail": {"code": "cards_not_allowed"}}
    place = bridges.new("place")
    settings = place.root / "home" / ".config" / "bipagents" / "backup.json"
    settings.parent.mkdir(parents=True, exist_ok=True)
    settings.write_text(json.dumps({"url": "https://guichet.test/v1/backup", "token": "t1"}))
    place.config["hermes"] = {"isolated": True}
    place.start()
    assert place.post("/v1/vault/payments", headers=AUTH, json={"enabled": True}).status_code == 409


def test_a_payment_approved_is_typed_on_the_real_site_then_the_bridge_clicks(bridges, fakes):
    chrome = FakeChrome()
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        item = bridge.post("/v1/vault", headers=AUTH, json=CARD).json()
        listed = tool(bridge, "list_cards")["content"][0]["text"]
        assert "« Petits achats » (ending 4242): shop.example; ready" in listed and NUMBER not in listed
        result, asked = pay(bridge, ARGS)
        assert result["isError"] is False, result
        assert re.fullmatch(r"Card payment pay_[0-9a-f]{12}: 12\.50 EUR on shop\.example with card \"Petits achats\" "
                            r"ending 4242, for \"Billet Paris-Lyon\"\. Approve in the BipAgents app with the card's "
                            r"security code\.", asked)
        # Typed where each field lives: the provider's frame (its own session) and the page.
        assert chrome.typed == [("F-main", "name", "Alex Martin"), ("F-psp", "number", NUMBER),
                                ("F-psp", "exp", "12/30"), ("F-psp", "csc", CODE)]
        text = result["content"][0]["text"]
        assert "paused" in text and "confirm_payment" in text and NUMBER not in text and CODE not in text
        # The browser is held: the flag reads « card <token> », and the bridge shows the token to DevTools.
        hold = [c for c in bridge.commands.of("podman") if "printf" in c[-1]]
        assert len(hold) == 1 and hold[0][:4] == ["exec", "bipagents-browser-vie", "sh", "-c"]
        token = re.search(r"printf 'card %s' ([0-9a-f]+) >", hold[0][-1]).group(1)
        assert token in chrome.headers and chrome.headers[0] is None    # the first look, before the approval
        active = bridge.get("/v1/vault/payments", headers=AUTH).json()["active"]
        assert [(a["agent"], a["card"], a["site"], a["amount"]) for a in active] == [("vie", "Petits achats", "shop.example", "12.50")]
        # A second payment waits for this one.
        assert "already in progress" in tool(bridge, "pay_with_card", ARGS)["content"][0]["text"]
        confirmed = tool(bridge, "confirm_payment", {"button": "Payer 12,50 €"})
        assert confirmed["isError"] is False and "moved on" in confirmed["content"][0]["text"]
        assert chrome.clicks == ["https://shop.example/checkout"]
        assert bridge.commands.of("podman")[-1][-1].endswith("rm -f /tmp/bipagents-hold; exit 0")
        card = bridge.get("/v1/vault", headers=AUTH).json()["items"][0]["card"]
        assert card["spent"] == {"EUR": 1250}
        [payment] = card["payments"]
        assert {k: payment[k] for k in ("site", "amount", "currency", "agent", "for")} == {
            "site": "shop.example", "amount": "12.50", "currency": "EUR", "agent": "vie", "for": "Billet Paris-Lyon"}
        journal = bridge.get("/v1/vault/payments", headers=AUTH).json()["journal"]
        assert [e["outcome"] for e in journal][:5] == ["page_moved_on", "clicked", "typed", "approved", "asked"]
        assert all(e.get("last4") == "4242" for e in journal[:5])
        for where in (json.dumps(journal), (bridge.root / "bridge.log").read_text()):
            assert NUMBER not in where and "4242 4242" not in where
        assert item["id"]
    finally:
        chrome.stop()


def test_only_on_the_cards_sites_and_only_with_both_approvals(bridges, fakes):
    chrome = FakeChrome("https://evil.example/pay")
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        refused = tool(bridge, "pay_with_card", ARGS)
        assert refused["isError"] is True and "only for shop.example; this page is on evil.example" in refused["content"][0]["text"]
        chrome.url = "https://shop.example/checkout"
        denied, asked = pay(bridge, ARGS, hermes="decline", app=None)
        assert asked and denied["isError"] is True and "did not approve" in denied["content"][0]["text"]
        app_no, _ = pay(bridge, ARGS, hermes="accept", app="deny")
        assert app_no["isError"] is True
        assert chrome.typed == [] and not [c for c in bridge.commands.of("podman") if "printf" in c[-1]]
        # Not one of its cards: another agent gets nothing.
        other = tool(bridge, "pay_with_card", {**ARGS, "card": "Autre"})
        assert "No card « Autre »" in other["content"][0]["text"]
        outcomes = [e["outcome"] for e in bridge.get("/v1/vault/payments", headers=AUTH).json()["journal"]]
        assert outcomes[:5] == ["denied", "asked", "denied", "asked", "refused_site"]
    finally:
        chrome.stop()


def test_a_single_use_card_waits_for_its_new_numbers(bridges, fakes):
    chrome = FakeChrome()
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        item = bridge.post("/v1/vault", headers=AUTH, json={**CARD, "single_use": True}).json()
        result, _ = pay(bridge, ARGS)
        assert result["isError"] is False and "single-use" in result["content"][0]["text"]
        assert tool(bridge, "cancel_payment")["content"][0]["text"].startswith("The payment is given up")
        assert bridge.get("/v1/vault", headers=AUTH).json()["items"][0]["card"]["used"] is True
        again = tool(bridge, "pay_with_card", ARGS)
        assert again["isError"] is True and "already used" in again["content"][0]["text"]
        renewed = bridge.patch(f"/v1/vault/{item['id']}", headers=AUTH, json={"number": "4000056655665556", "expiry": "11/31"})
        assert renewed.json()["card"]["used"] is False and renewed.json()["card"]["last4"] == "5556"
    finally:
        chrome.stop()


def test_suspending_gives_the_browser_back_with_the_card_erased(bridges, fakes):
    chrome = FakeChrome()
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        result, _ = pay(bridge, ARGS)
        assert result["isError"] is False
        assert chrome.values[("F-psp", 0)] == NUMBER
        suspended = bridge.post("/v1/vault/payments", headers=AUTH, json={"enabled": False}).json()
        assert suspended["active"] == [] and suspended["enabled"] is False
        assert chrome.values[("F-psp", 0)] == "" and chrome.values[("F-psp", 2)] == ""
        assert bridge.commands.of("podman")[-1][-1].endswith("rm -f /tmp/bipagents-hold; exit 0")
        assert "No payment is waiting" in tool(bridge, "confirm_payment", {"button": "Payer"})["content"][0]["text"]
    finally:
        chrome.stop()


PHONE_A, PHONE_B = "phone-a-0123456789", "phone-b-0123456789"
PHONE_CARD = {"kind": "card", "label": "Petits achats", "last4": "4242", "brand": "visa", "device_id": PHONE_A,
              "device_name": "iPhone d'Alex", "cap_eur": 20, "virtual_card": True, "sites": ["shop.example"],
              "agents": ["vie"]}
FROM_PHONE_A = {"code": CODE, "device_id": PHONE_A, "number": "4242 4242 4242 4242", "exp_month": 12, "exp_year": 2030,
                "name": "Alex Martin"}


def pay_from_phone(bridge, arguments, approvals):
    """pay_with_card on a place of the offer: each body of `approvals` sent as the app's approval (its status kept),
    then Hermes says yes. Returns (result, the question's message, the approvals' statuses and answers)."""
    body = {"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "pay_with_card", "arguments": arguments}}
    headers = {"Authorization": f"Bearer {TOKEN}", "Accept": "application/json, text/event-stream"}
    asked, answers = None, []
    with httpx.Client(timeout=90) as client, client.stream("POST", bridge.url + PAY, json=body, headers=headers) as resp:
        for line in resp.iter_lines():
            if not line.startswith("data: "):
                continue
            message = json.loads(line[6:])
            if message.get("method") == "elicitation/create":
                asked = message["params"]["message"]
                payment = re.search(r"Card payment (pay_[0-9a-f]{12}):", asked).group(1)
                for approval in approvals:
                    sent = bridge.post(f"/v1/vault/payments/{payment}/approve", headers=AUTH, json=approval)
                    answers.append((sent.status_code, sent.text))
                answer = {"jsonrpc": "2.0", "id": message["id"], "result": {"action": "accept", "content": {}}}
                assert client.post(bridge.url + PAY, json=answer, headers=headers).status_code == 202
            elif message.get("id") == 7:
                return message["result"], asked, answers
    raise AssertionError("no result")


def vault_records(bridge):
    folder = bridge.state_dir / "vault"
    key = (folder / "vault.key").read_bytes().strip()
    return json.loads(Fernet(key).decrypt((folder / "vault.json.enc").read_bytes()))


def test_on_a_place_of_the_offer_only_when_switched_on_and_the_number_stays_on_the_phone(bridges, fakes):
    off = pay_bridge(bridges, fakes, place="place-off")
    assert off.get("/v1/vault", headers=AUTH).json()["cards_allowed"] is False
    assert off.post("/v1/vault/payments", headers=AUTH, json={"enabled": True}).status_code == 409
    off.stop()
    chrome = FakeChrome()
    try:
        bridge = pay_bridge(bridges, fakes, chrome, place="place",
                            change=lambda c: c.update({"payments": {"offer_cards": True}}))
        listing = bridge.get("/v1/vault", headers=AUTH).json()
        assert listing["cards_allowed"] is True and listing["cards_on_phone"] is True
        assert listing["card_cap_max_eur"] == 150
        overview = turn_on(bridge)
        assert overview["numbers_on_phone"] is True and overview["cap_max_eur"] == 150
        # Never a number to keep here, and a capped virtual card only.
        with_number = bridge.post("/v1/vault", headers=AUTH, json={**PHONE_CARD, "number": NUMBER, "expiry": "12/30"})
        assert with_number.status_code == 400 and "stays on your phone" in with_number.json()["detail"]
        no_cap = bridge.post("/v1/vault", headers=AUTH, json={k: v for k, v in PHONE_CARD.items() if k != "cap_eur"})
        assert no_cap.status_code == 400 and no_cap.json() == {"detail": "a cap in euros is required"}
        assert bridge.post("/v1/vault", headers=AUTH, json={**PHONE_CARD, "cap_eur": 500}).status_code == 400
        assert bridge.post("/v1/vault", headers=AUTH, json={**PHONE_CARD, "virtual_card": False}).status_code == 400
        made = bridge.post("/v1/vault", headers=AUTH, json=PHONE_CARD)
        assert made.status_code == 201, made.text
        card = made.json()["card"]
        assert card["on_phone"] is True and card["device_id"] == PHONE_A and card["cap_cents"] == 2000
        assert card["exp_month"] is None and card["last4"] == "4242"
        assert vault_records(bridge)[0]["secret"] == {}
        # Approved from another phone: refused with the name of the one that keeps the card, nothing typed.
        refused, asked, answers = pay_from_phone(bridge, ARGS, [{**FROM_PHONE_A, "device_id": PHONE_B}])
        assert f"Card {made.json()['id']}." in asked
        assert answers[0][0] == 409 and json.loads(answers[0][1]) == {
            "detail": {"code": "card_on_other_device", "device_name": "iPhone d'Alex"}}
        assert refused["isError"] is True and chrome.typed == []
        # From its phone, with its numbers: typed once, then gone.
        result, _, answers = pay_from_phone(bridge, ARGS, [FROM_PHONE_A])
        assert answers == [(204, "")] and result["isError"] is False, result
        assert ("F-psp", "number", NUMBER) in chrome.typed and ("F-psp", "exp", "12/30") in chrome.typed
        assert tool(bridge, "confirm_payment", {"button": "Payer 12,50 €"})["isError"] is False
        assert bridge.get("/v1/vault", headers=AUTH).json()["items"][0]["card"]["spent"] == {"EUR": 1250}
        # The cap (20 €, 12.50 already paid): refused before asking.
        chrome.url = "https://shop.example/checkout"
        over = tool(bridge, "pay_with_card", ARGS)
        assert over["isError"] is True and "cap of 20.00 EUR" in over["content"][0]["text"]
        journal = bridge.get("/v1/vault/payments", headers=AUTH).json()["journal"]
        assert journal[0]["outcome"] == "refused_cap"
        records = vault_records(bridge)
        assert records[0]["secret"] == {}
        for where in (json.dumps(journal), bridge.log_path.read_text(), json.dumps(records)):
            assert NUMBER not in where and "4242 4242" not in where
        # The phone found its card gone: flagged, and no agent pays with it.
        gone = bridge.patch(f"/v1/vault/{made.json()['id']}", headers=AUTH, json={"missing": True})
        assert gone.status_code == 200 and gone.json()["card"]["missing"] is True
        assert "no longer on the person's phone" in tool(bridge, "list_cards")["content"][0]["text"]
    finally:
        chrome.stop()
