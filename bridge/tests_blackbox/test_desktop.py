"""The agents' desktop in the app (`/v1/agents/{id}/desktop`, `/desktop/<ticket>/…`): a ticket, the noVNC page, the
RFB stream relayed from Hermes' Unix socket with the person's input dropped until they take over (Hermes' lease), and
a VNC server set up by hand that the bridge logs in to itself (tests/test_desktop.py, over HTTP and WebSocket; the
desktops are small fake RFB servers)."""
import json
import os
import shutil
import socket
import struct
import tempfile
import threading
from pathlib import Path

import pytest

from bbkit.common import AUTH
from bbkit.servers import wait_until
from bbkit.ws import InvalidStatus, open_ws
from conftest import managed_install, profile

HANDSHAKE = b"RFB 003.008\n" + b"\x01" + b"\x00"          # version, security None, ClientInit not shared
UPDATE = b"\x03" + b"\x00" + struct.pack(">HHHH", 0, 0, 100, 100)
KEY = b"\x04\x01\x00\x00" + struct.pack(">I", 0x61)
POINTER = b"\x05\x01" + struct.pack(">HH", 10, 10)
ENCODINGS = b"\x02\x00" + struct.pack(">H", 2) + struct.pack(">ii", 0, -223)
CHALLENGE = bytes(range(16))
RESPONSE = bytes.fromhex("49c26463a09638c7a136d28d2d0d147e")   # desktop.vnc_response("MotVNC-42", CHALLENGE)


class FakeDesktop:
    """An RFB server: says its version, then records what it gets. `password`: VNC Authentication first."""

    def __init__(self, family, address, password=False):
        self.sock = socket.socket(family, socket.SOCK_STREAM)
        self.sock.bind(address)
        self.sock.listen(4)
        self.password = password
        self.received = bytearray()
        self.logged_in = None
        self.connections = 0
        threading.Thread(target=self.serve, daemon=True).start()

    def serve(self):
        while True:
            try:
                conn, _ = self.sock.accept()
            except OSError:
                return
            self.connections += 1
            threading.Thread(target=self.talk, args=(conn,), daemon=True).start()

    def recv_exactly(self, conn, n):
        data = b""
        while len(data) < n:
            chunk = conn.recv(n - len(data))
            if not chunk:
                raise ConnectionError
            data += chunk
        return data

    def talk(self, conn):
        try:
            conn.sendall(b"RFB 003.008\n")
            if self.password:
                self.received += self.recv_exactly(conn, 12)
                conn.sendall(b"\x01\x02")                      # one type: VNC Authentication
                self.received += self.recv_exactly(conn, 1)
                conn.sendall(CHALLENGE)
                self.logged_in = self.recv_exactly(conn, 16) == RESPONSE
                conn.sendall(b"\x00\x00\x00\x00" if self.logged_in else b"\x00\x00\x00\x01")
            while True:
                chunk = conn.recv(65536)
                if not chunk:
                    return
                self.received += chunk
                conn.sendall(b"frame")
        except (OSError, ConnectionError):
            return
        finally:
            conn.close()

    def close(self):
        self.sock.close()


def recv_bytes(ws, n, timeout=10):
    data = b""
    while len(data) < n:
        message = ws.recv(timeout=timeout)
        data += message if isinstance(message, bytes) else message.encode()
    return data


@pytest.fixture
def hermes_desktop(bridges, fakes):
    """Coach, an agent of the hub, with Hermes' Bot Desktop running (its RFB socket in its profile)."""
    bridge = bridges.new()
    managed_install(bridge, fakes, agents=("coach",))
    # A Unix socket's path must stay short (104 bytes on macOS): this HERMES_HOME lives in a folder of its own in /tmp.
    home = Path(tempfile.mkdtemp(prefix="bb-", dir="/tmp"))
    bridge.config["hermes"]["home"] = str(home)
    folder = profile(home, "coach") / "bot-desktop"
    folder.mkdir()
    desktop = FakeDesktop(socket.AF_UNIX, str(folder / "rfb.sock"))
    bridge.start()
    yield bridge, folder, desktop
    desktop.close()
    shutil.rmtree(home, ignore_errors=True)


