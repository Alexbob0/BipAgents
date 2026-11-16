"""Logging in with a login of the vault (`vault_login`, `/v1/agent-tools/vault`, 16/11): the bridge reads the agent's
page from a fake Chrome's DevTools, picks the login kept for that page's site (its registrable domain, then its login
page when several share the domain), and types the identifier and the password into that site's frames only. Neither
ever reaches the agent's answer or the bridge's log.

The page mirrors the extranet Alex could not log in to (« page de connexion propre au compte »): one form posting to
`../login_externe.php`, `<input name="login" type="text">` without id nor label, `<input name="mdp" id="mdp"
type="password">`, two hidden `groupe`, a submit. Fake entries only."""
import asyncio
import json
import re
import threading

from websockets.asyncio.server import serve

from bbkit.common import AUTH
from bbkit.mcp import give_token
from bbkit.servers import free_port

TOKEN = "agent-token-for-vie-0123456789abcdef"
VAULT = "/v1/agent-tools/vault"
ANSWER = json.dumps({"id": "vault_1", "removed": True}) + "\n"
SITE = "https://extranet2.example.fr"
PAGE_A = f"{SITE}/V5/connexion-agence-a.html"
PAGE_B = f"{SITE}/V5/connexion-agence-b.html"
PASSWORD_A = "Fake-A-pw-7781!"
PASSWORD_B = "Fake-B-pw-3302!"
IDENTIFIER_A = "agence-a@example.org"
IDENTIFIER_B = "agence-b@example.org"


class Frame:
    """One frame of the fake page: its address, and its login form as `detectLogin` would read it (`identifier`: an
    identifier field, `password`: a current-password field)."""

    def __init__(self, url, identifier=True, password=True, context=1):
        self.url = url
        self.identifier = identifier
        self.password = password
        self.context = context
        self.values = {"login": "", "mdp": ""}


class FakeChrome:
    """Chrome's DevTools as the bridge uses them for a login: the page's frames (`frames`, the main one first), the
    login script's calls answered by name, the checks on a field (`Runtime.callFunctionOn`), and what was typed where
    (`typed`: (frame url, field, text))."""

    def __init__(self, url=PAGE_A):
        self.frames = [Frame(url)]
        self.port = free_port()
        self.focused = None         # (context, field)
        self.typed = []
        self.loop = asyncio.new_event_loop()
        self.ready = threading.Event()
        threading.Thread(target=self._run, daemon=True).start()
        self.ready.wait(5)

    def frame(self, context):
        return next(f for f in self.frames if f.context == context)

    def _run(self):
        asyncio.set_event_loop(self.loop)

        async def main():
            self.done = asyncio.get_running_loop().create_future()
            async with serve(self._session, "127.0.0.1", self.port, process_request=self._http):
                self.ready.set()
                await self.done

        self.loop.run_until_complete(main())

    def _http(self, connection, request):
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

    @staticmethod
    def origin_of(url):
        scheme, rest = url.split("://", 1)
        return f"{scheme}://{rest.split('/', 1)[0]}"

    def _answer(self, method, params):
        if method == "Target.getTargets":
            return {"targetInfos": [{"targetId": "T1", "type": "page", "url": self.frames[0].url}]}
        if method == "Target.attachToTarget":
            return {"sessionId": "S-page"}
        if method == "Page.getFrameTree":
            main, *children = self.frames
            tree = {"frame": {"id": f"F{main.context}", "url": main.url}}
            if children:
                tree["childFrames"] = [{"frame": {"id": f"F{c.context}", "url": c.url}} for c in children]
            return {"frameTree": tree}
        if method == "Page.createIsolatedWorld":
            return {"executionContextId": int(params["frameId"][1:])}
        if method == "DOM.focus":
            context, field = params["objectId"].split("-")[1:]
            self.focused = (int(context), field)
            return {}
        if method == "Input.insertText":
            context, field = self.focused
            frame = self.frame(context)
            frame.values[field] = params["text"]
            self.typed.append((frame.url, field, params["text"]))
            return {}
        if method == "Runtime.callFunctionOn":
            context, field = params["objectId"].split("-")[1:]
            context = int(context)
            frame = self.frame(context)
            function = params["functionDeclaration"]
            args = [a.get("value") for a in params.get("arguments") or []]
            arg = args[0] if args else None
            password = field == "mdp"
            if "p.setUser(" in function:
                value = not password and args[1:] == [self.origin_of(frame.url)]
                if value:
                    frame.values[field] = arg
            elif "p.set(" in function:
                value = password and args[1:] == [self.origin_of(frame.url)]
                if value:
                    frame.values[field] = arg
            elif "activeElement" in function:
                kind_ok = password if "=== 'password'" in function else not password
                value = kind_ok and self.focused == (context, field) and arg == self.origin_of(frame.url)
            elif "this.value === v" in function:
                kind_ok = password if "'password'" in function else True
                value = kind_ok and frame.values[field] == arg
            elif "=== 'password'" in function:
                value = password
            else:
                value = not password    # the identifier's kind: a text field
            return {"result": {"type": "boolean", "value": value}}
        if method == "Runtime.evaluate":
            expression = params["expression"]
            if expression == "document.visibilityState":
                return {"result": {"type": "string", "value": "visible"}}
            context = params.get("contextId", 1)
            call = re.search(r"__bipLogin\.(\w+)\((.*)\)\s*$", expression, re.S)
            if not call:
                return {"result": {"type": "undefined"}}
            name, args = call.group(1), call.group(2)
            frame = self.frame(context)
            if name == "detectLogin":
                value = {"focused": False, "password": frame.password, "identifier": frame.identifier, "count": int(frame.password)}
                return {"result": {"type": "object", "value": value}}
            if name == "detect":
                return {"result": {"type": "object", "value": {"focused": False, "count": 0, "fields": []}}}
            if name == "userEl":
                return {"result": {"type": "object", "objectId": f"obj-{context}-login"}}
            if name == "el":
                return {"result": {"type": "object", "objectId": f"obj-{context}-mdp"}}
            if name == "prepareUser":
                wanted = json.loads(args)
                self.focused = (context, "login")
                if frame.values["login"] == wanted:
                    return {"result": {"type": "boolean", "value": False}}
                frame.values["login"] = ""
                return {"result": {"type": "boolean", "value": True}}
            if name == "prepare":
                self.focused = (context, "mdp")
                frame.values["mdp"] = ""
                return {"result": {"type": "boolean", "value": frame.password}}
            if name == "set":
                frame.values["mdp"] = ""
            return {"result": {"type": "boolean", "value": True}}
        return {}

    def stop(self):
        self.loop.call_soon_threadsafe(lambda: self.done.done() or self.done.set_result(None))


