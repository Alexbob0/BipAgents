"""Payment cards in the agents' vault and the agents' card payments (`/v1/vault`, `/v1/vault/payments`,
`/v1/agent-tools/pay`): the card kept encrypted by the bridge only, never shown back, no security code kept; payments
off until the person turns them on, never on a place of the offer; each payment approved through an MCP elicitation
and the app's approval with the code; the card typed by the bridge into a fake Chrome (its DevTools protocol, a page
with a payment provider's out-of-process frame) on the page's real site only, the browser held meanwhile, into the
page reloaded under the hold (other tabs closed, the site's service workers removed), never into a frame of an
unknown site, the tab left on an empty page unless the page moved on by itself. test_card_payments_chrome.py does the
same in a real Chromium, with a page an agent prepared.

Only the payment networks' published test numbers are used."""
import asyncio
import json
import re
import subprocess
import threading
import time

import httpx
import pytest
import websockets
from cryptography.fernet import Fernet
from websockets.asyncio.server import serve

from bbkit.common import AUTH
from bbkit.mcp import give_token
from bbkit.servers import free_port, wait_until

NUMBER = "4242424242424242"   # Visa's published test number
CODE = "737"
CARD = {"kind": "card", "label": "Petits achats", "number": "4242 4242 4242 4242", "expiry": "12/30",
        "name": "Alex Martin", "postal_code": "75011", "sites": ["shop.example"], "agents": ["vie"]}
TOKEN = "agent-token-for-vie-0123456789abcdef"
PAY = "/v1/agent-tools/pay"
# The bridge's look at the browser's MCP gates (browser/hold_gate.py): their tool calls still running, under the hold.
CALLS_CHECK = ('grep -q bipagents-calls /opt/stealth/hold_gate.py || exit 5; for f in /tmp/bipagents-calls/*; do '
               '[ -f "$f" ] || continue; [ -d "/proc/${f##*/}" ] || { rm -f "$f"; continue; }; [ "$(cat "$f")" = 0 ] '
               '|| exit 4; done; exit 0')