def test_watching_sends_no_input_and_taking_over_does(hermes_desktop):
    bridge, folder, desktop = hermes_desktop
    opened = bridge.post("/v1/agents/coach/desktop", headers=AUTH)
    assert opened.status_code == 200, opened.text
    data = opened.json()
    assert data["kind"] == "novnc" and data["path"].startswith("desktop/") and data["path"].endswith("/")
    assert {k: data[k] for k in ("you_hold", "holder", "reason", "pauses_agent")} == {
        "you_hold": False, "holder": "agent", "reason": None, "pauses_agent": True}
    page = bridge.get("/" + data["path"])
    assert page.status_code == 200 and page.headers["content-type"].startswith("text/html")
    assert page.headers["cache-control"] == "no-store" and "rfb.viewOnly = true;" in page.text
    with open_ws(bridge.ws_url("/" + data["path"] + "ws"), subprotocols=["binary"]) as ws:
        assert ws.subprotocol == "binary"                 # noVNC asks for it
        assert recv_bytes(ws, 12) == b"RFB 003.008\n"
        ws.send(HANDSHAKE)
        assert wait_until(lambda: bytes(desktop.received).startswith(b"RFB 003.008\n\x01\x01"))   # always shared
        ws.send(UPDATE + KEY + POINTER + ENCODINGS)
        assert wait_until(lambda: bytes(desktop.received).endswith(ENCODINGS))
        assert bytes(desktop.received) == b"RFB 003.008\n\x01\x01" + UPDATE + ENCODINGS            # no input
        assert recv_bytes(ws, 5) == b"frame"
        assert (folder / "activity").exists()          # watched: Hermes keeps the desktop up
        taken = bridge.post("/v1/agents/coach/desktop/control", headers=AUTH, json={"take": True})
        assert taken.json() == {"holder": "human", "you_hold": True}
        lease = json.loads((folder / "lease.json").read_text())
        assert {k: lease[k] for k in ("holder", "viewer_id", "reason", "epoch")} == {
            "holder": "human", "viewer_id": "bipagents-app", "reason": "Depuis l'app BipAgents", "epoch": 1}
        assert oct(os.stat(folder / "lease.json").st_mode & 0o777) == "0o600"
        ws.send(KEY[:3])
        ws.send(KEY[3:] + POINTER)                      # split across frames
        assert wait_until(lambda: bytes(desktop.received).endswith(KEY + POINTER))
    again = bridge.post("/v1/agents/coach/desktop", headers=AUTH).json()
    assert again["you_hold"] is True and again["holder"] == "human" and again["reason"] == "Depuis l'app BipAgents"
    assert "rfb.viewOnly = false;" in bridge.get("/" + again["path"]).text      # the person holds it
    back = bridge.post("/v1/agents/coach/desktop/control", headers=AUTH, json={"take": False})
    assert back.json() == {"holder": "agent", "you_hold": False}
    assert json.loads((folder / "lease.json").read_text())["epoch"] == 2


def test_routes(bridges, fakes):
    bridge = bridges.new()
    managed_install(bridge, fakes, agents=("coach",))
    bridge.config["agents"]["Vue"] = {"hermes_url": fakes.down.url, "hermes_key": "v" * 32,
                                      "desktop_url": "https://vue.tailnet.ts.net/vnc.html"}
    bridge.start()
    hand = bridge.post("/v1/agents/wellness/desktop", headers=AUTH)   # set up by hand, no viewer page
    assert hand.status_code == 409 and hand.json() == {"detail": {"code": "no_desktop"}}
    assert bridge.post("/v1/agents/coach/desktop", headers=AUTH).status_code == 409   # its desktop is not running
    assert bridge.post("/v1/agents/vue/desktop", headers=AUTH).json() == {"kind": "page",
                                                                           "url": "https://vue.tailnet.ts.net/vnc.html"}
    nobody = bridge.post("/v1/agents/nobody/desktop", headers=AUTH)
    assert nobody.status_code == 404 and nobody.json() == {"detail": "unknown agent"}
    assert bridge.post("/v1/agents/wellness/desktop").status_code == 401
    assert bridge.post("/v1/agents/nobody/desktop/control", headers=AUTH, json={"take": True}).status_code == 404
    expired = bridge.get("/desktop/nope/")
    assert expired.status_code == 404 and expired.json() == {"detail": "expired"}
    with pytest.raises(InvalidStatus) as refused:
        with open_ws(bridge.ws_url("/desktop/nope/ws")):
            pass
    assert refused.value.response.status_code == 403
    novnc = bridge.get("/desktop/static/novnc/core/rfb.js")
    assert novnc.status_code == 200 and "javascript" in novnc.headers["content-type"]