def login_bridge(bridges, fakes, chrome):
    bridge = bridges.new()
    bridge.config["agents"]["Vie"] = {"hermes_url": fakes.down.url, "hermes_key": "k" * 32, "isolated": True,
                                      "python": f"{bridge.fakebin}/podman exec -i hermes-vie python",
                                      "hermes_home": "/home/hermes/.hermes"}
    bridge.config["hermes"] = {"isolated": True, "bin": str(bridge.fakebin / "hermes"), "hub_url": fakes.hermes.url}
    folder = bridge.data / "browsers" / "vie"
    folder.mkdir(parents=True)
    (folder / "ports.json").write_text(json.dumps({"cdp": chrome.port, "vnc": chrome.port + 1}))
    bridge.commands.rule("podman", ["exec", "-i", "hermes-vie"], stdout=ANSWER)
    bridge.start()
    give_token(bridge, "vie", TOKEN)
    return bridge


def keep(bridge, label, site, identifier, password, agents=("vie",)):
    body = {"label": label, "site": site, "identifier": identifier, "password": password, "agents": list(agents)}
    made = bridge.post("/v1/vault", headers=AUTH, json=body)
    assert made.status_code == 201, made.text
    return made.json()


def vault_login(bridge, arguments=None):
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "vault_login", "arguments": arguments or {}}}
    resp = bridge.post(VAULT, json=body, headers={"Authorization": f"Bearer {TOKEN}"}, timeout=120)
    return resp.json()["result"]


def said(result):
    return result["content"][0]["text"]


def nowhere_but_the_page(bridge, results, *secrets):
    """Neither the identifiers nor the passwords in any answer, in the bridge's log, or in what the app lists apart from
    the identifier it shows the person."""
    log = (bridge.root / "bridge.log").read_text()
    for secret in secrets:
        assert secret not in log, secret
        for result in results:
            assert secret not in json.dumps(result), secret


