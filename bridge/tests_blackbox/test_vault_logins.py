"""Accounts an agent creates with a password of the vault (`/v1/agent-tools/vault`, `/v1/vault/logins/requests`):
one card for the person first (listed with the approvals), then a password made by the bridge typed into a fake Chrome's
password fields only (its DevTools protocol), kept encrypted « en attente de confirmation » and never handed back to
the agent nor logged; corrected rules within 15 minutes need no new card; the person's hold on the browser is
respected; only a frame of the approved site is typed into, its origin checked again in the page before typing."""
import json
import re
import threading
import time
from concurrent.futures import ThreadPoolExecutor

import asyncio
from cryptography.fernet import Fernet
from websockets.asyncio.server import serve

from bbkit.common import AUTH
from bbkit.mcp import give_token
from bbkit.servers import free_port

TOKEN = "agent-token-for-vie-0123456789abcdef"
VAULT = "/v1/agent-tools/vault"
ANSWER = json.dumps({"id": "vault_1", "removed": True}) + "\n"
ARGS = {"site": "https://shop.example", "username": "alex@example.org", "reason": "Pour commander le cadeau d'Inès"}


class FakeChrome:
    """Chrome's DevTools as the bridge uses them: one page on `url` with an e-mail field and two password fields (the
    second one its confirmation). It answers the login script's calls by name (`detect` reports `self.reported`, the
    page's inputs the script would pick) and the bridge's checks on a field (`Runtime.callFunctionOn`), and records
    what was typed where.

    `fields_frame`: the address of a child frame holding the password fields (None: the main frame holds them).
    `live_origin`: what `location.origin` says in the page when the bridge checks a field (None: that frame's own),
    a page that went to another site after Chrome's frame tree was read."""

    def __init__(self, url="https://shop.example/signup"):
        self.url = url
        self.fields_frame = None
        self.live_origin = None
        self.port = free_port()
        self.inputs = [{"name": "email", "type": "email", "value": "alex@example.org"},
                       {"name": "password", "type": "password", "value": ""},
                       {"name": "confirm", "type": "password", "value": ""}]
        self.reported = [1, 2]       # the inputs `detect` gives, in order
        self.focused_field = True
        self.typed = []              # (input name, text)
        self.headers = []            # x-bip-card of each connection
        self.focused = None          # index into self.inputs
        self.loop = asyncio.new_event_loop()
        self.ready = threading.Event()
        threading.Thread(target=self._run, daemon=True).start()
        self.ready.wait(5)

    def value(self, name):
        return next(i["value"] for i in self.inputs if i["name"] == name)

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
            result = self._answer(method, params)
            await ws.send(json.dumps({"id": message["id"], "result": result, **({"sessionId": session} if session else {})}))

    def _input(self, object_id):
        return self.reported[int(object_id.split("-")[-1])]

    def _answer(self, method, params):
        if method == "Target.getTargets":
            return {"targetInfos": [{"targetId": "T1", "type": "page", "url": self.url}]}
        if method == "Target.attachToTarget":
            return {"sessionId": "S-page"}
        if method == "Page.getFrameTree":
            tree = {"frame": {"id": "F-main", "url": self.url}}
            if self.fields_frame:
                tree["childFrames"] = [{"frame": {"id": "F-child", "url": self.fields_frame}}]
            return {"frameTree": tree}
        if method == "Page.createIsolatedWorld":
            return {"executionContextId": 2 if params.get("frameId") == "F-child" else 1}
        if method == "DOM.focus":
            self.focused = self._input(params["objectId"])
            return {}
        if method == "Input.insertText":
            target = self.inputs[self.focused]
            target["value"] = params["text"]
            self.typed.append((target["name"], params["text"]))
            return {}
        if method == "Runtime.callFunctionOn":
            index = self._input(params["objectId"])
            field, function = self.inputs[index], params["functionDeclaration"]
            args = [a.get("value") for a in params.get("arguments") or []]
            arg = args[0] if args else None
            password = field["type"] == "password"
            if "activeElement" in function:
                value = password and self.focused == index and arg == self.origin()
            elif "this.value === v" in function:
                value = password and field["value"] == arg
            elif "p.set(" in function:
                value = password and args[1:] == [self.origin()]
                if value:
                    field["value"] = arg
            else:
                value = password
            return {"result": {"type": "boolean", "value": value}}
        if method == "Runtime.evaluate":
            expression = params["expression"]
            if expression == "document.visibilityState":
                return {"result": {"type": "string", "value": "visible"}}
            call = re.search(r"__bipLogin\.(\w+)\(([^)]*)\)\s*$", expression)
            if not call:
                return {"result": {"type": "undefined"}}
            name, args = call.group(1), call.group(2)
            first = int(args.split(",")[0]) if args and args.split(",")[0].strip().isdigit() else None
            if name == "detect":
                if params.get("contextId") != (2 if self.fields_frame else 1):
                    return {"result": {"type": "object", "value": {"focused": False, "count": 0, "fields": []}}}
                fields = [{"i": k, "type": "password", "empty": not self.inputs[n]["value"]} for k, n in enumerate(self.reported)]
                value = {"focused": self.focused_field, "count": 2, "fields": fields}
                return {"result": {"type": "object", "value": value}}
            if name == "el":
                return {"result": {"type": "object", "objectId": f"obj-1-{first}"}}
            if name == "set":
                field = self.inputs[self.reported[first]]
                if field["type"] == "password":
                    field["value"] = ""
            return {"result": {"type": "boolean", "value": True}}
        return {}

    def origin(self):
        """`location.origin` in the frame holding the fields, as the page says it."""
        if self.live_origin:
            return self.live_origin
        scheme, rest = (self.fields_frame or self.url).split("://", 1)
        return f"{scheme}://{rest.split('/', 1)[0]}"

    def stop(self):
        self.loop.call_soon_threadsafe(lambda: self.done.done() or self.done.set_result(None))


