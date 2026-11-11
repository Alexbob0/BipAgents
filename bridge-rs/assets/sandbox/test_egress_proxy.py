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
import shutil
import sys
import tempfile
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


def test_never_an_ipv4_wrapped_in_ipv6(tmp_path, monkeypatch):
    """NAT64 (64:ff9b::/96, and its local-use 64:ff9b:1::/48) and 6to4 (2002::/16) carry an IPv4 address: behind such
    a gateway, 64:ff9b::c0a8:101 is 192.168.1.1. Refused whatever the IPv4 inside, whatever Python's version says."""
    proxy, _, _ = load(tmp_path, monkeypatch)
    for address in ("64:ff9b::c0a8:101", "64:ff9b::a00:1", "64:ff9b::7f00:1", "64:ff9b::808:808",
                    "64:ff9b:1::c0a8:101", "2002:c0a8:101::1", "2002:a00:1::", "2002:808:808::1",
                    "::ffff:192.168.1.1", "::ffff:10.0.0.1"):
        assert not proxy.public(address), address
    # the IPv4 inside a mapped address is judged as itself; a public IPv6 next to these prefixes stays public
    assert proxy.public("::ffff:8.8.8.8") and proxy.public("2001:4860:4860::8888") and proxy.public("2003::1")


def record(kind, size):
    return bytes([kind, 3, 3]) + size.to_bytes(2, "big") + b"x" * size