def test_one_login_fills_both_fields_of_the_extranet_page(bridges, fakes):
    chrome = FakeChrome(PAGE_A)
    try:
        bridge = login_bridge(bridges, fakes, chrome)
        item = keep(bridge, "Extranet agence A", PAGE_A, IDENTIFIER_A, PASSWORD_A)
        # kept with its login page, the site its origin (the app shows both)
        assert item["origin"] == SITE and item["login_url"] == PAGE_A
        result = vault_login(bridge)
        assert result["isError"] is False, result
        assert said(result).startswith("Typed the identifier and the password of « Extranet agence A » into the login "
                                       "form of extranet2.example.fr.")
        assert "submit the form yourself" in said(result)
        page = chrome.frames[0]
        assert page.values == {"login": IDENTIFIER_A, "mdp": PASSWORD_A}
        assert chrome.typed == [(PAGE_A, "login", IDENTIFIER_A), (PAGE_A, "mdp", PASSWORD_A)]
        assert "saved login typed into the agent's page" in (bridge.root / "bridge.log").read_text()
        nowhere_but_the_page(bridge, [result], IDENTIFIER_A, PASSWORD_A)
        # the identifier already in its field: left as it is, the password typed again
        chrome.typed.clear()
        again = vault_login(bridge)
        assert again["isError"] is False and chrome.typed == [(PAGE_A, "mdp", PASSWORD_A)]
    finally:
        chrome.stop()


def test_two_agencies_on_one_extranet_are_chosen_by_their_login_page_and_never_guessed(bridges, fakes):
    chrome = FakeChrome(PAGE_B)
    try:
        bridge = login_bridge(bridges, fakes, chrome)
        a = keep(bridge, "Extranet agence A", PAGE_A, IDENTIFIER_A, PASSWORD_A)
        b = keep(bridge, "Extranet agence B", PAGE_B, IDENTIFIER_B, PASSWORD_B)
        on_b = vault_login(bridge)
        assert on_b["isError"] is False and "« Extranet agence B »" in said(on_b)
        assert chrome.frames[0].values == {"login": IDENTIFIER_B, "mdp": PASSWORD_B}
        # a page of the extranet that is neither's login page: both named, nothing typed
        chrome.frames = [Frame(f"{SITE}/V5/accueil.html")]
        chrome.typed.clear()
        several = vault_login(bridge)
        assert several["isError"] is True
        text = said(several)
        assert "Several logins of the vault fit extranet2.example.fr" in text and "Nothing was typed" in text
        assert f"« Extranet agence A » (id {a['id']}, login page {PAGE_A})" in text
        assert f"« Extranet agence B » (id {b['id']}, login page {PAGE_B})" in text
        assert "Never pick one yourself" in text and chrome.typed == []
        # the agent names the one the person means
        named = vault_login(bridge, {"id": a["id"]})
        assert named["isError"] is False and "« Extranet agence A »" in said(named)
        assert chrome.frames[0].values == {"login": IDENTIFIER_A, "mdp": PASSWORD_A}
        nowhere_but_the_page(bridge, [on_b, several, named], IDENTIFIER_A, IDENTIFIER_B, PASSWORD_A, PASSWORD_B)
    finally:
        chrome.stop()


def test_a_login_fills_on_its_subdomains_never_on_another_site(bridges, fakes):
    chrome = FakeChrome("https://www.amazon.fr/ap/signin?openid.return_to=x")
    try:
        bridge = login_bridge(bridges, fakes, chrome)
        shop = keep(bridge, "Amazon", "https://www.amazon.fr", "client@example.org", "Fake-shop-pw-1!")
        assert shop["login_url"] is None
        assert vault_login(bridge)["isError"] is False
        assert chrome.frames[0].values == {"login": "client@example.org", "mdp": "Fake-shop-pw-1!"}
        # another subdomain of amazon.fr: the same login
        chrome.frames = [Frame("https://smile.amazon.fr/ap/signin")]
        assert vault_login(bridge)["isError"] is False and chrome.frames[0].values["mdp"] == "Fake-shop-pw-1!"
        # look-alikes, another country's site, plain http: nothing typed
        for url in ["https://www.amazon.fr.evil.example/ap/signin", "https://www.amazon.com/ap/signin",
                    "https://amaz0n.fr/ap/signin", "http://www.amazon.fr/ap/signin"]:
            chrome.frames = [Frame(url)]
            chrome.typed.clear()
            refused = vault_login(bridge)
            assert refused["isError"] is True and "gives you no login for" in said(refused), url
            assert chrome.typed == [], url
        # a login asked for by id on another site's page: refused
        chrome.frames = [Frame("https://www.amazon.com/ap/signin")]
        elsewhere = vault_login(bridge, {"id": shop["id"]})
        assert elsewhere["isError"] is True and "not for www.amazon.com" in said(elsewhere) and chrome.typed == []
    finally:
        chrome.stop()