def login_bridge(bridges, fakes, chrome=None):
    bridge = bridges.new()
    bridge.config["agents"]["Vie"] = {"hermes_url": fakes.down.url, "hermes_key": "k" * 32, "isolated": True,
                                      "python": f"{bridge.fakebin}/podman exec -i hermes-vie python",
                                      "hermes_home": "/home/hermes/.hermes"}
    bridge.config["hermes"] = {"isolated": True, "bin": str(bridge.fakebin / "hermes"), "hub_url": fakes.hermes.url}
    if chrome:
        folder = bridge.data / "browsers" / "vie"
        folder.mkdir(parents=True)
        (folder / "ports.json").write_text(json.dumps({"cdp": chrome.port, "vnc": chrome.port + 1}))
    bridge.commands.rule("podman", ["exec", "-i", "hermes-vie"], stdout=ANSWER)
    bridge.start()
    give_token(bridge, "vie", TOKEN)
    return bridge


def tool(bridge, name, arguments):
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}
    resp = bridge.post(VAULT, json=body, headers={"Authorization": f"Bearer {TOKEN}"}, timeout=120)
    return resp.json()["result"]


def cards(bridge):
    items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": "vie"}).json()["items"]
    return [i for i in items if i["run_id"].startswith("bip-login-")]


def wait_card(bridge, timeout=15):
    deadline = time.time() + timeout
    while time.time() < deadline:
        found = cards(bridge)
        if found:
            return found[0]
        time.sleep(0.1)
    raise AssertionError("no card")


def create(bridge, arguments=None, allow=True):
    """vault_create_login as the agent calls it; the person answers its card (`allow`: True, False or None for no
    card expected). Returns (result, card)."""
    with ThreadPoolExecutor(1) as pool:
        call = pool.submit(tool, bridge, "vault_create_login", arguments or ARGS)
        card = None
        if allow is not None:
            card = wait_card(bridge)
            answered = bridge.post(f"/v1/vault/logins/requests/{card['request_id']}", headers=AUTH, json={"allow": allow})
            assert answered.status_code == 200, answered.text
        return call.result(timeout=120), card


