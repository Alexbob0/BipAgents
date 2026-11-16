"""Security review of the family relay (branch family-relay-anywhere): the scanning bridge's « direct first » step.

`POST /v1/family/join` first sends the invitation's code, a fresh key for the link and its half of the link's key
exchange, in clear, to the address the invitation names (peers.rs `join`, `direct`), and takes whatever answers there
as the inviter. For two homes on different networks (the case this branch is for), that address is a private one
(`http://192.168.1.10:8642`, `http://lea-pc.local:8642`, a tailnet address…): on the scanner's own network it is
some other device, or nothing. Anything there that answers (a device at that IP, any device answering the mDNS name,
a resolver that answers unknown names) gets the code and the key, and is linked to as the inviter.

Here the scanner's own network is played by an HTTP proxy (`HTTP_PROXY` for the scanning bridge only, 127.0.0.1 not
proxied): the address in Léa's invitation reaches « a device » the test controls. Each test fails on dd94437a8."""
import json
import socket
import threading

import pytest

from bbkit.common import AUTH
from bbkit.kerfy_relay import KerfyRelay
from test_family_relay_anywhere import echo, named


class Device:
    """A device at the inviter's home address on the scanner's network: what it is sent kept, one fixed answer."""

    def __init__(self, status=404, body=b'{"detail":"Not Found"}'):
        self.received = []
        self.status, self.body = status, body
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen(16)
        self.port = self.sock.getsockname()[1]
        threading.Thread(target=self.serve, daemon=True).start()

    def serve(self):
        while True:
            try:
                conn, _ = self.sock.accept()
            except OSError:
                return
            threading.Thread(target=self.handle, args=(conn,), daemon=True).start()

    def handle(self, conn):
        with conn:
            conn.settimeout(10)
            data = b""
            while b"\r\n\r\n" not in data:
                chunk = conn.recv(65536)
                if not chunk:
                    return
                data += chunk
            head, _, rest = data.partition(b"\r\n\r\n")
            length = 0
            for line in head.split(b"\r\n")[1:]:
                name, _, value = line.partition(b":")
                if name.strip().lower() == b"content-length":
                    length = int(value.strip())
            while len(rest) < length:
                chunk = conn.recv(65536)
                if not chunk:
                    break
                rest += chunk
            self.received.append(head + b"\r\n\r\n" + rest)
            reason = {200: "OK", 404: "Not Found"}.get(self.status, "Other")
            conn.sendall(f"HTTP/1.1 {self.status} {reason}\r\nContent-Type: application/json\r\n"
                         f"Content-Length: {len(self.body)}\r\nConnection: close\r\n\r\n".encode() + self.body)

    def stop(self):
        self.sock.close()


@pytest.fixture
def relay(tmp_path):
    started = KerfyRelay(tmp_path).start()
    yield started
    started.stop()


def home(bridges, fakes, name, agents, relay_url, device=None):
    def change(c):
        c["agents"] = {a.title(): {"hermes_url": f"{fakes.hermes.url}/p/{a}", "hermes_key": f"{a}-key-" + "x" * 24}
                       for a in agents}
        c["family"] = {"relay_url": relay_url}
    bridge = bridges.new(name, change=change)
    bridge.config["public_url"] = f"http://{name}.home.arpa:8642"
    if device is not None:
        # this bridge's own network: the other home's address reaches `device` there
        bridge.env_extra.update({"HTTP_PROXY": f"http://127.0.0.1:{device.port}", "NO_PROXY": "127.0.0.1,localhost"})
    return bridge.start()


def households(bridge):
    try:
        return json.loads((bridge.state_dir / "households.json").read_text()).get("households", {})
    except (OSError, ValueError):
        return {}


def homes(bridges, fakes, relay, device):
    fakes.hermes.override = echo
    alex = home(bridges, fakes, "alex", ["vie"], relay.url, device=device)
    lea = home(bridges, fakes, "lea", ["cuisine"], relay.url)
    named(alex, "Alex")
    named(lea, "Léa")
    invitation = lea.post("/v1/family/invite", headers=AUTH).json()
    assert len(invitation.get("relay", "")) == 22, invitation
    return alex, lea, invitation


def join(alex, invitation):
    return alex.post("/v1/family/join", headers=AUTH, json={
        "url": invitation["url"], "code": invitation["code"], "relay": invitation["relay"]})


def test_the_invitations_code_never_goes_in_clear_to_whatever_is_at_a_home_address(bridges, fakes, relay):
    device = Device(status=404)
    try:
        alex, lea, invitation = homes(bridges, fakes, relay, device)
        join(alex, invitation)
        sent = b"".join(device.received)
        assert invitation["code"].encode() not in sent, (
            f"Léa's invitation's code (and Alex's key for the link) went in clear to a device at {invitation['url']} "
            f"on Alex's own network: {sent[:600]!r}")
    finally:
        device.stop()


def test_something_answering_at_that_address_does_not_stop_the_link_through_the_relay(bridges, fakes, relay):
    device = Device(status=404)   # a NAS, a printer, another household's own bridge at the same IP and port
    try:
        alex, lea, invitation = homes(bridges, fakes, relay, device)
        joined = join(alex, invitation)
        assert joined.status_code == 200, (
            f"the link failed ({joined.status_code} {joined.text}) because something on Alex's network answered at "
            f"{invitation['url']}, when Léa's bridge was there all along through the relay")
        assert households(alex)["lea"].get("via") == "relay", households(alex)
    finally:
        device.stop()


def test_a_device_answering_as_the_inviter_is_not_linked_to(bridges, fakes, relay):
    device = Device(status=200, body=json.dumps({"name": "Léa", "key": "k" * 43}).encode())
    try:
        alex, lea, invitation = homes(bridges, fakes, relay, device)
        joined = join(alex, invitation)
        assert joined.status_code != 200 or "alex" in households(lea), (
            f"Alex's bridge is linked to « Léa » ({households(alex)}) but Léa's bridge never heard of it: whatever "
            f"answered at {invitation['url']} on Alex's network took her place, without ever proving it held her "
            f"invitation; Alex's family calls to Léa now go to it")
    finally:
        device.stop()