def test_private_suffixes_keep_two_tenants_apart(bridges, fakes):
    chrome = FakeChrome("https://bob-shop.myshopify.com/account/login")
    try:
        bridge = login_bridge(bridges, fakes, chrome)
        keep(bridge, "Boutique d'Alice", "https://alice-shop.myshopify.com/account/login", "alice@example.org", "Fake-pw-9!")
        refused = vault_login(bridge)
        assert refused["isError"] is True and "gives you no login for bob-shop.myshopify.com" in said(refused)
        assert chrome.typed == []
    finally:
        chrome.stop()


def test_a_frame_of_another_site_is_never_written_into(bridges, fakes):
    chrome = FakeChrome(PAGE_A)
    try:
        bridge = login_bridge(bridges, fakes, chrome)
        keep(bridge, "Extranet agence A", PAGE_A, IDENTIFIER_A, PASSWORD_A)
        # the page itself has no form; a frame of another site has one: never looked at
        chrome.frames[0].identifier = chrome.frames[0].password = False
        chrome.frames.append(Frame("https://ads.thirdparty.example/login-widget", context=2))
        refused = vault_login(bridge)
        assert refused["isError"] is True and "No login form of extranet2.example.fr" in said(refused)
        assert chrome.typed == [] and chrome.frames[1].values == {"login": "", "mdp": ""}
        # a frame of the extranet's own domain (another host of it) is the site's own: typed there
        chrome.frames[1] = Frame("https://auth.example.fr/form", context=2)
        done = vault_login(bridge)
        assert done["isError"] is False, done
        assert chrome.typed == [("https://auth.example.fr/form", "login", IDENTIFIER_A),
                                ("https://auth.example.fr/form", "mdp", PASSWORD_A)]
    finally:
        chrome.stop()


def test_a_first_step_asking_for_the_identifier_alone(bridges, fakes):
    chrome = FakeChrome("https://www.amazon.fr/ap/signin")
    try:
        bridge = login_bridge(bridges, fakes, chrome)
        keep(bridge, "Amazon", "https://www.amazon.fr", "client@example.org", "Fake-shop-pw-1!")
        chrome.frames[0].password = False
        first = vault_login(bridge)
        assert first["isError"] is False
        assert said(first).startswith("Typed the identifier of « Amazon » on www.amazon.fr.")
        assert "call vault_login again" in said(first)
        assert chrome.typed == [("https://www.amazon.fr/ap/signin", "login", "client@example.org")]
        nowhere_but_the_page(bridge, [first], "client@example.org", "Fake-shop-pw-1!")
    finally:
        chrome.stop()


def test_a_numeric_client_code_typed_by_the_vault_is_left_alone_by_the_card_checks(bridges, fakes):
    """A client code that looks like a card's number (16 digits passing Luhn): the vault types it, and writes it down
    for the browser's card checks (only its hash, for that frame's origin), which then leave that very value alone in
    that origin's fields. Never written down for a code that isn't card-like, nor typed by anyone else."""
    code = "4970101234567893"   # a Luhn-valid 16 digits, as some extranets give their clients
    chrome = FakeChrome(PAGE_A)
    try:
        bridge = login_bridge(bridges, fakes, chrome)
        keep(bridge, "Extranet agence A", PAGE_A, code, PASSWORD_A)
        typed_file = bridge.data / "browsers" / "vie" / "vault-typed.json"
        result = vault_login(bridge)
        assert result["isError"] is False and chrome.frames[0].values["login"] == code
        [row] = json.loads(typed_file.read_text())
        assert row["o"] == SITE and code not in typed_file.read_text()
        assert re.fullmatch(r"[0-9a-f]{1,8}", row["h"])
        nowhere_but_the_page(bridge, [result], code, PASSWORD_A)
        # an e-mail identifier: nothing written down
        typed_file.unlink()
        keep(bridge, "Extranet agence B", PAGE_B, IDENTIFIER_B, PASSWORD_B)
        chrome.frames = [Frame(PAGE_B)]
        assert vault_login(bridge)["isError"] is False and not typed_file.exists()
    finally:
        chrome.stop()


def test_no_login_no_browser_no_page(bridges, fakes):
    chrome = FakeChrome("https://www.example.com/login")
    try:
        bridge = login_bridge(bridges, fakes, chrome)
        none = vault_login(bridge)
        assert none["isError"] is True and "ask_secret" in said(none) and chrome.typed == []
        unknown = vault_login(bridge, {"id": "item_000000000000"})
        assert unknown["isError"] is True and "No login of yours has this id" in said(unknown)
    finally:
        chrome.stop()
