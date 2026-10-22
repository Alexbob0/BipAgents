"""« Bureau » without the black screen (`/v1/agents/{id}/desktop/state`, `…/desktop/open`) and, on a phone, the page
alone (`/desktop/<ticket>/page/…`): Chrome's screencast relayed to the app, the person's taps and keys passed on while
they hold the browser, the tab rendered as on a phone meanwhile and given back as it was. The agent's Chrome is a
fake DevTools server playing the container's relay (its hold lets only the bridge's own token through)."""
import base64
import json
import socket
import threading
import time

import pytest
from websockets.sync.server import serve

from bbkit.common import AUTH
from bbkit.servers import wait_until
from bbkit.ws import open_ws

JPEG = b"\xff\xd8\xff\xe0fake-jpeg"


class FakeChrome:
    """Chrome's DevTools behind the container's relay: `/json/list`, one WebSocket per tab, a screencast frame on
    each start and ack. `hold(token)`: as the person's hold, only connections showing that token get through."""

    def __init__(self, tabs):
        self.tabs = tabs
        self.calls = []                  # (tab, method, params, token shown)
        self.token = None
        self.open = []
        self.lock = threading.Lock()
        self.server = serve(self.talk, "127.0.0.1", 0, process_request=self.http, compression=None, max_size=None)
        self.port = self.server.socket.getsockname()[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def allowed(self, headers):
        return self.token is None or headers.get("x-bip-hold") == self.token

    def http(self, connection, request):
        if not self.allowed(request.headers):
            return connection.respond(403, "held\n")
        if request.path == "/json/list":
            return connection.respond(200, json.dumps([{"id": t["id"], "type": "page", "url": t["url"],
                                                        "title": t["title"]} for t in self.tabs]))
        return None

    def methods(self, name=None):
        with self.lock:
            return [c for c in self.calls if name is None or c[1] == name]

    def talk(self, ws):
        tab = ws.request.path.rsplit("/", 1)[-1]
        shown = ws.request.headers.get("x-bip-hold")
        with self.lock:
            self.open.append((ws, shown))
        try:
            for raw in ws:
                m = json.loads(raw)
                with self.lock:
                    self.calls.append((tab, m["method"], m.get("params") or {}, shown))
                result = {}
                if m["method"] == "Browser.getVersion":
                    result = {"product": "Chrome/131.0.6778.85", "userAgent": "Mozilla/5.0 (X11; Linux x86_64)"}
                if m["method"] == "Page.getNavigationHistory":
                    result = {"currentIndex": 1, "entries": [{"id": 7, "url": "https://a.fr/"}, {"id": 8, "url": "https://b.fr/"}]}
                ws.send(json.dumps({"id": m["id"], "result": result}))
                if m["method"] in ("Page.startScreencast", "Page.screencastFrameAck"):
                    ws.send(json.dumps({"method": "Page.screencastFrame", "params": {
                        "data": base64.b64encode(JPEG).decode(), "sessionId": len(self.calls),
                        "metadata": {"deviceWidth": 1366, "deviceHeight": 600, "offsetTop": 0, "pageScaleFactor": 1}}}))
        except Exception:
            pass

    def hold(self, token):
        """The relay's watch: connections without the token closed."""
        self.token = token
        with self.lock:
            for ws, shown in list(self.open):
                if shown != token:
                    ws.close()

    def close(self):
        self.server.shutdown()


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@pytest.fixture
def chrome():
    fake = FakeChrome([{"id": "TAB1", "url": "https://www.lemonde.fr/economie/", "title": "Le Monde"}])
    yield fake
    fake.close()


def browser_bridge(bridges, fakes, cdp_port):
    bridge = bridges.new()
    bridge.config["agents"]["Vie"] = {"hermes_url": fakes.down.url, "hermes_key": "v" * 32,
                                      "desktop_vnc": f"tcp://127.0.0.1:{free_port()}"}
    folder = bridge.data / "browsers" / "vie"
    folder.mkdir(parents=True)
    (folder / "ports.json").write_text(json.dumps({"cdp": cdp_port, "vnc": free_port()}))
    bridge.start()
    return bridge, folder


def state(bridge):
    answer = bridge.get("/v1/agents/vie/desktop/state", headers=AUTH)
    assert answer.status_code == 200, answer.text
    return answer.json()


def test_the_state_tells_what_the_agents_browser_is_doing(bridges, fakes, chrome):
    bridge, folder = browser_bridge(bridges, fakes, chrome.port)
    assert state(bridge) == {"state": "active", "browser": True, "page": True, "can_open": True,
                             "site": "lemonde.fr", "title": "Le Monde", "you_hold": False}
    chrome.tabs[:] = [{"id": "NEW", "url": "chrome://newtab/", "title": "Nouvel onglet"}]
    assert {k: state(bridge)[k] for k in ("state", "browser", "site")} == {"state": "idle", "browser": True, "site": None}
    chrome.tabs[:] = []
    assert state(bridge)["state"] == "idle"
    assert bridge.commands.of("podman") == [] and bridge.commands.of("systemctl") == []   # asking starts nothing


def test_a_stopped_browser_and_one_starting(bridges, fakes):
    bridge, folder = browser_bridge(bridges, fakes, free_port())
    assert {k: state(bridge)[k] for k in ("state", "browser", "can_open")} == {
        "state": "stopped", "browser": False, "can_open": True}
    (folder / "busy").touch()                          # the agent's browser tools are starting it
    assert state(bridge)["state"] == "starting"


def test_the_person_opens_the_browser(bridges, fakes):
    bridge, folder = browser_bridge(bridges, fakes, free_port())
    opened = bridge.post("/v1/agents/vie/desktop/open", headers=AUTH)
    assert opened.status_code == 200 and opened.json()["state"] == "starting"
    assert wait_until(lambda: ["exec", "-d", "bipagents-browser-vie", "/opt/stealth/open_browser.sh"]
                      in bridge.commands.of("podman"))
    assert state(bridge)["state"] == "starting"        # until Chrome answers (here: never)


def test_routes_for_other_desktops(bridges, fakes):
    bridge = bridges.new()
    bridge.config["agents"]["Vue"] = {"hermes_url": fakes.down.url, "hermes_key": "v" * 32,
                                      "desktop_url": "https://vue.tailnet.ts.net/vnc.html"}
    bridge.start()
    assert bridge.get("/v1/agents/vue/desktop/state", headers=AUTH).json() == {
        "state": "active", "browser": True, "page": False, "can_open": False, "site": None, "title": None,
        "you_hold": False}
    assert bridge.post("/v1/agents/vue/desktop/open", headers=AUTH).json() == {"detail": {"code": "cannot_open"}}
    none = bridge.get("/v1/agents/wellness/desktop/state", headers=AUTH)
    assert none.status_code == 409 and none.json() == {"detail": {"code": "no_desktop"}}
    assert bridge.get("/v1/agents/nobody/desktop/state", headers=AUTH).status_code == 404
    assert bridge.get("/v1/agents/vue/desktop/state").status_code == 401
    assert bridge.get("/desktop/nope/page/").status_code == 404


def recv_until(ws, predicate, limit=40):
    seen = []
    for _ in range(limit):
        message = ws.recv(timeout=10)
        item = message if isinstance(message, bytes) else json.loads(message)
        seen.append(item)
        if predicate(item):
            return seen
    raise AssertionError(f"not seen in {seen!r}")


def test_a_phone_watches_the_page_then_takes_it_over_as_a_phone(bridges, fakes, chrome):
    bridge, folder = browser_bridge(bridges, fakes, chrome.port)
    data = bridge.post("/v1/agents/vie/desktop", headers=AUTH).json()
    assert data["page_path"].startswith("desktop/") and data["page_path"].endswith("/page/")
    page = bridge.get("/" + data["page_path"])
    assert page.status_code == 200 and page.headers["cache-control"] == "no-store" and "setViewOnly" in page.text
    url = bridge.ws_url("/" + data["page_path"] + "ws?mobile=1&w=390&h=700&dpr=3")
    with open_ws(url) as ws:
        seen = recv_until(ws, lambda m: m == JPEG)
        assert {"type": "state", "state": "live", "mobile": False, "holding": False} in seen
        assert {"type": "page", "site": "lemonde.fr", "url": "https://www.lemonde.fr/economie/", "title": "Le Monde"} in seen
        assert {"type": "frame", "w": 1366.0, "h": 600.0} in seen
        start = chrome.methods("Page.startScreencast")[0]
        assert start[0] == "TAB1" and start[2]["format"] == "jpeg" and start[3] is None
        # Watching: nothing the person does reaches the page.
        ws.send(json.dumps({"type": "touch", "phase": "start", "points": [[0.5, 0.5]]}))
        ws.send(json.dumps({"type": "text", "text": "secret"}))
        time.sleep(0.5)
        assert not chrome.methods("Input.dispatchTouchEvent") and not chrome.methods("Input.insertText")
        assert not chrome.methods("Emulation.setDeviceMetricsOverride")

        taken = bridge.post("/v1/agents/vie/desktop/control", headers=AUTH, json={"take": True})
        assert taken.json() == {"holder": "human", "you_hold": True}
        token = (folder / "hold.token").read_text()
        chrome.hold(token)                              # what the container's relay does with the hold's flag
        seen = recv_until(ws, lambda m: isinstance(m, dict) and m.get("mobile") is True)
        assert {"type": "state", "state": "live", "mobile": True, "holding": True} in seen
        metrics = chrome.methods("Emulation.setDeviceMetricsOverride")[-1]
        assert metrics[3] == token                       # through the relay with the hold's token
        assert metrics[2] == {"width": 390, "height": 600, "deviceScaleFactor": 2.0, "mobile": True,
                              "screenWidth": 390, "screenHeight": 600}
        agent = chrome.methods("Emulation.setUserAgentOverride")[-1][2]
        assert "Android" in agent["userAgent"] and "Chrome/131.0.0.0 Mobile" in agent["userAgent"]
        assert agent["userAgentMetadata"]["mobile"] is True
        assert chrome.methods("Emulation.setTouchEmulationEnabled")[-1][2] == {"enabled": True, "maxTouchPoints": 5}
        assert chrome.methods("Page.reload")
        # The person's finger and keys, on the page as on a phone.
        ws.send(json.dumps({"type": "touch", "phase": "start", "points": [[0.5, 0.25]]}))
        ws.send(json.dumps({"type": "touch", "phase": "end"}))
        ws.send(json.dumps({"type": "text", "text": "bonjour"}))
        ws.send(json.dumps({"type": "key", "key": "Enter"}))
        ws.send(json.dumps({"type": "go", "url": "service-public.fr"}))
        ws.send(json.dumps({"type": "back"}))
        assert wait_until(lambda: len(chrome.methods("Page.navigateToHistoryEntry")) == 1)
        touches = [c[2] for c in chrome.methods("Input.dispatchTouchEvent")]
        assert touches[0]["type"] == "touchStart" and touches[1] == {"type": "touchEnd", "touchPoints": []}
        # fractions of the picture, in the CSS pixels of the last frame (this fake's are always 1366 × 600)
        assert touches[0]["touchPoints"] == [{"x": 683.0, "y": 150.0, "id": 0}]
        assert chrome.methods("Input.insertText")[-1][2] == {"text": "bonjour"}
        keys = [c[2] for c in chrome.methods("Input.dispatchKeyEvent")]
        assert keys[0]["type"] == "keyDown" and keys[0]["key"] == "Enter" and keys[0]["text"] == "\r"
        assert keys[1]["type"] == "keyUp"
        assert chrome.methods("Page.navigate")[-1][2] == {"url": "https://service-public.fr/"}
        assert chrome.methods("Page.navigateToHistoryEntry")[-1][2] == {"entryId": 7}

        # Handing back: the tab back to desktop rendering and reloaded before the hold is released.
        reloads = len(chrome.methods("Page.reload"))
        back = bridge.post("/v1/agents/vie/desktop/control", headers=AUTH, json={"take": False})
        assert back.json() == {"holder": "agent", "you_hold": False}
        assert chrome.methods("Emulation.clearDeviceMetricsOverride")
        assert chrome.methods("Emulation.setTouchEmulationEnabled")[-1][2] == {"enabled": False}
        assert len(chrome.methods("Page.reload")) == reloads + 1
        assert bridge.commands.of("podman")[-1] == ["exec", "bipagents-browser-vie", "sh", "-c", "rm -f /tmp/bipagents-hold"]
        assert not (folder / "hold.token").exists()
        chrome.hold(None)
        seen = recv_until(ws, lambda m: isinstance(m, dict) and m.get("state") == "live" and m.get("holding") is False)
        before = len(chrome.methods("Input.insertText"))
        ws.send(json.dumps({"type": "text", "text": "after"}))
        time.sleep(0.5)
        assert len(chrome.methods("Input.insertText")) == before    # the agent's again: nothing passes


def test_another_tab_in_front_is_shown_instead(bridges, fakes, chrome):
    bridge, folder = browser_bridge(bridges, fakes, chrome.port)
    data = bridge.post("/v1/agents/vie/desktop", headers=AUTH).json()
    with open_ws(bridge.ws_url("/" + data["page_path"] + "ws")) as ws:
        recv_until(ws, lambda m: m == JPEG)
        chrome.tabs.insert(0, {"id": "TAB2", "url": "https://www.service-public.fr/", "title": "Service public"})
        recv_until(ws, lambda m: isinstance(m, dict) and m.get("site") == "service-public.fr")
        assert wait_until(lambda: any(c[0] == "TAB2" for c in chrome.methods("Page.startScreencast")))
