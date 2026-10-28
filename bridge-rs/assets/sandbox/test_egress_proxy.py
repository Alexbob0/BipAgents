"""The agents' proxy (egress_proxy.py): reading open to any public name for a known agent, never an address of this
machine's networks; what an agent sends to a site its person did not validate counted and cut past the budgets, with
one report; the offer's limits (downloads, connections, blocklist); the visible browser known by its address.
Run: `python3 -m pytest -q bridge-rs/assets/sandbox`."""
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


def load(tmp_path, monkeypatch, agents=None, limits=None, browsers=None, env=None):
    policy, reports = tmp_path / "sites.json", tmp_path / "reports.jsonl"
    body = {"agents": agents or {}}
    if limits is not None:
        body["limits"] = limits
    if browsers is not None:
        body["browsers"] = browsers
    policy.write_text(json.dumps(body))
    monkeypatch.setenv("BIP_ALLOW", "pypi.org,deb.debian.org")
    monkeypatch.setenv("BIP_POLICY", str(policy))
    monkeypatch.setenv("BIP_REPORTS", str(reports))
    monkeypatch.setenv("BIP_USAGE", str(tmp_path / "usage.json"))
    for name in ("BIP_BROWSERS", "BIP_BLOCKLIST", "BIP_STRICT"):
        monkeypatch.delenv(name, raising=False)
    for name, value in (env or {}).items():
        monkeypatch.setenv(name, value)
    spec = importlib.util.spec_from_file_location(f"egress_{id(tmp_path)}", HERE / "egress_proxy.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.SINK_IDLE = 0.3
    return module, policy, reports


def key(token):
    return hashlib.sha256(token.encode()).hexdigest()


def basic(name, token):
    return "Basic " + base64.b64encode(f"{name}:{token}".encode()).decode()


AGENTS = {"vie": {"key": key("t-vie"), "hosts": ["data.gouv.fr", "*.example.org"]},
          "budget": {"key": key("t-budget"), "hosts": []},
          "conn-c1": {"key": key("t-conn"), "hosts": ["api.garmin.example"], "connector": True}}


def reports_of(reports):
    return [json.loads(line) for line in reports.read_text().splitlines()] if reports.exists() else []


def test_who_checks_the_token(tmp_path, monkeypatch):
    proxy, _, _ = load(tmp_path, monkeypatch, AGENTS)
    assert proxy.who(basic("vie", "t-vie")) == "vie"
    assert proxy.who(basic("vie", "t-budget")) is None
    assert proxy.who(basic("nobody", "t-vie")) is None
    assert proxy.who("Bearer t-vie") is None and proxy.who("") is None and proxy.who("Basic !!") is None


def test_reading_is_open_to_known_agents_never_this_machine(tmp_path, monkeypatch):
    proxy, policy, _ = load(tmp_path, monkeypatch, AGENTS)
    assert proxy.allowed("pypi.org", 443, 443) and proxy.allowed("deb.debian.org", 80, 80)
    assert not proxy.allowed("data.gouv.fr", 443, 443)                 # no credentials: registries only
    for agent in ("vie", "budget"):
        assert proxy.allowed("www.lemonde.fr", 443, 443, agent) and proxy.allowed("xn--bcher-kva.example", 443, 443, agent)
    assert not proxy.allowed("www.lemonde.fr", 80, 80, "vie")             # programs: HTTPS only
    assert proxy.allowed("www.lemonde.fr", 80, 80, "vie", browser=True)   # its browser: http:// pages too
    assert not proxy.allowed("www.lemonde.fr", 8443, 443, "vie")
    for host in ("localhost", "nas.local", "host.containers.internal", "10.0.0.1", "[::1]", "router.home", "x"):
        assert not proxy.allowed(host, 443, 443, "vie"), host
        assert not proxy.allowed(host, 80, 80, "vie", browser=True), host
    # validated for sending: the registries and the person's hosts, per agent
    assert proxy.validated("vie", "Data.Gouv.FR.") and proxy.validated("vie", "a.b.example.org")
    assert proxy.validated("budget", "pypi.org") and not proxy.validated("budget", "data.gouv.fr")
    assert not proxy.validated("vie", "example.org") and not proxy.validated("vie", "badexample.org")
    # the bridge changes the list: read again
    policy.write_text(json.dumps({"agents": {"budget": {"key": key("t-budget"), "hosts": ["impots.gouv.fr"]}}}) + " " * 7)
    os.utime(policy, ns=(1, 2))
    assert proxy.validated("budget", "impots.gouv.fr") and not proxy.validated("vie", "data.gouv.fr")


def test_a_connector_reaches_its_hosts_only(tmp_path, monkeypatch):
    proxy, _, _ = load(tmp_path, monkeypatch, AGENTS)
    assert proxy.allowed("api.garmin.example", 443, 443, "conn-c1")
    assert not proxy.allowed("pypi.org", 443, 443, "conn-c1") and not proxy.allowed("www.lemonde.fr", 443, 443, "conn-c1")
    assert not proxy.allowed("api.garmin.example", 443, 443, "vie", strict=True)   # its network: connectors only


def test_never_an_address_of_this_machine(tmp_path, monkeypatch):
    proxy, _, _ = load(tmp_path, monkeypatch)
    for address in ("127.0.0.1", "10.0.0.5", "192.168.1.2", "172.17.0.1", "100.101.102.103", "169.254.169.254",
                    "::1", "fd00::1", "fe80::1", "::ffff:127.0.0.1", "0.0.0.0"):
        assert not proxy.public(address), address
    assert proxy.public("151.101.0.223") and proxy.public("2a04:4e42::223")
    assert asyncio.run(proxy.resolve("127.0.0.1", 443)) == []
    assert asyncio.run(proxy.resolve("localhost", 443)) == []


def record(kind, size):
    return bytes([kind, 3, 3]) + size.to_bytes(2, "big") + b"x" * size


def test_tls_counts_the_application_data_only(tmp_path, monkeypatch):
    proxy, _, _ = load(tmp_path, monkeypatch)
    hello = record(22, 512)
    data = record(23, 700) + record(20, 1) + record(23, 300)
    tls = proxy.Tls()
    stream = hello + data
    # in any slicing (a header split across reads included)
    counted = sum(tls.count(stream[i:i + 7]) for i in range(0, len(stream), 7))
    assert counted == 1000
    assert proxy.Tls().count(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n") == 27   # not TLS: every byte
    # a browser's: the first 4 KB of each record free (its requests), an upload's full records counted past them
    browser = proxy.Tls(4096)
    requests = hello + b"".join(record(23, 1500) for _ in range(200))
    assert sum(browser.count(requests[i:i + 5000]) for i in range(0, len(requests), 5000)) == 0
    upload = b"".join(record(23, 16384) for _ in range(128))           # 2 MB
    assert browser.count(upload) == 128 * (16384 - 4096)


class Site:
    """An upstream server on this machine standing in for a site; the proxy's resolution is patched to reach it.
    It reads what comes (counting it) and, when `serve` is set, answers with that many bytes."""

    def __init__(self, serve=0):
        self.serve = serve
        self.received = 0

    async def handle(self, reader, writer):
        try:
            if self.serve:
                await reader.read(65536)
                block = b"y" * 65536
                left = self.serve
                while left > 0:
                    writer.write(block[:left])
                    await writer.drain()
                    left -= 65536
            else:
                while True:
                    data = await reader.read(65536)
                    if not data:
                        break
                    self.received += len(data)
        except Exception:
            pass
        writer.close()


async def through(proxy, monkeypatch, site, steps):
    """Runs `steps(port)` against the proxy, the proxy's every site being `site`."""
    upstream = await asyncio.start_server(site.handle, "127.0.0.1", 0)
    up_port = upstream.sockets[0].getsockname()[1]

    async def resolve(host, port):
        return ["127.0.0.1"]

    async def connect(addresses, port):
        return await asyncio.open_connection(addresses[0], up_port)

    monkeypatch.setattr(proxy, "resolve", resolve)
    monkeypatch.setattr(proxy, "connect", connect)
    server = await asyncio.start_server(proxy.handle, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]
    try:
        return await steps(port)
    finally:
        server.close()
        upstream.close()


async def tunnel(port, host, auth=None):
    """A CONNECT: (status line, reader, writer)."""
    reader, writer = await asyncio.open_connection("127.0.0.1", port)
    extra = f"Proxy-Authorization: {auth}\r\n" if auth else ""
    writer.write(f"CONNECT {host}:443 HTTP/1.1\r\nHost: {host}\r\n{extra}\r\n".encode())
    await writer.drain()
    head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 5)
    return head.split(b"\r\n")[0].decode(), reader, writer


async def send_tls(port, host, auth, app_bytes, chunk=16000):
    """A TLS-looking stream: a handshake, then `app_bytes` of application data. Returns (status, bytes written before
    the proxy stopped taking them, whether the tunnel was closed on us)."""
    status, reader, writer = await tunnel(port, host, auth)
    if not status.startswith("HTTP/1.1 200"):
        writer.close()
        return status, 0, True
    written = 0
    try:
        writer.write(record(22, 517))
        left = app_bytes
        while left > 0:
            size = min(chunk, left)
            writer.write(record(23, size))
            await writer.drain()
            written += size
            left -= size
        await asyncio.sleep(0.05)
        closed = await asyncio.wait_for(reader.read(10), 0.5) == b""
    except (ConnectionError, asyncio.TimeoutError):
        closed = True
    writer.close()
    return status, written, closed


VIE = basic("vie", "t-vie")
LIMITS = {"send_site_hour": 64 * 1024, "send_day": 256 * 1024, "held_tunnel": 16 * 1024}


def test_a_big_upload_is_cut_with_one_report_a_validated_site_passes(tmp_path, monkeypatch):
    proxy, _, reports = load(tmp_path, monkeypatch, AGENTS, LIMITS)
    site = Site()

    async def steps(port):
        out = {}
        # a form post of a few KB, then a 2 MB upload to the same unvalidated site
        out["form"] = await send_tls(port, "upload.example.net", VIE, 6000)
        out["big"] = await send_tls(port, "upload.example.net", VIE, 2 * 1024 * 1024)
        await asyncio.sleep(0.5)
        # the host is held: reading still opens it, a request's size passes, an upload is cut (no new report)
        out["read"] = await send_tls(port, "upload.example.net", VIE, 3000)
        out["again"] = await send_tls(port, "upload.example.net", VIE, 200 * 1024)
        await asyncio.sleep(0.5)
        # validated for vie: no limit at all
        out["validated"] = await send_tls(port, "data.gouv.fr", VIE, 1024 * 1024)
        return out

    out = asyncio.run(through(proxy, monkeypatch, site, steps))
    assert out["form"][0].startswith("HTTP/1.1 200")
    lines = reports_of(reports)
    assert [(l["kind"], l["agent"], l["host"]) for l in lines] == [("send", "vie", "upload.example.net")], lines
    assert 1.5 * 1024 * 1024 < lines[0]["bytes"] <= 2.2 * 1024 * 1024        # « environ 2 Mo »
    assert lines[0]["day"] is False
    assert out["read"][0].startswith("HTTP/1.1 200")
    assert out["validated"][0].startswith("HTTP/1.1 200")
    # what reached the sites: the form, at most the hour's budget of the upload, the small read, a held tunnel's cap,
    # and the validated megabyte whole
    assert site.received >= 1024 * 1024 + 6000
    assert site.received < 1024 * 1024 + 6000 + 64 * 1024 + 3000 + 16 * 1024 + 6 * 1024


def test_downloads_of_any_size_pass(tmp_path, monkeypatch):
    proxy, _, reports = load(tmp_path, monkeypatch, AGENTS, LIMITS)
    site = Site(serve=20 * 1024 * 1024)

    async def steps(port):
        status, reader, writer = await tunnel(port, "files.example.net", VIE)
        writer.write(record(22, 517) + record(23, 400))      # a GET
        await writer.drain()
        got = 0
        while True:
            data = await asyncio.wait_for(reader.read(1 << 20), 5)
            if not data:
                break
            got += len(data)
        writer.close()
        return status, got

    status, got = asyncio.run(through(proxy, monkeypatch, site, steps))
    assert status.startswith("HTTP/1.1 200") and got == 20 * 1024 * 1024
    assert reports_of(reports) == []
    proxy.write_usage()
    usage = json.loads((tmp_path / "usage.json").read_text())["agents"]["vie"]
    assert usage["received"] == 20 * 1024 * 1024 and usage["connections"] == 1


def test_a_slow_leak_hits_the_days_total(tmp_path, monkeypatch):
    proxy, policy, reports = load(tmp_path, monkeypatch, AGENTS, LIMITS)
    site = Site()

    async def steps(port):
        # 40 KB to each of many sites, under the hour's budget per site
        results = []
        for i in range(8):
            results.append(await send_tls(port, f"s{i}.leak.example", VIE, 40 * 1024, chunk=8 * 1024))
        await asyncio.sleep(0.5)
        return results

    results = asyncio.run(through(proxy, monkeypatch, site, steps))
    lines = reports_of(reports)
    assert len(lines) == 1 and lines[0]["day"] is True and lines[0]["host"] == "s6.leak.example", lines
    assert site.received <= 256 * 1024 + 7 * 1024 + 16 * 1024
    assert proxy.day_of("vie")["held"] is True
    # the person said « Autoriser »: the bridge's `reset`, counts cleared
    import time
    data = json.loads(policy.read_text())
    data["agents"]["vie"]["reset"] = time.time() + 1
    data["agents"]["vie"]["hosts"].append("s6.leak.example")
    policy.write_text(json.dumps(data))
    os.utime(policy, ns=(3, 4))
    assert proxy.day_of("vie")["held"] is False and proxy.day_of("vie")["unvalidated"] == 0


def test_plain_http_request_bodies_count_too(tmp_path, monkeypatch):
    proxy, _, reports = load(tmp_path, monkeypatch, AGENTS, LIMITS, browsers={"127.0.0.1": "budget"},
                             env={"BIP_BROWSERS": "127.0.0.1"})
    site = Site()

    async def steps(port):
        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        body = b"z" * (300 * 1024)
        writer.write(b"POST http://forms.example.net/up HTTP/1.1\r\nHost: forms.example.net\r\n"
                     + f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
        await writer.drain()
        await asyncio.sleep(1)
        writer.close()

    asyncio.run(through(proxy, monkeypatch, site, steps))
    [line] = reports_of(reports)
    assert line["agent"] == "budget" and line["host"] == "forms.example.net" and line["bytes"] >= 280 * 1024
    assert site.received <= 64 * 1024 + 4096 + 200


def test_the_browser_is_known_by_its_address(tmp_path, monkeypatch):
    proxy, _, _ = load(tmp_path, monkeypatch, AGENTS, browsers={"127.0.0.1": "vie"}, env={"BIP_BROWSERS": "127.0.0.1"})
    site = Site()

    async def steps(port):
        ok = (await tunnel(port, "www.lemonde.fr"))[0]
        local = (await tunnel(port, "nas.local"))[0]
        return ok, local

    ok, local = asyncio.run(through(proxy, monkeypatch, site, steps))
    assert ok.startswith("HTTP/1.1 200") and local.startswith("HTTP/1.1 403")
    # an address the policy doesn't know: refused (never asked for credentials)
    other = tmp_path / "other"
    other.mkdir()
    proxy2, _, _ = load(other, monkeypatch, AGENTS, browsers={"10.0.0.9": "vie"}, env={"BIP_BROWSERS": "127.0.0.1"})

    async def steps2(port):
        return (await tunnel(port, "www.lemonde.fr"))[0]

    assert asyncio.run(through(proxy2, monkeypatch, Site(), steps2)).startswith("HTTP/1.1 403 Unknown browser")


def test_offer_limits_downloads_connections_blocklist(tmp_path, monkeypatch):
    blocklist = tmp_path / "blocklist.txt"
    blocklist.write_text("# piracy\nthepiratebay.example\ntracker.opentrackr.example\n\n")
    proxy, _, _ = load(tmp_path, monkeypatch, AGENTS,
                       dict(LIMITS, download_day=3 * 1024 * 1024, connections_minute=5),
                       env={"BIP_BLOCKLIST": str(blocklist)})
    site = Site(serve=2 * 1024 * 1024)

    async def fetch(port, host):
        status, reader, writer = await tunnel(port, host, VIE)
        if not status.startswith("HTTP/1.1 200"):
            writer.close()
            return status, 0
        writer.write(record(22, 100) + record(23, 300))
        await writer.drain()
        got = 0
        try:
            while True:
                data = await asyncio.wait_for(reader.read(1 << 20), 5)
                if not data:
                    break
                got += len(data)
        except ConnectionError:
            pass
        writer.close()
        return status, got

    async def steps(port):
        out = {}
        out["blocked"] = await fetch(port, "www.thepiratebay.example")
        out["first"] = await fetch(port, "a.example.net")
        out["second"] = await fetch(port, "b.example.net")       # crosses 3 MB: cut
        out["third"] = await fetch(port, "c.example.net")        # refused for today
        out["registry"] = await fetch(port, "pypi.org")          # the day's limit counts the registries too
        return out

    out = asyncio.run(through(proxy, monkeypatch, site, steps))
    assert out["blocked"][0] == "HTTP/1.1 403 Site refused on this service (piracy or torrent tracker)"
    assert out["first"] == ("HTTP/1.1 200 Connection established", 2 * 1024 * 1024)
    assert out["second"][1] < 2 * 1024 * 1024
    assert out["third"][0].startswith("HTTP/1.1 403 Download limit for today reached")
    assert out["registry"][0].startswith("HTTP/1.1 403 Download limit")
    assert proxy.blocked_name("x.tracker.opentrackr.example") and not proxy.blocked_name("opentrackr.example")

    proxy2, _, _ = load(tmp_path, monkeypatch, AGENTS, dict(LIMITS, connections_minute=3))

    async def many(port):
        return [(await tunnel(port, f"h{i}.example.net", VIE))[0] for i in range(5)]

    statuses = asyncio.run(through(proxy2, monkeypatch, Site(), many))
    assert [s.split(" ")[1] for s in statuses] == ["200", "200", "200", "429", "429"]
    assert statuses[3] == "HTTP/1.1 429 Too many new connections - wait a minute"


def test_without_limits_nothing_is_capped_but_sends(tmp_path, monkeypatch):
    proxy, _, _ = load(tmp_path, monkeypatch, AGENTS)
    assert proxy.limit("download_day") == 0 and proxy.limit("connections_minute") == 0
    assert proxy.limit("send_site_hour") == 64 * 1024 and proxy.limit("send_day") == 256 * 1024
    assert not proxy.blocked_name("thepiratebay.example")


def test_registries_and_anonymous_clients(tmp_path, monkeypatch):
    proxy, _, reports = load(tmp_path, monkeypatch, AGENTS)
    site = Site()

    async def steps(port):
        out = {}
        out["anonymous"] = (await tunnel(port, "data.gouv.fr"))[0]
        out["registry"] = (await tunnel(port, "pypi.org"))[0]
        out["conn"] = (await tunnel(port, "pypi.org", basic("conn-c1", "t-conn")))[0]
        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        writer.write(b"GET http://deb.debian.org/debian/dists HTTP/1.1\r\nHost: deb.debian.org\r\n"
                     b"Proxy-Authorization: " + VIE.encode() + b"\r\nAccept: */*\r\n\r\n")
        await writer.drain()
        await asyncio.sleep(0.3)
        writer.close()
        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        writer.write(b"GET http://www.lemonde.fr/ HTTP/1.1\r\nHost: www.lemonde.fr\r\nProxy-Authorization: "
                     + VIE.encode() + b"\r\n\r\n")
        await writer.drain()
        out["http"] = (await asyncio.wait_for(reader.read(4096), 5)).split(b"\r\n")[0]
        writer.close()
        return out

    out = asyncio.run(through(proxy, monkeypatch, site, steps))
    assert out["anonymous"].startswith("HTTP/1.1 407")
    assert out["registry"].startswith("HTTP/1.1 200 Connection established")
    assert out["conn"].startswith("HTTP/1.1 403 Site not approved for this connector")
    assert out["http"] == b"HTTP/1.1 403 Only HTTPS sites are reachable (https://, port 443)"
    assert site.received > 0 and reports_of(reports) == []


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-q"]))