def test_the_bridge_logs_in_to_a_password_protected_vnc(bridges, fakes):
    desktop = FakeDesktop(socket.AF_INET, ("127.0.0.1", 0), password=True)
    port = desktop.sock.getsockname()[1]
    bridge = bridges.new()
    password = bridge.root / "vnc-pass.txt"
    password.write_text("MotVNC-42\n")
    bridge.config["agents"]["Vie"] = {"hermes_url": fakes.down.url, "hermes_key": "v" * 32,
                                      "desktop_vnc": f"tcp://127.0.0.1:{port}", "desktop_vnc_password_file": str(password)}
    bridge.start()
    data = bridge.post("/v1/agents/vie/desktop", headers=AUTH).json()
    assert {k: data[k] for k in ("kind", "you_hold", "holder", "reason", "pauses_agent")} == {
        "kind": "novnc", "you_hold": False, "holder": "agent", "reason": None, "pauses_agent": False}
    with open_ws(bridge.ws_url("/" + data["path"] + "ws")) as ws:
        # To the app: the bridge's own handshake, no password (the app passed Face ID and holds a ticket).
        assert recv_bytes(ws, 12) == b"RFB 003.008\n"
        ws.send(b"RFB 003.008\n")
        assert recv_bytes(ws, 2) == b"\x01\x01"
        ws.send(b"\x01")
        assert recv_bytes(ws, 4) == b"\x00\x00\x00\x00"
        ws.send(b"\x00")                                                   # ClientInit, not shared
        assert wait_until(lambda: desktop.logged_in is True)
        assert wait_until(lambda: bytes(desktop.received) == b"RFB 003.008\n\x02\x01")   # shared all the same
        ws.send(KEY + UPDATE)
        assert wait_until(lambda: bytes(desktop.received).endswith(UPDATE))
        assert KEY not in bytes(desktop.received)
        assert bridge.post("/v1/agents/vie/desktop/control", headers=AUTH, json={"take": True}).json() == {
            "holder": "human", "you_hold": True}
        ws.send(KEY)
        assert wait_until(lambda: bytes(desktop.received).endswith(KEY))
    assert bridge.post("/v1/agents/vie/desktop", headers=AUTH).json()["you_hold"] is True
    desktop.close()


def vnc_bridge(bridges, fakes, password_bytes):
    desktop = FakeDesktop(socket.AF_INET, ("127.0.0.1", 0), password=True)
    port = desktop.sock.getsockname()[1]
    bridge = bridges.new()
    password = bridge.root / "passwd"
    password.write_bytes(password_bytes)
    bridge.config["agents"]["Vie"] = {"hermes_url": fakes.down.url, "hermes_key": "v" * 32,
                                      "desktop_vnc": f"tcp://127.0.0.1:{port}", "desktop_vnc_password_file": str(password)}
    return bridge.start(), desktop


def test_the_vnc_password_file_may_be_vncpasswd(bridges, fakes):
    # Written by TigerVNC's own `vncpasswd` for « MotVNC-42 » (VNC keeps the first 8 characters).
    bridge, desktop = vnc_bridge(bridges, fakes, bytes.fromhex("5bb576d0733bc57b"))
    data = bridge.post("/v1/agents/vie/desktop", headers=AUTH).json()
    with open_ws(bridge.ws_url("/" + data["path"] + "ws")) as ws:
        assert recv_bytes(ws, 12) == b"RFB 003.008\n"
    assert wait_until(lambda: desktop.logged_in is True)
    desktop.close()


def test_a_wrong_vnc_password_closes_the_viewer(bridges, fakes):
    bridge, desktop = vnc_bridge(bridges, fakes, b"Mauvais\n")
    data = bridge.post("/v1/agents/vie/desktop", headers=AUTH).json()
    with pytest.raises(InvalidStatus) as refused:
        with open_ws(bridge.ws_url("/" + data["path"] + "ws")):
            pass
    assert refused.value.response.status_code == 403     # closed (4001) before the handshake
    assert desktop.logged_in is False
    assert bridge.logged("desktop unavailable")[0]["agent"] == "vie"
    desktop.close()


def test_the_agents_own_browser_is_woken_and_held(bridges, fakes):
    """An agent with its own browser container (browsers.py: `<browser root>/<agent>/ports.json`): opening its desktop
    wakes the container (started by its unit when its screen is not up), taking over makes its tools wait."""
    desktop = FakeDesktop(socket.AF_INET, ("127.0.0.1", 0))
    port = desktop.sock.getsockname()[1]
    bridge = bridges.new()
    bridge.config["agents"]["Vie"] = {"hermes_url": fakes.down.url, "hermes_key": "v" * 32,
                                      "desktop_vnc": f"tcp://127.0.0.1:{port}"}
    folder = bridge.data / "browsers" / "vie"
    folder.mkdir(parents=True)
    (folder / "ports.json").write_text('{"devtools": 9600, "vnc": 9601}')
    bridge.commands.rule("podman", ["exec", "bipagents-browser-vie", "test", "-e", "/tmp/.X11-unix/X99"], exit=1, times=1)
    bridge.start()
    data = bridge.post("/v1/agents/vie/desktop", headers=AUTH).json()
    assert data["pauses_agent"] is True and data["holder"] == "agent"
    assert wait_until(lambda: ["--user", "start", "bipagents-browser-vie"] in bridge.commands.of("systemctl"))
    assert (folder / "busy").exists()                      # used now: not stopped for `idle_minutes`
    taken = bridge.post("/v1/agents/vie/desktop/control", headers=AUTH, json={"take": True})
    assert taken.json() == {"holder": "human", "you_hold": True}
    assert ["exec", "bipagents-browser-vie", "sh", "-c", "touch /tmp/bipagents-hold"] in bridge.commands.of("podman")
    bridge.post("/v1/agents/vie/desktop/control", headers=AUTH, json={"take": False})
    assert bridge.commands.of("podman")[-1] == ["exec", "bipagents-browser-vie", "sh", "-c", "rm -f /tmp/bipagents-hold"]
    desktop.close()