def test_tls_counts_every_record_type(tmp_path, monkeypatch):
    """Every record's content counts, whatever its type (BV-4, 11/11: data framed as handshake records was free); a
    browser's first 4 KB of each record stay free."""
    proxy, _, _ = load(tmp_path, monkeypatch)
    hello = record(22, 512)
    data = record(23, 700) + record(20, 1) + record(23, 300)
    tls = proxy.Tls()
    stream = hello + data
    # in any slicing (a header split across reads included)
    counted = sum(tls.count(stream[i:i + 7]) for i in range(0, len(stream), 7))
    assert counted == 512 + 1000 + 1
    assert proxy.Tls().count(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n") == 27   # not TLS: every byte
    # a browser's: the first 4 KB of each record free (its hello, its requests), an upload's full records counted past
    browser = proxy.Tls(4096)
    requests = hello + b"".join(record(23, 1500) for _ in range(200))
    assert sum(browser.count(requests[i:i + 5000]) for i in range(0, len(requests), 5000)) == 0
    upload = b"".join(record(23, 16384) for _ in range(128))           # 2 MB
    assert browser.count(upload) == 128 * (16384 - 4096)
    # handshake-typed records are no way around it
    handshakes = proxy.Tls(4096)
    assert handshakes.count(b"".join(record(22, 16384) for _ in range(128))) == 128 * (16384 - 4096)


def test_a_programs_every_byte_counts(tmp_path, monkeypatch):
    """A sandbox program frames its bytes as it likes: N bytes framed as handshake records count N (and their record
    headers too)."""
    proxy, _, _ = load(tmp_path, monkeypatch, AGENTS, LIMITS)
    guard = proxy.Guard("budget", "x.example.net", True)
    stream = b"".join(record(22, 1000) for _ in range(10))
    assert guard.sending(stream) and guard.app == len(stream) == 10 * 1005


class Site:
    """An upstream server on this machine standing in for a site; the proxy's resolution is patched to reach it.
    It reads what comes (counting it) and, when `serve` is set, answers with that many bytes."""

    def __init__(self, serve=0):
        self.serve = serve
        self.received = 0
        self.heads = []      # the first bytes of each connection

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
                first = True
                while True:
                    data = await reader.read(65536)
                    if not data:
                        break
                    if first:
                        self.heads.append(data[:512])
                        first = False
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


async def send_tls(port, host, auth, app_bytes, chunk=16000, kind=23):
    """A TLS-looking stream: a handshake, then `app_bytes` in records of `kind` (application data). Returns (status,
    bytes written before the proxy stopped taking them, whether the tunnel was closed on us)."""
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
            writer.write(record(kind, size))
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
LIMITS = {"send_site_hour": 64 * 1024, "send_day": 256 * 1024, "held_site": 16 * 1024, "held_day": 64 * 1024}


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
    # what reached the sites: the form, at most the hour's budget of the upload, the small read and the rest of the
    # host's held allowance (both held tunnels together), and the validated megabyte whole
    assert site.received >= 1024 * 1024 + 6000
    assert site.received < 1024 * 1024 + 6000 + 64 * 1024 + 16 * 1024 + 6 * 1024


def test_data_framed_as_handshake_records_is_cut_and_reported(tmp_path, monkeypatch):
    """BV-4 (11/11): a program writing 1 MB as TLS handshake records (type 22) to a site nobody validated is counted,
    cut past the hour's budget and reported once, like any upload."""
    proxy, _, reports = load(tmp_path, monkeypatch, AGENTS, LIMITS)
    site = Site()

    async def steps(port):
        return await send_tls(port, "drop.example.net", VIE, 1024 * 1024, kind=22)

    status, _, closed = asyncio.run(through(proxy, monkeypatch, site, steps))
    assert status.startswith("HTTP/1.1 200") and closed
    lines = reports_of(reports)
    assert [(l["agent"], l["host"]) for l in lines] == [("vie", "drop.example.net")], lines
    assert lines[0]["bytes"] >= 900 * 1024
    assert site.received <= 64 * 1024


def test_a_held_host_shares_one_allowance_then_refuses(tmp_path, monkeypatch):
    """PI-2 (11/11): once a send to a host was cut, its new tunnels share ONE held allowance (16 KB), not 16 KB each;
    past it, new connections to it are refused (403) until the hold ends. Other hosts are untouched."""
    proxy, _, reports = load(tmp_path, monkeypatch, AGENTS, LIMITS)
    site = Site()

    async def steps(port):
        out = {}
        out["big"] = await send_tls(port, "upload.example.net", VIE, 300 * 1024)
        await asyncio.sleep(0.5)
        before = site.received
        # requests of 3 KB (with their 0.5 KB hello): four fit in the host's held 16 KB, the fifth is cut
        out["tunnels"] = [await send_tls(port, "upload.example.net", VIE, 3000) for _ in range(20)]
        await asyncio.sleep(0.5)
        out["held"] = site.received - before
        out["other"] = await send_tls(port, "other.example.net", VIE, 3000)
        return out

    out = asyncio.run(through(proxy, monkeypatch, site, steps))
    assert len(reports_of(reports)) == 1                       # one card, the held cuts are not reported
    assert 4 * (522 + 3005) <= out["held"] <= 16 * 1024, out["held"]
    statuses = [t[0].split(" ")[1] for t in out["tunnels"]]
    assert statuses == ["200"] * 5 + ["403"] * 15, statuses
    assert out["tunnels"][-1][0] == "HTTP/1.1 403 Sending paused - your person has a card to answer about this site"
    assert out["other"][0].startswith("HTTP/1.1 200")


def test_a_held_day_shares_one_allowance(tmp_path, monkeypatch):
    """Past the day's total every host not validated is held: all of them together get held_day (64 KB) more, then
    new connections to them are refused; a validated host still passes."""
    proxy, _, _ = load(tmp_path, monkeypatch, AGENTS, LIMITS)
    proxy.day_of("vie")["held"] = True
    site = Site()

    async def steps(port):
        results = [await send_tls(port, f"h{i}.example.net", VIE, 10 * 1024, chunk=2000) for i in range(12)]
        await asyncio.sleep(0.5)
        held = site.received
        validated = await send_tls(port, "data.gouv.fr", VIE, 100 * 1024)
        return results, held, validated

    results, held, validated = asyncio.run(through(proxy, monkeypatch, site, steps))
    assert held <= 64 * 1024, held
    assert results[-1][0].startswith("HTTP/1.1 403 Sending paused")
    assert validated[0].startswith("HTTP/1.1 200") and validated[1] == 100 * 1024


def test_many_distinct_names_reach_the_days_total(tmp_path, monkeypatch):
    """PI-5 (11/11): data spelt in host names, one name per CONNECT, nothing sent inside: each name's length counts
    before it is looked up; the day's total is reached (one report), then the held day's allowance, then every new
    name is refused without being looked up. The same name again costs nothing."""
    proxy, _, reports = load(tmp_path, monkeypatch, AGENTS, dict(LIMITS, send_day=20 * 1024, held_day=4 * 1024))
    looked_up = []

    async def steps(port):
        real = proxy.resolve

        async def resolve(host, port):
            looked_up.append(host)
            return await real(host, port)

        proxy.resolve = resolve
        for _ in range(3):
            (await tunnel(port, "same.example.net", VIE))[2].close()
        same = proxy.day_of("vie")["unvalidated"]
        looked_up.clear()
        statuses = []
        for i in range(200):
            name = f"{i:04d}{'a' * 55}.{'b' * 60}.leak.example"     # 133 characters
            status, _, writer = await tunnel(port, name, VIE)
            writer.close()
            statuses.append(status.split(" ")[1])
        again = (await tunnel(port, "www.lemonde.fr", VIE))[0]
        return same, statuses, again

    same, statuses, again = asyncio.run(through(proxy, monkeypatch, Site(), steps))
    assert same == len("same.example.net")
    first_refused = statuses.index("403")
    # 20 KB of names, then 4 KB more while held: about 154 + 30 names, then none
    assert 150 <= first_refused <= 190, first_refused
    assert set(statuses[first_refused:]) == {"403"} and len(looked_up) == first_refused
    [line] = reports_of(reports)
    assert line["day"] is True and line["host"].endswith(".leak.example")
    # the held day spent: any new host not validated is refused
    assert again.startswith("HTTP/1.1 403 Sending paused")
    assert proxy.day_of("vie")["unvalidated"] <= 20 * 1024


def test_registries_take_installs_not_uploads(tmp_path, monkeypatch):
    """PI-3 (11/11): what is SENT to a package registry is bounded per agent (or per client address without
    credentials) per hour and per day, with no card: past it the tunnel is cut and new connections to the registries
    get a plain 403 until the window passes; another agent is not concerned. Downloads are not counted."""
    proxy, _, reports = load(tmp_path, monkeypatch, AGENTS, dict(LIMITS, registry_hour=256 * 1024))
    site = Site()

    async def steps(port):
        out = {}
        out["upload"] = await send_tls(port, "pypi.org", VIE, 1024 * 1024)
        await asyncio.sleep(0.3)
        out["after"] = await send_tls(port, "pypi.org", VIE, 1000)
        out["plain"] = (await tunnel(port, "deb.debian.org", VIE))[0]
        out["budget"] = await send_tls(port, "pypi.org", basic("budget", "t-budget"), 100 * 1024)
        out["anonymous"] = await send_tls(port, "pypi.org", None, 1024 * 1024)
        await asyncio.sleep(0.3)
        out["anonymous_after"] = (await tunnel(port, "pypi.org"))[0]
        out["site"] = await send_tls(port, "www.lemonde.fr", VIE, 3000)   # other sites: as before
        return out

    out = asyncio.run(through(proxy, monkeypatch, site, steps))
    assert out["upload"][0].startswith("HTTP/1.1 200") and out["upload"][2]          # cut
    assert out["after"][0] == "HTTP/1.1 403 Sending to package registries is limited - wait an hour"
    assert out["plain"].startswith("HTTP/1.1 403 Sending to package registries is limited")
    assert out["budget"][0].startswith("HTTP/1.1 200") and out["budget"][1] == 100 * 1024
    assert out["anonymous"][0].startswith("HTTP/1.1 200") and out["anonymous"][2]
    assert out["anonymous_after"].startswith("HTTP/1.1 403 Sending to package registries is limited")
    assert out["site"][0].startswith("HTTP/1.1 200")
    # vie's and the anonymous client's uploads stopped at the hour's ceiling each, budget's 100 KB whole
    assert site.received <= 2 * 256 * 1024 + 100 * 1024 + 2 * 1024 + 4 * 1024, site.received
    assert reports_of(reports) == []                                                   # never a card


def test_an_install_stays_far_below_the_registries_ceiling(tmp_path, monkeypatch):
    """With the shipped ceilings: a large cold install (40 connections, 2 000 requests of 700 bytes each, a 1.8 KB
    ClientHello per connection — about what npm sends for ~2 000 packages) passes whole, using a quarter of the
    hour's ceiling, and its downloads count for nothing."""
    proxy, _, reports = load(tmp_path, monkeypatch, AGENTS, LIMITS)
    site = Site()

    async def steps(port):
        results = []
        for _ in range(40):
            status, reader, writer = await tunnel(port, "pypi.org", VIE)
            writer.write(record(22, 1800))
            for _ in range(50):
                writer.write(record(23, 700))
            await writer.drain()
            writer.close()
            results.append(status)
        await asyncio.sleep(0.3)
        return results

    results = asyncio.run(through(proxy, monkeypatch, site, steps))
    sent = 40 * (1805 + 50 * 705)
    assert all(r.startswith("HTTP/1.1 200") for r in results) and site.received == sent
    assert proxy._registries["vie"][1] == sent < proxy.LIMITS["registry_hour"] / 4
    assert reports_of(reports) == []


def test_plain_http_carries_the_requested_host(tmp_path, monkeypatch):
    """A plain-HTTP request for a registry with another site in its Host line (a shared front would serve that
    site): the Host line sent is the registry's (27/10 audit, low point)."""
    proxy, _, _ = load(tmp_path, monkeypatch, AGENTS, LIMITS)
    site = Site()

    async def steps(port):
        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        writer.write(b"GET http://deb.debian.org/debian/dists HTTP/1.1\r\nHost: attacker.example\r\n"
                     b"HOST: other.example\r\nAccept: */*\r\n\r\n")
        await writer.drain()
        await asyncio.sleep(0.3)
        writer.close()

    asyncio.run(through(proxy, monkeypatch, site, steps))
    [head] = site.heads
    assert head.startswith(b"GET /debian/dists HTTP/1.1\r\nHost: deb.debian.org\r\nAccept: */*\r\n\r\n"), head
    assert b"attacker" not in head and b"other.example" not in head


def test_the_browsers_free_bytes_have_a_pool(tmp_path, monkeypatch):
    """PI-4 / BV-8 (11/11): the browser's records of at most 4 KB counted nothing, with no total. Now their free bytes
    come from a pool per host per hour (and per agent per day); past it they count like a program's: the same cut, one
    report. Ordinary browsing (the 200 requests of 1.5 KB of the Tls test, 300 KB) stays far below the shipped pool."""
    proxy, _, reports = load(tmp_path, monkeypatch, AGENTS, dict(LIMITS, browser_free_site_hour=1024 * 1024),
                             browsers={"127.0.0.1": "vie"}, env={"BIP_BROWSERS": "127.0.0.1"})
    site = Site()

    async def steps(port):
        out = {}
        # 8 MB in 3 KB records, as `fetch` in a loop would: the pool (1 MB here), then the hour's 64 KB, then cut
        out["leak"] = await send_tls(port, "drop.example.net", None, 8 * 1024 * 1024, chunk=3000)
        await asyncio.sleep(0.5)
        out["leak_received"] = site.received
        return out

    out = asyncio.run(through(proxy, monkeypatch, site, steps))
    assert out["leak"][0].startswith("HTTP/1.1 200")
    [line] = reports_of(reports)                                            # cut, reported once
    assert line["agent"] == "vie" and line["host"] == "drop.example.net" and line["bytes"] >= 64 * 1024
    assert out["leak_received"] <= 1024 * 1024 + 64 * 1024 + 2 * 3005 + 600, out["leak_received"]
    proxy.write_usage()
    assert json.loads((tmp_path / "usage.json").read_text())["agents"]["vie"]["browser_free"] == 1024 * 1024

    # the shipped pool: ten times the Tls test's 200 requests of 1.5 KB to one host (3 MB) pass, nothing reported
    other = tmp_path / "other"
    other.mkdir()
    proxy2, _, reports2 = load(other, monkeypatch, AGENTS, LIMITS, browsers={"127.0.0.1": "vie"},
                               env={"BIP_BROWSERS": "127.0.0.1"})
    assert proxy2.limit("browser_free_site_hour") == 4 * 1024 * 1024
    assert proxy2.limit("browser_free_day") == 32 * 1024 * 1024
    site2 = Site()

    async def browse(port):
        return [await send_tls(port, "www.leboncoin.example", None, 200 * 1500, chunk=1500) for _ in range(10)]

    pages = asyncio.run(through(proxy2, monkeypatch, site2, browse))
    assert all(p[0].startswith("HTTP/1.1 200") and p[1] == 300000 for p in pages)
    assert reports_of(reports2) == [] and proxy2.day_of("vie")["unvalidated"] == len("www.leboncoin.example")


def test_the_browsers_day_pool(tmp_path, monkeypatch):
    """Spread over hosts, the browser's free bytes meet the agent's day pool (512 KB here); past it they count like a
    program's: the third host's 200 KB goes over, is cut past the hour's 64 KB and reported."""
    proxy, _, reports = load(tmp_path, monkeypatch, AGENTS, dict(LIMITS, browser_free_day=512 * 1024),
                             browsers={"127.0.0.1": "vie"}, env={"BIP_BROWSERS": "127.0.0.1"})
    site = Site()

    async def steps(port):
        results = [await send_tls(port, f"h{i}.example.net", None, 200 * 1024, chunk=3000) for i in range(3)]
        await asyncio.sleep(0.5)
        return results

    results = asyncio.run(through(proxy, monkeypatch, site, steps))
    assert all(r[0].startswith("HTTP/1.1 200") for r in results)
    assert [line["host"] for line in reports_of(reports)] == ["h2.example.net"]   # the third one cut
    assert proxy.day_of("vie")["free"] == 512 * 1024
    assert site.received <= 512 * 1024 + 64 * 1024 + 16 * 1024, site.received


def test_refuser_holds_autoriser_frees(tmp_path, monkeypatch):
    """BV-7 (11/11): the person's « Refuser » reaches the proxy (`refused`, until its week ends): a new connection to
    that host is refused and nothing reaches it; past the date, or on another host, as before. « Autoriser » (the host
    validated, `reset`): the same upload goes whole."""
    import time
    proxy, policy, reports = load(tmp_path, monkeypatch, AGENTS, LIMITS)
    site = Site()

    def rewrite(change, bump):
        data = json.loads(policy.read_text())
        change(data["agents"]["vie"])
        policy.write_text(json.dumps(data))
        os.utime(policy, ns=(bump, bump))

    async def steps(port):
        out = {}
        out["big"] = await send_tls(port, "upload.example.net", VIE, 300 * 1024)
        await asyncio.sleep(0.5)
        rewrite(lambda e: e.update(refused={"upload.example.net": time.time() + 7 * 86400,
                                            "gone.example.net": time.time() - 1}), 10)
        before = site.received
        out["refused"] = await send_tls(port, "upload.example.net", VIE, 1000)
        out["refused_sub"] = await send_tls(port, "www.upload.example.net", VIE, 1000)   # the host only
        out["expired"] = await send_tls(port, "gone.example.net", VIE, 1000)
        await asyncio.sleep(0.3)
        out["passed"] = site.received - before
        rewrite(lambda e: (e.pop("refused"), e["hosts"].append("upload.example.net"),
                           e.update(reset=time.time() + 1)), 20)
        before = site.received
        out["allowed"] = await send_tls(port, "upload.example.net", VIE, 300 * 1024)
        await asyncio.sleep(0.3)
        out["allowed_bytes"] = site.received - before
        return out

    out = asyncio.run(through(proxy, monkeypatch, site, steps))
    assert out["refused"][0] == "HTTP/1.1 403 Sending to this site refused by your person - do without it"
    assert out["refused_sub"][0].startswith("HTTP/1.1 200") and out["expired"][0].startswith("HTTP/1.1 200")
    assert out["passed"] == 2 * (522 + 1005)                    # the two others only, nothing to the refused host
    assert out["allowed"][0].startswith("HTTP/1.1 200") and out["allowed_bytes"] >= 300 * 1024
    assert len(reports_of(reports)) == 1


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


def test_phone_exit_hosts_go_to_the_hub_never_direct(tmp_path, monkeypatch):
    """The agent's `phone` sites go through the hub's unix socket (« EXIT host 443 agent »), counted like any tunnel;
    the minimal route sends a site's static-file hosts direct; the hub's refusal is said (paused: 503, refused: 403);
    no hub means refused, never the datacenter instead."""
    # a short folder: a unix socket's path is at most ~104 bytes (macOS), pytest's tmp_path can be longer
    short = Path(tempfile.mkdtemp(prefix="pe-", dir="/tmp"))
    sock = short / "hub.sock"
    agents = {**AGENTS, "vie": {**AGENTS["vie"], "phone": [{"site": "doctolib.fr", "minimal": False},
                                                           {"site": "leboncoin.fr", "minimal": True}]},
              "conn-c1": {**AGENTS["conn-c1"], "phone": [{"site": "garmin.example", "minimal": False}]}}
    proxy, _, _ = load(tmp_path, monkeypatch, agents, LIMITS, env={"BIP_EXIT_SOCK": str(sock)})
    assert proxy.phone_exit("www.doctolib.fr", "vie") and proxy.phone_exit("doctolib.fr", "vie")
    assert proxy.phone_exit("assets.doctolib.fr", "vie")   # no minimal route for this one
    assert proxy.phone_exit("www.leboncoin.fr", "vie") and proxy.phone_exit("dd.leboncoin.fr", "vie")
    assert not proxy.phone_exit("img.leboncoin.fr", "vie") and not proxy.phone_exit("x.img.leboncoin.fr", "vie")
    assert not proxy.phone_exit("evil-doctolib.fr", "vie") and not proxy.phone_exit("doctolib.fr", None)
    assert not proxy.phone_exit("doctolib.fr", "budget")   # another agent: its own list
    assert not proxy.phone_exit("api.garmin.example", "conn-c1")   # never a connector's credentials
    asked, direct = [], []

    async def hub(reader, writer):
        line = (await reader.readline()).decode().split()
        asked.append(line)
        if line[1] == "paused.doctolib.fr":
            writer.write(b"ERR paused\n")
        elif line[1] == "lan.doctolib.fr":
            writer.write(b"ERR private\n")
        else:
            writer.write(b"OK\n")
            await writer.drain()
            data = await reader.read(100)
            writer.write(b"echo:" + data)
        await writer.drain()
        writer.close()

    async def no_direct(addresses, port):
        direct.append(addresses)
        raise OSError("must not go direct")

    async def steps():
        hub_server = await asyncio.start_unix_server(hub, str(sock))
        monkeypatch.setattr(proxy, "connect", no_direct)
        server = await asyncio.start_server(proxy.handle, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        out = {}
        status, reader, writer = await tunnel(port, "www.doctolib.fr", VIE)
        writer.write(b"hello")
        await writer.drain()
        out["ok"] = (status, await asyncio.wait_for(reader.read(100), 5))
        writer.close()
        out["paused"] = (await tunnel(port, "paused.doctolib.fr", VIE))[0]
        out["private"] = (await tunnel(port, "lan.doctolib.fr", VIE))[0]
        hub_server.close()
        await hub_server.wait_closed()
        if sock.exists():
            sock.unlink()
        out["gone"] = (await tunnel(port, "www.doctolib.fr", VIE))[0]
        # plain HTTP to a phone host: refused (the phone carries TLS only)
        r, w = await asyncio.open_connection("127.0.0.1", port)
        w.write(f"GET http://www.doctolib.fr/ HTTP/1.1\r\nHost: www.doctolib.fr\r\nProxy-Authorization: {VIE}\r\n\r\n"
                .encode())
        await w.drain()
        out["http"] = (await asyncio.wait_for(r.readuntil(b"\r\n\r\n"), 5)).split(b"\r\n")[0].decode()
        w.close()
        server.close()
        return out

    out = asyncio.run(steps())
    assert out["ok"] == ("HTTP/1.1 200 Connection established", b"echo:hello")
    assert asked[0] == ["EXIT", "www.doctolib.fr", "443", "vie"]
    assert out["paused"].startswith("HTTP/1.1 503 Phone exit paused")
    assert out["private"].startswith("HTTP/1.1 403 Site refused by the phone exit")
    assert out["gone"].startswith("HTTP/1.1 503 Phone exit paused")
    assert out["http"].startswith("HTTP/1.1 403")
    assert direct == []
    shutil.rmtree(short, ignore_errors=True)


def test_a_site_switched_to_the_phone_closes_its_direct_tunnels(tmp_path, monkeypatch):
    """Chrome reuses an open connection: once the person accepted the card, the site's tunnels still open direct are
    closed (and the phone's once the session ended), so the next page goes the new way (lab, 6/11)."""
    proxy, _, _ = load(tmp_path, monkeypatch, AGENTS, LIMITS, env={"BIP_EXIT_SOCK": "/nonexistent/hub.sock"})
    policy = tmp_path / "sites.json"

    class Writer:
        closed = False

        def close(self):
            self.closed = True

    def rewrite(agents, bump):
        policy.write_text(json.dumps({"agents": agents, "limits": LIMITS}))
        stamp = os.stat(policy).st_mtime_ns + bump
        os.utime(policy, ns=(stamp, stamp))

    direct, other, phone = [Writer(), Writer()], [Writer(), Writer()], [Writer(), Writer()]
    proxy._open_tunnels[:] = [["vie", "www.ameli.fr", False, *direct], ["vie", "www.doctolib.fr", False, *other]]
    proxy.switch_tunnels()
    assert not direct[0].closed and len(proxy._open_tunnels) == 2   # nothing changed yet
    rewrite({**AGENTS, "vie": {**AGENTS["vie"], "phone": [{"site": "ameli.fr", "minimal": True}]}}, 1_000_000)
    proxy._open_tunnels.append(["vie", "www.ameli.fr", True, *phone])
    proxy.switch_tunnels()
    assert direct[0].closed and direct[1].closed and not other[0].closed and not phone[0].closed
    assert [e[1:3] for e in proxy._open_tunnels] == [["www.doctolib.fr", False], ["www.ameli.fr", True]]
    # the session over: its phone tunnels closed too
    rewrite(AGENTS, 2_000_000)
    proxy.switch_tunnels()
    assert phone[0].closed and not other[0].closed