def vault_records(bridge):
    folder = bridge.state_dir / "vault"
    key = Fernet((folder / "vault.key").read_bytes().strip())
    return json.loads(key.decrypt((folder / "vault.json.enc").read_bytes()))


def test_the_tools_are_listed_for_an_agent_that_may_hold_credentials(bridges, fakes):
    bridge = login_bridge(bridges, fakes)
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}
    listed = bridge.post(VAULT, json=body, headers={"Authorization": f"Bearer {TOKEN}"}).json()["result"]["tools"]
    assert [t["name"] for t in listed] == ["vault_create_login", "vault_confirm_login", "vault_contact_details",
                                         "vault_save_contact_details"]
    assert "captchas stay the person's" in listed[0]["description"]
    assert bridge.post(VAULT, json=body, headers={"Authorization": "Bearer nope"}).status_code == 401


def test_a_card_first_then_the_password_typed_into_password_fields_only_never_returned_nor_logged(bridges, fakes):
    chrome = FakeChrome()
    try:
        bridge = login_bridge(bridges, fakes, chrome)
        result, card = create(bridge)
        # The card, as the apps list it.
        assert card["choices"] == ["once", "deny"] and card["session_id"] is None
        assert card["bip_login_site"] == "shop.example" and card["bip_login_username"] == "alex@example.org"
        assert card["bip_login_reason"] == "Pour commander le cadeau d'Inès"
        assert card["description"] == ("Vie veut créer un compte sur shop.example (identifiant alex@example.org) "
                                       "avec un mot de passe du coffre")
        assert result["isError"] is False, result
        text = result["content"][0]["text"]
        assert "saved as « shop.example »" in text and "vault_confirm_login" in text
        password = chrome.value("password")
        assert len(password) == 20 and chrome.value("confirm") == password
        assert any(c.islower() for c in password) and any(c.isupper() for c in password)
        assert any(c.isdigit() for c in password) and any(not c.isalnum() for c in password)
        # Typed into the password field and its confirmation, nothing else.
        assert chrome.typed == [("password", password), ("confirm", password)]
        assert chrome.value("email") == "alex@example.org"
        # The browser was held meanwhile (the flag reads « card <token> », shown to DevTools), then given back.
        holds = [c for c in bridge.commands.of("podman") if c[:2] == ["exec", "bipagents-browser-vie"] and "printf" in c[-1]]
        assert len(holds) == 1
        token = re.search(r"printf 'card %s' ([0-9a-f]+) >", holds[0][-1]).group(1)
        assert token in chrome.headers and chrome.headers[0] is None
        assert bridge.commands.of("podman")[-1][-1].endswith("rm -f /tmp/bipagents-hold; exit 0")
        # Kept encrypted, waiting for confirmation, with the agent that created it; given to its own Hermes vault.
        [item] = bridge.get("/v1/vault", headers=AUTH).json()["items"]
        assert {k: item[k] for k in ("kind", "label", "origin", "identifier", "identifier_type", "agents",
                                      "created_by", "pending")} == {
            "kind": "login", "label": "shop.example", "origin": "https://shop.example", "identifier": "alex@example.org",
            "identifier_type": "email", "agents": ["vie"], "created_by": "vie", "pending": True}
        assert password.encode() not in (bridge.state_dir / "vault" / "vault.json.enc").read_bytes()
        assert vault_records(bridge)[0]["secret"] == {"password": password}
        [add] = [json.loads(e["stdin"]) for e in bridge.commands.entries("podman") if e["argv"][:3] == ["exec", "-i", "hermes-vie"]]
        assert add["op"] == "add" and add["origin"] == "https://shop.example" and add["secret"]["password"] == password
        # Never shown to the agent, never in a log or a card; the person can see it (the app asks for Face ID).
        answered = bridge.get("/v1/approvals/answered", headers=AUTH, params={"agent": "vie"}).json()["items"]
        assert [(a["topic"], a["choice"], a["via"]) for a in answered] == [("vault_login", "once", "app")]
        for where in (json.dumps(result), json.dumps(answered), bridge.get("/v1/vault", headers=AUTH).text,
                      (bridge.root / "bridge.log").read_text()):
            assert password not in where
        shown = bridge.post(f"/v1/vault/{item['id']}/reveal", headers=AUTH)
        assert shown.status_code == 200 and shown.json() == {"password": password}
        assert shown.headers["cache-control"] == "no-store"
        assert password not in (bridge.root / "bridge.log").read_text()
        # The site accepted: the agent confirms.
        confirmed = tool(bridge, "vault_confirm_login", {"site": "shop.example", "username": "Alex@example.org"})
        assert confirmed["isError"] is False and "Confirmed" in confirmed["content"][0]["text"]
        [item] = bridge.get("/v1/vault", headers=AUTH).json()["items"]
        assert item["pending"] is False and item["confirmed_at"]
        # Created and confirmed: a second sign-up for it is refused (log in with the vault instead).
        again = tool(bridge, "vault_create_login", ARGS)
        assert again["isError"] is True and "already in the vault" in again["content"][0]["text"]
        assert cards(bridge) == []
    finally:
        chrome.stop()