class FakeChrome:
    """Chrome's DevTools as the bridge uses them: one page on `url`, its main frame with the cardholder's name and a
    pay button, a payment provider's out-of-process frame (its own session) with the number, expiry and code, and
    (`other`) a frame of another site found before it, with a card number field of its own. It answers the card
    script's calls by name and records what was typed where."""

    PSP = "https://js.stripe.com/v3/elements-inner-card.html"

    def __init__(self, url="https://shop.example/checkout", psp=PSP, other=None):
        self.url = url
        self.psp = psp
        self.sessions = {"S-page": "F-main", **({"S-other": "F-other"} if other else {}), "S-psp": "F-psp"}
        self.frame_urls = {"F-other": other, "F-psp": psp}
        self.emptied = []      # (frame, i) emptied by their node (card_fill.rs `clear`)
        self.port = free_port()
        self.typed = []        # (frame, kind, text)
        self.headers = []      # x-bip-card of each connection
        self.clicks = []
        self.after_click = "https://shop.example/done"
        self.bank_asks = False
        self.loader = 1         # the main frame's document: a new one on each reload or navigation
        self.reloads = 0
        self.reload_to = None   # the address a reload ends on (a redirect), else the same
        self.lose_form = False  # a reload brings the page back without its card form
        self.reload_delay = 0   # seconds Page.reload takes to answer
        self.reloading = False  # a Page.reload was asked (before its answer)
        self.focus_trap = False  # something of the page keeps the keyboard's focus: DOM.focus moves nothing
        self.slow = None        # a step held up once (8 s): "reload", "detect" (the card form, after it), "type"
        self.slowed = False     # that step was reached
        self.button = "payer 12,50 €"   # the pay button found (None: none)
        self.split = False      # after a reload, the provider's expiry and code appear one look after its number
        self.looks = 0          # the provider frame's card fields looked for since the last reload
        self.others = []        # other tabs (their target ids)
        self.closed = []
        self.cleared = []       # Storage.clearDataForOrigin's parameters
        self.navigated = []
        self.resets = []        # the address shown when the tab's history was forgotten
        self.anchored = []      # frames whose owner element was marked as the card form's (confirm_payment)
        self.fields = {"F-main": [{"i": 0, "kind": "name", "tag": "input", "fmt": "", "empty": True}],
                       "F-psp": [{"i": 0, "kind": "number", "tag": "input", "fmt": "", "empty": True},
                                 {"i": 1, "kind": "exp", "tag": "input", "fmt": "MM/YY", "empty": True},
                                 {"i": 2, "kind": "csc", "tag": "input", "fmt": "", "empty": True}]}
        if other:
            self.fields["F-other"] = [{"i": 0, "kind": "number", "tag": "input", "fmt": "", "empty": True}]
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

    async def _session(self, ws):
        async for raw in ws:
            message = json.loads(raw)
            method, params, session = message["method"], message.get("params", {}), message.get("sessionId")
            for event in self._events(method, session):
                await ws.send(json.dumps(event))
            if method == "Page.reload":
                self.reloading = True
                await asyncio.sleep(self.reload_delay)
            if self._held_up(method, params):
                self.slowed = True
                await asyncio.sleep(8)   # under the bridge's 10 s for one call
            try:
                result = self._answer(method, params, session)
                await ws.send(json.dumps({"id": message["id"], "result": result, **({"sessionId": session} if session else {})}))
            except LookupError as exc:
                await ws.send(json.dumps({"id": message["id"], "error": {"code": -32000, "message": str(exc)}}))

    def _held_up(self, method, params):
        if self.slowed or not self.slow:
            return False
        if self.slow == "reload":
            return method == "Page.reload"
        if self.slow == "detect":
            return (method == "Runtime.evaluate" and "__bipPay.detect" in params.get("expression", "")
                    and self.reloads > 0)
        return method == "Input.insertText" and len(self.typed) == 2   # the name and the number typed

    def _events(self, method, session):
        if method == "Target.setAutoAttach" and session == "S-page":
            return [{"method": "Target.attachedToTarget", "sessionId": "S-page",
                     "params": {"sessionId": child, "targetInfo": {"type": "iframe", "url": self.frame_urls[frame]}}}
                    for child, frame in self.sessions.items() if child != "S-page"]
        return []

    def _node(self, frame, i):
        """The backend node id of field `i` of `frame` (the same for every connection)."""
        return 100 * list(self.sessions.values()).index(frame) + i + 1

    def _answer(self, method, params, session):
        if method == "Target.getTargets":
            others = [{"targetId": t, "type": "page", "url": "https://shop.example/cart"} for t in self.others]
            return {"targetInfos": [{"targetId": "T1", "type": "page", "url": self.url}, *others,
                                    {"targetId": "T0", "type": "service_worker", "url": "https://x.example/sw.js"}]}
        if method == "Target.attachToTarget":
            if params["targetId"] not in ("T1", *self.others):
                raise LookupError("No target with given id found")
            return {"sessionId": "S-page"}
        if method == "Target.closeTarget":
            self.others.remove(params["targetId"])
            self.closed.append(params["targetId"])
            return {"success": True}
        if method == "Storage.clearDataForOrigin":
            self.cleared.append(params)
            return {}
        if method == "Page.reload":
            self.reloads += 1
            self.loader += 1
            self.url = self.reload_to or self.url
            self.values = {}
            self.looks = 0
            if self.lose_form:
                self.fields = {**self.fields, "F-psp": []}
            return {}
        if method == "Page.navigate":
            self.navigated.append(params["url"])
            self.url = params["url"]
            self.loader += 1
            return {"frameId": "F-main", "loaderId": f"L{self.loader}"}
        if method == "Page.resetNavigationHistory":
            self.resets.append(self.url)
            return {}
        if method == "DOM.getFrameOwner":
            return {"backendNodeId": 500 + list(self.sessions.values()).index(params["frameId"])}
        if method == "DOM.resolveNode" and "backendNodeId" in params and params["backendNodeId"] >= 500:
            return {"object": {"objectId": f"owner-{params['backendNodeId']}"}}
        if method == "Runtime.callFunctionOn" and params["objectId"].startswith("owner-"):
            self.anchored.append(list(self.sessions.values())[int(params["objectId"][6:]) - 500])
            return {"result": {"type": "boolean", "value": True}}
        if method == "DOM.describeNode":
            _, context, i = params["objectId"].split("-")
            return {"node": {"backendNodeId": self._node(self.contexts[int(context)], int(i))}}
        if method == "DOM.resolveNode":
            frame = list(self.sessions.values())[(params["backendNodeId"] - 1) // 100]
            if self.contexts.get(params.get("executionContextId")) != frame:
                raise LookupError("Node with given id does not belong to the document")
            return {"object": {"objectId": f"obj-{params['executionContextId']}-{(params['backendNodeId'] - 1) % 100}"}}
        if method == "Runtime.callFunctionOn" and "selectedIndex" in params["functionDeclaration"]:
            _, context, i = params["objectId"].split("-")
            self.values[(self.contexts[int(context)], int(i))] = ""
            self.emptied.append((self.contexts[int(context)], int(i)))
            return {"result": {"type": "boolean", "value": True}}
        if method == "Page.getFrameTree":
            if session != "S-page":
                frame = self.sessions[session]
                return {"frameTree": {"frame": {"id": frame, "url": self.frame_urls[frame]}}}
            children = [{"frame": {"id": f, "url": self.frame_urls[f]}} for f in self.sessions.values() if f != "F-main"]
            return {"frameTree": {"frame": {"id": "F-main", "url": self.url, "loaderId": f"L{self.loader}"},
                                  "childFrames": children}}
        if method == "Page.createIsolatedWorld":
            if self.sessions.get(session) != params["frameId"]:
                raise LookupError("No frame for given id found")
            context = len(self.contexts) + 1
            self.contexts[context] = params["frameId"]
            return {"executionContextId": context}
        if method == "Runtime.evaluate":
            return self._evaluate(params, session)
        if method == "DOM.focus" and self.focus_trap:
            return {}
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
        elif expression == "document.readyState":
            value = "complete"
        elif call:
            name, args = call.group(1), call.group(2)
            first = int(args.split(",")[0]) if args and args.split(",")[0].strip().isdigit() else None
            if name == "detect":
                value = self.fields[frame]
                if frame == "F-psp" and self.reloads:
                    self.looks += 1
                    if self.split and self.looks == 1:
                        value = [f for f in value if f["kind"] == "number"]
            elif name == "focused":
                value = self.focused.get(session) == (frame, first)
            elif name == "el":
                return {"result": {"type": "object", "objectId": f"obj-{context}-{first}"}}
            elif name == "holds":
                text = self.values.get((frame, first), "")
                value = re.sub(r"\D", "", text) if "true" in args else text.strip().lower()
            elif name == "set":
                self.values[(frame, first)] = ""
                value = True
            elif name == "findButton":
                value = self.button
            elif name == "clickButton":
                self.clicks.append(self.url)
                self.url = self.after_click
                self.loader += 1
                value = True
            elif name == "bankAsks":
                value = self.bank_asks
            else:
                value = True
        return {"result": {"type": "object" if isinstance(value, list) else "string", "value": value}}

    def stop(self):
        self.loop.call_soon_threadsafe(lambda: self.done.done() or self.done.set_result(None))


def pay_bridge(bridges, fakes, chrome=None, change=None, place=None, prepare=None):
    bridge = bridges.new(place or "bridge")
    if prepare:   # before it starts (its fake commands' rules)
        prepare(bridge)
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
        chrome.others = ["T2"]   # another tab of the agent's
        # Under the hold, the agent's browser tool calls still running are waited for (busy twice, then none).
        bridge.commands.rule("podman", ["exec", "bipagents-browser-vie", "sh", "-c", CALLS_CHECK], exit=4, times=2)
        result, asked = pay(bridge, ARGS)
        assert result["isError"] is False, result
        # Typed into a fresh document: the other tab closed, the site's service workers removed, the page reloaded.
        assert chrome.closed == ["T2"] and chrome.reloads == 1
        assert chrome.cleared == [{"origin": "https://shop.example", "storageTypes": "service_workers"}]
        podman = [c[-1] for c in bridge.commands.of("podman")]
        assert [c for c in podman if c == CALLS_CHECK] == [CALLS_CHECK] * 3
        assert "printf 'card %s'" in podman[podman.index(CALLS_CHECK) - 1]
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
        # The shop's confirmation is a new document: the agent keeps it (and can look at it).
        assert chrome.navigated == [] and chrome.url == "https://shop.example/done"
        # Its history forgotten (« back » can't bring the typed-into document again); the button was the card form's.
        assert chrome.resets == ["https://shop.example/done"] and chrome.anchored == ["F-psp"]
        assert "empty page" not in confirmed["content"][0]["text"]
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
        assert chrome.navigated == ["about:blank"]   # given up: the page the card was typed into is gone
        assert chrome.resets == ["about:blank"]      # and out of the tab's history before the hold is lifted
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
        assert chrome.navigated == ["about:blank"]   # then the tab left on an empty page, before the hold is lifted
        assert chrome.resets == ["about:blank"]
        assert bridge.commands.of("podman")[-1][-1].endswith("rm -f /tmp/bipagents-hold; exit 0")
        assert "No payment is waiting" in tool(bridge, "confirm_payment", {"button": "Payer"})["content"][0]["text"]
    finally:
        chrome.stop()


HOLD_LIFTED = "rm -f /tmp/bipagents-hold; exit 0"   # how the command lifting the hold ends


def lifted(command):
    return command.endswith(HOLD_LIFTED)


def test_cancel_empties_exactly_the_fields_typed_into(bridges, fakes):
    # Fix review 11/11, R4: clear() used to empty the first field of each kind of any frame: with a second card form
    # in a frame of another site found first, that one was « emptied » and the provider's number stayed typed.
    chrome = FakeChrome(other="https://js.psp.example/f")
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        result, _ = pay(bridge, ARGS)
        assert result["isError"] is False, result
        assert ("F-other", "number", NUMBER) not in chrome.typed and ("F-psp", "number", NUMBER) in chrome.typed
        assert tool(bridge, "cancel_payment")["content"][0]["text"].startswith("The payment is given up")
        assert chrome.values[("F-psp", 0)] == "" and chrome.values[("F-psp", 2)] == ""
        assert sorted(chrome.emptied) == [("F-main", 0), ("F-psp", 0), ("F-psp", 1), ("F-psp", 2)]
        assert ("F-other", 0) not in chrome.values
        assert chrome.navigated == ["about:blank"] and lifted(podman_last(bridge))
    finally:
        chrome.stop()


def podman_last(bridge):
    return bridge.commands.of("podman")[-1][-1]


def test_nothing_is_typed_where_the_keyboard_s_focus_is_not_the_field(bridges, fakes):
    # Fix review 11/11, R6: Input.insertText types into whatever has the focus (a dialog that traps it, a chat box in
    # another frame). The focus is checked in the field and its frame first, focused again once, else nothing typed.
    chrome = FakeChrome()
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        chrome.focus_trap = True
        result, asked = pay(bridge, ARGS)
        assert asked and result["isError"] is True
        assert "would not take the keyboard" in result["content"][0]["text"]
        assert "hand_over_payment" in result["content"][0]["text"]
        assert chrome.typed == [] and all(v == "" for v in chrome.values.values())
        assert chrome.navigated == ["about:blank"] and lifted(podman_last(bridge))
        outcomes = [e["outcome"] for e in bridge.get("/v1/vault/payments", headers=AUTH).json()["journal"]]
        assert outcomes[0] == "failed" and "typed" not in outcomes
    finally:
        chrome.stop()


def test_typing_waits_until_the_card_form_has_finished_appearing(bridges, fakes):
    # Fix review 11/11, R7: the first look that showed a number field started the typing; a provider whose expiry
    # and code frames load a moment later got only the number. Now the fields must be the same in two looks in a row.
    chrome = FakeChrome()
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        chrome.split = True
        result, _ = pay(bridge, ARGS)
        assert result["isError"] is False, result
        assert chrome.looks >= 3   # number alone, then all three, then the same again
        assert chrome.typed == [("F-main", "name", "Alex Martin"), ("F-psp", "number", NUMBER),
                                ("F-psp", "exp", "12/30"), ("F-psp", "csc", CODE)]
        assert "were not found" not in result["content"][0]["text"]
    finally:
        chrome.stop()


def test_a_browser_image_that_counts_no_calls_is_never_typed_into(bridges, fakes):
    # Fix review 11/11, R3: an image from before hold_gate.py counted the calls in flight (no `bipagents-calls` in it)
    # used to be typed into as if none ran. Now nothing is typed: the payment is handed over, the image to update.
    chrome = FakeChrome()
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        bridge.commands.rule("podman", ["exec", "bipagents-browser-vie", "sh", "-c", CALLS_CHECK], exit=5)
        result, asked = pay(bridge, ARGS)
        assert asked and result["isError"] is True
        text = result["content"][0]["text"]
        assert "must be updated" in text and "hand_over_payment" in text and "nothing was typed" in text
        assert chrome.typed == [] and chrome.reloads == 0 and chrome.navigated == []
        assert lifted(podman_last(bridge))
        again, _ = pay(bridge, ARGS)
        assert again["isError"] is True and chrome.typed == []
        assert bridge.log_path.read_text().count("the browser image must be updated") == 1   # said once
    finally:
        chrome.stop()


@pytest.mark.parametrize("how", ["cancel_payment", "suspend"])
def test_a_payment_stopped_while_the_page_reloads_types_nothing(bridges, fakes, how):
    # Fix review 11/11, B1: cancel_payment (another run of the agent) or « Suspendre tous les paiements » arriving while
    # the bridge reloads the payment page used to lift the hold while the bridge went on and typed the card into a page
    # nobody held any more. Now the payment is asked to stop first, types nothing, leaves the tab on an empty page,
    # and the hold is lifted once, after that.
    chrome = FakeChrome()
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        chrome.reload_delay = 2
        paid = {}
        paying = threading.Thread(target=lambda: paid.update(result=pay(bridge, ARGS)[0]))
        paying.start()
        deadline = time.monotonic() + 60
        while not chrome.reloading and time.monotonic() < deadline:
            time.sleep(0.05)
        assert chrome.reloading, "the payment page was never reloaded"
        if how == "cancel_payment":
            answer = tool(bridge, "cancel_payment")["content"][0]["text"]
            assert answer.startswith("The payment is given up")
        else:
            assert bridge.post("/v1/vault/payments", headers=AUTH, json={"enabled": False}).json()["active"] == []
        paying.join(60)
        result = paid["result"]
        assert result["isError"] is True and "stopped" in result["content"][0]["text"], result
        assert chrome.typed == [] and chrome.values == {}
        assert chrome.navigated == ["about:blank"] and chrome.resets == ["about:blank"]
        podman = [c[-1] for c in bridge.commands.of("podman")]
        assert sum(map(lifted, podman)) == 1 and lifted(podman[-1])   # once, after the empty page
        outcome = "cancelled" if how == "cancel_payment" else "suspended"
        outcomes = [e["outcome"] for e in bridge.get("/v1/vault/payments", headers=AUTH).json()["journal"]]
        assert outcomes.count(outcome) == 1 and "typed" not in outcomes and "failed" not in outcomes
        assert bridge.get("/v1/vault/payments", headers=AUTH).json()["active"] == []
    finally:
        chrome.stop()


def test_a_page_that_comes_back_without_its_form_or_elsewhere_is_handed_over(bridges, fakes):
    chrome = FakeChrome()
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        form = dict(chrome.fields)
        # A one-page checkout that loses its state when reloaded: nothing typed, the person pays.
        chrome.lose_form = True
        lost, asked = pay(bridge, ARGS)
        assert asked and lost["isError"] is True
        text = lost["content"][0]["text"]
        assert "without its card form" in text and "hand_over_payment" in text and "nothing was typed" in text
        assert chrome.typed == [] and chrome.reloads == 1
        assert chrome.navigated == [] and chrome.resets == []   # nothing typed: the page stays as it came back
        assert bridge.commands.of("podman")[-1][-1].endswith("rm -f /tmp/bipagents-hold; exit 0")
        # A reload that ends on another site (a redirect): not the payment the person approved.
        chrome.lose_form, chrome.fields = False, form
        chrome.reload_to = "https://evil.example/checkout"
        moved, _ = pay(bridge, ARGS)
        assert moved["isError"] is True and "another address" in moved["content"][0]["text"]
        assert "hand_over_payment" in moved["content"][0]["text"] and chrome.typed == []
        assert bridge.get("/v1/vault", headers=AUTH).json()["items"][0]["card"]["spent"] == {}
        outcomes = [e["outcome"] for e in bridge.get("/v1/vault/payments", headers=AUTH).json()["journal"]]
        assert outcomes[:6] == ["failed", "approved", "asked", "failed", "approved", "asked"]
    finally:
        chrome.stop()


def test_a_card_form_in_a_frame_of_an_unknown_site_is_handed_over(bridges, fakes):
    chrome = FakeChrome(psp="https://js.psp.example/f")
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        refused, asked = pay(bridge, ARGS)
        assert asked is None and refused["isError"] is True   # nobody asked: the card could not go there
        text = refused["content"][0]["text"]
        assert "frame of a payment service" in text and "hand_over_payment" in text
        assert chrome.typed == [] and not [c for c in bridge.commands.of("podman") if "printf" in c[-1]]
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


# -- the person takes the browser over during a payment (security check 11/11, N1) ------------------------------------

def with_desktop(config):
    """The agent's browser shown in the app's live view (« Prendre la main »)."""
    config["agents"]["Vie"]["desktop_vnc"] = f"tcp://127.0.0.1:{free_port()}"


def flag_kinds(bridge, tmp_path):
    """What the hold's flag reads after each command the bridge ran on it, the commands replayed in order by a real
    shell on a file of their own (the fake podman only records them): « card », « human » or None (no flag)."""
    flag = tmp_path / "hold"
    flag.unlink(missing_ok=True)
    kinds = []
    for argv in bridge.commands.of("podman"):
        if (argv[:4] != ["exec", "bipagents-browser-vie", "sh", "-c"] or "/tmp/bipagents-hold" not in argv[-1]
                or argv[-1] == STALE_CHECK):   # the start-up look, which changes nothing
            continue
        subprocess.run(["sh", "-c", argv[-1].replace("/tmp/bipagents-hold", str(flag))], check=False)
        kinds.append(flag.read_text().split(" ")[0] if flag.exists() else None)
    return kinds


def control(bridge, take):
    return bridge.post("/v1/agents/vie/desktop/control", headers=AUTH, json={"take": take})


def test_taking_over_during_a_payment_never_lifts_its_hold(bridges, fakes, tmp_path):
    # The person's take-over used to write « human » over the payment's « card » flag and their hand-back removed it:
    # the bridge's own clean-up was then refused by the relay and the agent's tools ran in the tab holding the card.
    # Now the payment's hold stays for the agent while the person answers the bank in the page, and only the
    # payment's release (fields emptied, page left, history forgotten) removes it.
    chrome = FakeChrome()
    try:
        bridge = pay_bridge(bridges, fakes, chrome, change=with_desktop)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        result, _ = pay(bridge, ARGS)
        assert result["isError"] is False, result
        # The bank asks in the page (3-D Secure): the page does not move on after the click.
        chrome.after_click, chrome.bank_asks = chrome.url, True
        asked = tool(bridge, "confirm_payment", {"button": "Payer 12,50 €"})["content"][0]["text"]
        assert "bank asks" in asked and "Prendre la main" in asked
        taken = control(bridge, True)
        assert taken.status_code == 200 and taken.json() == {"holder": "human", "you_hold": True}
        assert flag_kinds(bridge, tmp_path)[-1] == "card"
        assert not any("printf 'human" in c[-1] for c in bridge.commands.of("podman"))
        back = control(bridge, False)
        assert back.status_code == 200 and back.json()["you_hold"] is False
        # Handed back: still the payment's hold, nothing of the page given back yet.
        kinds = flag_kinds(bridge, tmp_path)
        assert kinds[0] == "card" and set(kinds) == {"card"}, kinds
        assert chrome.navigated == [] and chrome.values[("F-psp", 0)] == NUMBER
        assert [a["agent"] for a in bridge.get("/v1/vault/payments", headers=AUTH).json()["active"]] == ["vie"]
        # The payment ends: the card erased and the page left before the hold goes.
        assert tool(bridge, "cancel_payment")["content"][0]["text"].startswith("The payment is given up")
        assert chrome.values[("F-psp", 0)] == "" and chrome.navigated == ["about:blank"]
        assert chrome.resets == ["about:blank"]
        kinds = flag_kinds(bridge, tmp_path)
        assert kinds[-1] is None and set(kinds[:-1]) == {"card"}, kinds
        assert sum(map(lifted, (c[-1] for c in bridge.commands.of("podman")))) == 1
    finally:
        chrome.stop()


def test_a_payment_that_ends_while_the_person_holds_the_browser_leaves_it_to_them(bridges, fakes, tmp_path):
    chrome = FakeChrome()
    try:
        bridge = pay_bridge(bridges, fakes, chrome, change=with_desktop)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        assert pay(bridge, ARGS)[0]["isError"] is False
        assert control(bridge, True).status_code == 200
        # The payment ends while they hold it: cleaned first, then the hold is theirs (the agent still waits).
        assert tool(bridge, "cancel_payment")["content"][0]["text"].startswith("The payment is given up")
        assert chrome.navigated == ["about:blank"] and chrome.resets == ["about:blank"]
        assert flag_kinds(bridge, tmp_path)[-2:] == ["card", "human"]
        token = (bridge.data / "browsers" / "vie" / "hold.token").read_text()
        assert f"printf 'human %s' {token}" in bridge.commands.of("podman")[-1][-1]
        # Handed back: the agent's browser is its own again.
        assert control(bridge, False).status_code == 200
        assert flag_kinds(bridge, tmp_path)[-1] is None
        assert not (bridge.data / "browsers" / "vie" / "hold.token").exists()
    finally:
        chrome.stop()


def test_a_take_over_while_the_bridge_holds_the_browser_is_refused(bridges, fakes):
    # A payment or a vault fill starting or ending (its flag set, not known as a payment here): not taken, nothing
    # written over the bridge's hold, and a line the app can show.
    chrome = FakeChrome()
    try:
        bridge = pay_bridge(bridges, fakes, chrome, change=with_desktop)
        # (the start-up look for a payment's hold left behind first: none here, so nothing else is done)
        assert wait_until(lambda: STALE_CHECK in [c[-1] for c in bridge.commands.of("podman")])
        assert [c[-1] for c in bridge.commands.of("podman") if c[:4] == ["exec", "bipagents-browser-vie", "sh", "-c"]
                ] == [STALE_CHECK]
        bridge.commands.rule("podman", ["exec", "bipagents-browser-vie", "sh", "-c"], exit=3, times=1)
        refused = control(bridge, True)
        assert refused.status_code == 409, refused.text
        detail = refused.json()["detail"]
        assert detail["code"] == "payment_in_progress" and "payment" in detail["text"].lower()
        assert bridge.get("/v1/agents/vie/desktop/state", headers=AUTH).json()["you_hold"] is False
        assert bridge.commands.of("podman")[-1][-1].endswith("= card ] || rm -f /tmp/bipagents-hold")
        assert not (bridge.data / "browsers" / "vie" / "hold.token").exists()
    finally:
        chrome.stop()


# -- a bridge restarted mid-payment (security check 11/11, N2) ----------------------------------------------------------

STALE_CHECK = '[ "$(head -c 4 /tmp/bipagents-hold 2>/dev/null)" = card ] && exit 7; exit 0'


def stale_hold(bridge):
    """The agent's browser still carries a payment's flag from the bridge's previous run."""
    bridge.commands.rule("podman", ["exec", "bipagents-browser-vie", "sh", "-c", STALE_CHECK], exit=7)


def test_a_payments_hold_left_by_a_stopped_bridge_is_ended_at_start(bridges, fakes):
    # The held payments are memory only: a bridge restarted mid-payment left the « card » flag and nobody ended it,
    # with the card maybe still typed in a tab. At start the bridge now leaves every tab (empty page, history
    # forgotten), then lifts the hold.
    chrome = FakeChrome()
    chrome.others = ["T2"]
    try:
        bridge = pay_bridge(bridges, fakes, chrome, prepare=stale_hold)
        assert wait_until(lambda: any(lifted(c[-1]) for c in bridge.commands.of("podman")))
        podman = [c[-1] for c in bridge.commands.of("podman")]
        rekey = next(c for c in podman if "printf 'card %s'" in c)
        assert rekey.startswith('[ "$(head -c 4 /tmp/bipagents-hold 2>/dev/null)" = card ] || exit 1;')
        token = re.search(r"printf 'card %s' ([0-9a-f]+) >", rekey).group(1)
        assert token in chrome.headers                              # the page reached through the hold
        assert chrome.navigated == ["about:blank", "about:blank"]   # every tab: the agent's and the other one
        assert chrome.resets == ["about:blank", "about:blank"]
        assert podman.index(STALE_CHECK) < podman.index(rekey) < [i for i, c in enumerate(podman) if lifted(c)][0]
        assert sum(map(lifted, podman)) == 1
        assert not [c for c in bridge.commands.of("systemctl") if "restart" in c]
        assert "a hold left by a stopped bridge" in bridge.log_path.read_text()
    finally:
        chrome.stop()


def test_a_payments_hold_left_in_a_browser_that_cannot_be_reached_restarts_it(bridges, fakes):
    def prepare(bridge):
        folder = bridge.data / "browsers" / "vie"
        folder.mkdir(parents=True)
        (folder / "ports.json").write_text(json.dumps({"cdp": free_port(), "vnc": free_port()}))   # nothing there
        stale_hold(bridge)

    bridge = pay_bridge(bridges, fakes, prepare=prepare)
    assert wait_until(lambda: any(lifted(c[-1]) for c in bridge.commands.of("podman")))
    assert ["--user", "restart", "bipagents-browser-vie"] in bridge.commands.of("systemctl")
    entries = [(e["cmd"], e["argv"]) for e in bridge.commands.all() if e["cmd"] in ("podman", "systemctl")]
    restart = entries.index(("systemctl", ["--user", "restart", "bipagents-browser-vie"]))
    assert [i for i, (cmd, argv) in enumerate(entries) if cmd == "podman" and lifted(argv[-1])][0] > restart


# -- pay_with_card left before its answer (security check 11/11, N3) ---------------------------------------------------

def pay_and_leave(bridge, chrome):
    """pay_with_card as Hermes calls it, the stream closed (the run stopped, Hermes restarted) once the fake browser
    is held up at its slow step (`chrome.slow`): the call's task is aborted there."""
    body = {"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "pay_with_card", "arguments": ARGS}}
    headers = {"Authorization": f"Bearer {TOKEN}", "Accept": "application/json, text/event-stream"}
    with httpx.Client(timeout=90) as client, client.stream("POST", bridge.url + PAY, json=body, headers=headers) as resp:
        for line in resp.iter_lines():
            if not line.startswith("data: "):
                continue
            message = json.loads(line[6:])
            if message.get("method") == "elicitation/create":
                payment = re.search(r"Card payment (pay_[0-9a-f]{12}):", message["params"]["message"]).group(1)
                sent = bridge.post(f"/v1/vault/payments/{payment}/approve", headers=AUTH, json={"code": CODE})
                assert sent.status_code == 204, sent.text
                answer = {"jsonrpc": "2.0", "id": message["id"], "result": {"action": "accept", "content": {}}}
                assert client.post(bridge.url + PAY, json=answer, headers=headers).status_code == 202
                assert wait_until(lambda: chrome.slowed, timeout=60), "the slow step was never reached"
                return   # the stream closed here
            if message.get("id") == 7:
                raise AssertionError(f"answered before the slow step: {message}")


@pytest.mark.parametrize("step", ["reload", "detect", "type"])
def test_a_payment_left_at_any_step_after_its_hold_is_given_back_once(bridges, fakes, step):
    # `watch`, the only way back for a typed card, started after the typing and after recording the payment: a call
    # aborted (Hermes closed its stream) or failing in between left a held entry nobody watched, the card typed.
    chrome = FakeChrome()
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        chrome.slow = step
        pay_and_leave(bridge, chrome)
        assert wait_until(lambda: any(lifted(c[-1]) for c in bridge.commands.of("podman")), timeout=40)
        time.sleep(10)   # the step held up has answered meanwhile: nothing more happens
        podman = [c[-1] for c in bridge.commands.of("podman")]
        assert sum(map(lifted, podman)) == 1
        assert bridge.get("/v1/vault/payments", headers=AUTH).json()["active"] == []
        outcomes = [e["outcome"] for e in bridge.get("/v1/vault/payments", headers=AUTH).json()["journal"]]
        assert outcomes.count("failed") == 1 and "typed" not in outcomes, outcomes
        assert "ended before its answer" in bridge.log_path.read_text()
        if step == "type":
            # Typing began: the document the card went into is gone (empty page), and out of the tab's history.
            assert ("F-psp", "number", NUMBER) in chrome.typed and len(chrome.typed) <= 3
            assert chrome.navigated == ["about:blank"] and chrome.resets == ["about:blank"]
        else:
            assert chrome.typed == [] and chrome.navigated == []
        assert bridge.get("/v1/vault", headers=AUTH).json()["items"][0]["card"]["spent"] == {}
    finally:
        chrome.stop()
