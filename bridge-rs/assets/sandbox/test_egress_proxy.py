"""The sandboxes' proxy (egress_proxy.py): registries for all, an agent's own sites with its credentials, refusals
reported, never an address of this machine's networks. Run: `python3 -m pytest -q bridge-rs/assets/sandbox`."""
import asyncio
import base64
import hashlib
import importlib.util
import json
import os
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).parent


def load(tmp_path, monkeypatch, agents=None):
    policy, reports = tmp_path / "sites.json", tmp_path / "blocked.jsonl"
    policy.write_text(json.dumps({"agents": agents or {}}))
    monkeypatch.setenv("BIP_ALLOW", "pypi.org,deb.debian.org")
    monkeypatch.setenv("BIP_POLICY", str(policy))
    monkeypatch.setenv("BIP_REPORTS", str(reports))
    spec = importlib.util.spec_from_file_location(f"egress_{id(tmp_path)}", HERE / "egress_proxy.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module, policy, reports


def key(token):
    return hashlib.sha256(token.encode()).hexdigest()


def basic(name, token):
    return "Basic " + base64.b64encode(f"{name}:{token}".encode()).decode()


AGENTS = {"vie": {"key": key("t-vie"), "hosts": ["data.gouv.fr", "*.example.org"]},
          "budget": {"key": key("t-budget"), "hosts": []}}


def test_who_checks_the_token(tmp_path, monkeypatch):
    proxy, _, _ = load(tmp_path, monkeypatch, AGENTS)
    assert proxy.who(basic("vie", "t-vie")) == "vie"
    assert proxy.who(basic("vie", "t-budget")) is None
    assert proxy.who(basic("nobody", "t-vie")) is None
    assert proxy.who("Bearer t-vie") is None and proxy.who("") is None and proxy.who("Basic !!") is None


def test_each_agent_its_own_sites_the_registries_for_all(tmp_path, monkeypatch):
    proxy, policy, _ = load(tmp_path, monkeypatch, AGENTS)
    assert proxy.allowed("pypi.org", 443, 443) and proxy.allowed("deb.debian.org", 80, 80)
    assert not proxy.allowed("data.gouv.fr", 443, 443)                 # no credentials: registries only
    assert proxy.allowed("Data.Gouv.FR.", 443, 443, "vie")
    assert not proxy.allowed("data.gouv.fr", 80, 80, "vie")              # HTTPS only
    assert not proxy.allowed("data.gouv.fr", 8443, 443, "vie")
    assert not proxy.allowed("data.gouv.fr", 443, 443, "budget")
    assert proxy.allowed("api.example.org", 443, 443, "vie") and proxy.allowed("a.b.example.org", 443, 443, "vie")
    assert not proxy.allowed("example.org", 443, 443, "vie") and not proxy.allowed("badexample.org", 443, 443, "vie")
    # the bridge changes the list: read again
    AGENTS2 = {"budget": {"key": key("t-budget"), "hosts": ["impots.gouv.fr"]}}
    policy.write_text(json.dumps({"agents": AGENTS2}) + " " * 7)
    os.utime(policy, ns=(1, 2))
    assert proxy.allowed("impots.gouv.fr", 443, 443, "budget")
    assert not proxy.allowed("data.gouv.fr", 443, 443, "vie")


def test_never_an_address_of_this_machine(tmp_path, monkeypatch):
    proxy, _, _ = load(tmp_path, monkeypatch)
    for address in ("127.0.0.1", "10.0.0.5", "192.168.1.2", "172.17.0.1", "100.101.102.103", "169.254.169.254",
                    "::1", "fd00::1", "fe80::1", "::ffff:127.0.0.1", "0.0.0.0"):
        assert not proxy.public(address), address
    assert proxy.public("151.101.0.223") and proxy.public("2a04:4e42::223")
    assert asyncio.run(proxy.resolve("127.0.0.1", 443)) == []
    assert asyncio.run(proxy.resolve("localhost", 443)) == []


def test_a_refusal_is_reported_once(tmp_path, monkeypatch):
    proxy, _, reports = load(tmp_path, monkeypatch, AGENTS)
    proxy.report("vie", "api.meteo.fr", 443)
    proxy.report("vie", "api.meteo.fr", 443)
    proxy.report(None, "x.fr", 443)
    proxy.report("vie", "x.fr", 80)
    lines = [json.loads(l) for l in reports.read_text().splitlines()]
    assert [(l["agent"], l["host"]) for l in lines] == [("vie", "api.meteo.fr")]


class Echo:
    """An upstream server on this machine standing in for a site; the proxy's resolution is patched to reach it."""

    def __init__(self):
        self.seen = []

    async def handle(self, reader, writer):
        data = await reader.read(65536)
        self.seen.append(data)
        writer.write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
        await writer.drain()
        writer.close()


async def exchange(port, raw):
    reader, writer = await asyncio.open_connection("127.0.0.1", port)
    writer.write(raw)
    await writer.drain()
    data = await asyncio.wait_for(reader.read(65536), 5)
    writer.close()
    return data


def test_through_the_proxy(tmp_path, monkeypatch):
    proxy, _, reports = load(tmp_path, monkeypatch, AGENTS)

    async def scenario():
        echo = Echo()
        upstream = await asyncio.start_server(echo.handle, "127.0.0.1", 0)
        up_port = upstream.sockets[0].getsockname()[1]

        async def resolve(host, port):
            return ["127.0.0.1"]

        async def connect(addresses, port):
            return await asyncio.open_connection(addresses[0], up_port)

        monkeypatch.setattr(proxy, "resolve", resolve)
        monkeypatch.setattr(proxy, "connect", connect)
        server = await asyncio.start_server(proxy.handle, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        auth = basic("vie", "t-vie").encode()
        out = {}
        out["anonymous"] = await exchange(port, b"CONNECT data.gouv.fr:443 HTTP/1.1\r\nHost: data.gouv.fr\r\n\r\n")
        out["registry"] = await exchange(port, b"CONNECT pypi.org:443 HTTP/1.1\r\n\r\n")
        out["own"] = await exchange(port, b"CONNECT data.gouv.fr:443 HTTP/1.1\r\nProxy-Authorization: " + auth + b"\r\n\r\n")
        out["other"] = await exchange(port, b"CONNECT api.meteo.fr:443 HTTP/1.1\r\nProxy-Authorization: " + auth + b"\r\n\r\n")
        out["apt"] = await exchange(port, b"GET http://deb.debian.org/debian/dists HTTP/1.1\r\nHost: deb.debian.org\r\n"
                                          b"Proxy-Authorization: " + auth + b"\r\nAccept: */*\r\n\r\n")
        server.close()
        upstream.close()
        return out, echo

    out, echo = asyncio.run(scenario())
    assert out["anonymous"].startswith(b"HTTP/1.1 407") and b"Proxy-Authenticate: Basic" in out["anonymous"]
    assert out["registry"].startswith(b"HTTP/1.1 200 Connection established")
    assert out["own"].startswith(b"HTTP/1.1 200 Connection established")
    assert out["other"].startswith(b"HTTP/1.1 403")
    assert b"200 OK" in out["apt"]
    forwarded = next(s for s in echo.seen if s.startswith(b"GET"))
    assert forwarded.startswith(b"GET /debian/dists HTTP/1.1\r\n") and b"Accept: */*" in forwarded
    assert b"Proxy-Authorization" not in forwarded and b"t-vie" not in forwarded
    assert json.loads(reports.read_text())["host"] == "api.meteo.fr"


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-q"]))