def test_a_refusal_conflicting_rules_another_site_or_a_known_login_type_nothing(bridges, fakes):
    chrome = FakeChrome()
    try:
        bridge = login_bridge(bridges, fakes, chrome)
        conflict = tool(bridge, "vault_create_login", {**ARGS, "rules": {"require_symbol": True, "allowed_symbols": ""}})
        assert conflict["isError"] is True
        assert "a special character is required but none is allowed" in conflict["content"][0]["text"]
        elsewhere = tool(bridge, "vault_create_login", {**ARGS, "site": "other.example"})
        assert elsewhere["isError"] is True and "not https://other.example" in elsewhere["content"][0]["text"]
        assert cards(bridge) == []
        refused, card = create(bridge, allow=False)
        assert card and refused["isError"] is True and "The person refused" in refused["content"][0]["text"]
        assert chrome.typed == [] and bridge.get("/v1/vault", headers=AUTH).json()["items"] == []
        assert not [c for c in bridge.commands.of("podman") if "printf" in c[-1]]
        answered = bridge.get("/v1/approvals/answered", headers=AUTH, params={"agent": "vie"}).json()["items"]
        assert [(a["topic"], a["choice"]) for a in answered] == [("vault_login", "deny")]
        # A refusal is not remembered as a yes: the next call asks again.
        _, card = create(bridge, allow=False)
        assert card
        # A login the person entered for that site and identifier: no account to create.
        person = {"label": "Shop", "site": "shop.example", "identifier": "alex@example.org", "password": "mine-123!",
                  "agents": ["vie"]}
        assert bridge.post("/v1/vault", headers=AUTH, json=person).status_code == 201
        known = tool(bridge, "vault_create_login", ARGS)
        assert known["isError"] is True and "already in the vault" in known["content"][0]["text"]
        assert chrome.typed == []
        # Its password stays write-only.
        item = bridge.get("/v1/vault", headers=AUTH).json()["items"][0]
        assert bridge.post(f"/v1/vault/{item['id']}/reveal", headers=AUTH).status_code == 400
    finally:
        chrome.stop()


def test_corrected_rules_within_the_window_need_no_new_card_and_replace_the_waiting_login(bridges, fakes):
    chrome = FakeChrome()
    try:
        bridge = login_bridge(bridges, fakes, chrome)
        first, _ = create(bridge)
        assert first["isError"] is False
        before = chrome.value("password")
        rules = {"max_length": 12, "allowed_symbols": "", "require_digit": True}
        retry, card = create(bridge, {**ARGS, "rules": rules}, allow=None)
        assert card is None and retry["isError"] is False, retry
        after = chrome.value("password")
        assert after != before and len(after) == 12 and after.isalnum() and any(c.isdigit() for c in after)
        records = vault_records(bridge)
        assert len(records) == 1 and records[0]["secret"] == {"password": after} and records[0]["pending"] is True
        answered = bridge.get("/v1/approvals/answered", headers=AUTH, params={"agent": "vie"}).json()["items"]
        assert len(answered) == 1                                      # one card for both
        # The agent's own Hermes vault: the first one taken back, the new one given.
        ops = [json.loads(e["stdin"])["op"] for e in bridge.commands.entries("podman") if e["argv"][:3] == ["exec", "-i", "hermes-vie"]]
        assert ops == ["add", "remove", "add"]
        # Another identifier is another account: a new card.
        other, card = create(bridge, {**ARGS, "username": "ines@example.org"}, allow=False)
        assert card and other["isError"] is True
    finally:
        chrome.stop()


def test_the_persons_hold_is_respected(bridges, fakes):
    chrome = FakeChrome()
    try:
        bridge = login_bridge(bridges, fakes, chrome)
        hold = bridge.data / "browsers" / "vie" / "hold.token"
        hold.write_text("abc123\n")
        waiting = tool(bridge, "vault_create_login", ARGS)
        assert waiting["isError"] is True and "using your browser" in waiting["content"][0]["text"]
        assert cards(bridge) == [] and chrome.typed == []
        hold.unlink()
        # Taken by the person while they were asked: the flag is there, nothing is typed.
        bridge.commands.rule("podman", ["exec", "bipagents-browser-vie", "sh", "-c"], exit=3, times=1)
        held, card = create(bridge)
        assert card and held["isError"] is True and "using your browser" in held["content"][0]["text"]
        assert chrome.typed == [] and bridge.get("/v1/vault", headers=AUTH).json()["items"] == []
    finally:
        chrome.stop()


def test_never_typed_into_a_field_that_is_not_a_password(bridges, fakes):
    chrome = FakeChrome()
    try:
        bridge = login_bridge(bridges, fakes, chrome)
        chrome.reported = [0]          # a page whose « password » field turned into its e-mail field
        result, card = create(bridge)
        assert card and result["isError"] is True and "could not be typed" in result["content"][0]["text"]
        assert chrome.typed == [] and chrome.value("email") == "alex@example.org"
        assert bridge.get("/v1/vault", headers=AUTH).json()["items"] == []
    finally:
        chrome.stop()


def test_a_password_field_in_another_sites_frame_is_never_typed_into(bridges, fakes):
    """The right page, but the field the agent focused lives in a frame of another site (an ad, a look-alike widget,
    a subdomain): nothing is asked, nothing typed. A frame of the approved site itself still is."""
    chrome = FakeChrome()
    try:
        bridge = login_bridge(bridges, fakes, chrome)
        for frame in ("https://ads.evil.test/login", "https://shop.example.evil.test/x", "https://www.shop.example/x",
                      "http://shop.example/x", "about:blank"):
            chrome.fields_frame = frame
            refused = tool(bridge, "vault_create_login", ARGS)
            assert refused["isError"] is True and "No password field" in refused["content"][0]["text"], frame
        assert cards(bridge) == [] and chrome.typed == []
        assert bridge.get("/v1/vault", headers=AUTH).json()["items"] == []
        chrome.fields_frame = "https://shop.example/embedded-signup"
        result, card = create(bridge)
        assert card and result["isError"] is False, result
        assert [name for name, _ in chrome.typed] == ["password", "confirm"]
    finally:
        chrome.stop()


def test_a_page_that_left_the_site_before_typing_gets_nothing(bridges, fakes):
    """Chrome's frame tree still showed the approved site, but in the page the field's frame is now elsewhere: the
    check made on the field right before typing refuses, nothing is typed nor kept."""
    chrome = FakeChrome()
    try:
        bridge = login_bridge(bridges, fakes, chrome)
        chrome.live_origin = "https://evil.test"
        result, card = create(bridge)
        assert card and result["isError"] is True and "could not be typed" in result["content"][0]["text"]
        assert chrome.typed == [] and chrome.value("password") == "" and chrome.value("confirm") == ""
        assert bridge.get("/v1/vault", headers=AUTH).json()["items"] == []
    finally:
        chrome.stop()
