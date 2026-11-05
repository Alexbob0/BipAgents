"""The phone exit, phase 0 (bridge-rs/src/phone_exit.rs, prototype behind `[phone_exit] enabled`): the phone's
tunnel (`GET /v1/exit/tunnel`, its debug token or the bridge key), the agents' proxy's side (the hub's unix socket,
« EXIT host 443 agent »), streams carried byte for byte within their windows, every refusal before the phone is asked
(not listed, an address, a local name, another port, no phone: never the datacenter instead), the phone's own refusal,
the ClientHello's server name, the session's caps (idle, bytes) and the kill switch. One log line per stream: host and
bytes, never content."""
import asyncio
import hashlib
import os

import httpx
import pytest

from bbkit.common import AUTH, BRIDGE_KEY
from bbkit.phone_exit import FakePhone, client_hello, echo_server, exit_stream
from bbkit.ws import InvalidStatus, open_ws

TOKEN = "phone-debug-token-" + "p" * 24
HOSTS = ["site.phone.example", "*.cdn.phone.example", "localhost", "10.0.0.1", "nas.local"]


def exit_bridge(bridges, **extra):
    bridge = bridges.new()
    bridge.config["phone_exit"] = {"enabled": True, "hosts": HOSTS, "token_sha256": hashlib.sha256(TOKEN.encode()).hexdigest(),
                                   **extra}
    return bridge.start()


def sock(bridge) -> str:
    return str(bridge.state_dir / "phone-exit" / "hub.sock")


async def with_phone(bridge, steps, judge=lambda h, p: None, token=TOKEN):
    """Runs `steps(phone, site_port, received)` with a fake phone connected and a local echo server as every site."""
    server, port, got = await echo_server()
    phone = FakePhone(bridge.ws_url("/v1/exit/tunnel"), token, lambda h, p: ("127.0.0.1", port), judge)
    task = asyncio.ensure_future(phone.run())
    await asyncio.wait_for(phone.ready.wait(), 10)
    await asyncio.sleep(0.2)   # the hub has taken the tunnel
    try:
        return await steps(phone, got)
    finally:
        task.cancel()
        server.close()


def test_off_by_default(bridge):
    assert bridge.get("/v1/exit/tunnel", headers=AUTH).status_code == 404
    assert not (bridge.state_dir / "phone-exit").exists()
    assert bridge.get("/v1/exit/session", headers=AUTH).json()["enabled"] is False


def test_the_tunnel_needs_its_token(bridges):
    bridge = exit_bridge(bridges)
    for wrong in (None, "nope", BRIDGE_KEY[:-1]):
        with pytest.raises(InvalidStatus) as refused:
            open_ws(bridge.ws_url("/v1/exit/tunnel"), {"Authorization": f"Bearer {wrong}"} if wrong else {})
        assert refused.value.response.status_code == 401
    for good in (TOKEN, BRIDGE_KEY):
        with open_ws(bridge.ws_url("/v1/exit/tunnel"), {"Authorization": f"Bearer {good}"}):
            pass
    assert bridge.get("/v1/exit/session").status_code == 401   # the state and the kill switch: the bridge key


def test_a_stream_crosses_the_phone_byte_for_byte(bridges):
    bridge = exit_bridge(bridges)
    payload = os.urandom(1_500_000)   # past the windows both ways

    async def steps(phone, got):
        line, reader, writer = await exit_stream(sock(bridge), "site.phone.example")
        assert line == "OK"
        state = httpx.get(f"{bridge.url}/v1/exit/session", headers=AUTH).json()
        assert state["connected"] and state["device"] == "fake" and state["streams"] == 1
        hello = client_hello("site.phone.example")
        writer.write(hello + payload)
        await writer.drain()
        back = await asyncio.wait_for(reader.readexactly(len(hello) + len(payload)), 30)
        writer.write_eof()
        assert await asyncio.wait_for(reader.read(), 10) == b""
        writer.close()
        await asyncio.sleep(0.3)
        after = httpx.get(f"{bridge.url}/v1/exit/session", headers=AUTH).json()
        assert after["up"] >= 1_500_000 and after["down"] >= 1_500_000 and after["opened"] == 1, after
        assert after["streams"] == 0
        return back

    back = asyncio.run(with_phone(bridge, steps))
    assert back[-len(payload):] == payload and back.startswith(b"\x16\x03\x01")
    lines = bridge.logged("phone exit stream")
    assert len(lines) == 1, bridge.log_tail(30)
    line = lines[0]
    assert line["host"] == "site.phone.example" and line["agent"] == "vie"
    assert line["up"] >= 1_500_000 and line["down"] >= 1_500_000 and line["ended"] == "fin/fin"
    assert payload[:64].hex() not in bridge.log_path.read_text(errors="replace")


def test_refused_before_the_phone_is_asked(bridges):
    bridge = exit_bridge(bridges)

    async def steps(phone, got):
        out = {}
        for host, port in [("other.example", 443), ("10.0.0.1", 443), ("192.168.1.1", 443), ("[::1]", 443),
                           ("64:ff9b::c0a8:101", 443), ("::ffff:192.168.1.1", 443), ("2130706433", 443),
                           ("localhost", 443), ("nas.local", 443), ("site.phone.example", 80),
                           ("cdn.phone.example", 443)]:
            line, _, writer = await exit_stream(sock(bridge), host, port)
            writer.close()
            out[f"{host}:{port}"] = line
        return out, list(phone.opens)

    out, opens = asyncio.run(with_phone(bridge, steps))
    assert out == {
        "other.example:443": "ERR not-allowed", "10.0.0.1:443": "ERR literal-ip", "192.168.1.1:443": "ERR literal-ip",
        "[::1]:443": "ERR literal-ip", "64:ff9b::c0a8:101:443": "ERR literal-ip",
        "::ffff:192.168.1.1:443": "ERR literal-ip", "2130706433:443": "ERR literal-ip",
        "localhost:443": "ERR local-name", "nas.local:443": "ERR local-name", "site.phone.example:80": "ERR port",
        "cdn.phone.example:443": "ERR not-allowed",   # `*.cdn…` is its subdomains only
    }
    assert opens == []   # the phone was never asked
    reasons = {(e["host"], e["reason"]) for e in bridge.logged("phone exit refused")}
    assert ("10.0.0.1", "literal-ip") in reasons and ("other.example", "not-allowed") in reasons


def test_no_phone_never_the_datacenter(bridges):
    bridge = exit_bridge(bridges)

    async def steps():
        line, _, writer = await exit_stream(sock(bridge), "site.phone.example")
        writer.close()
        return line

    assert asyncio.run(steps()) == "ERR paused"
    assert bridge.logged("phone exit refused")[-1]["reason"] == "paused"


def test_the_phone_has_the_last_word_and_the_server_name_must_match(bridges):
    bridge = exit_bridge(bridges)

    def judge(host, port):
        return "private" if host.startswith("lan.") else None

    async def steps(phone, got):
        refused, _, w1 = await exit_stream(sock(bridge), "lan.cdn.phone.example")
        w1.close()
        line, reader, writer = await exit_stream(sock(bridge), "www.cdn.phone.example")
        assert line == "OK"
        # a ClientHello naming another site on the same CDN: cut before anything reaches it
        writer.write(client_hello("bank.example") + b"secret request")
        await writer.drain()
        cut = await asyncio.wait_for(reader.read(), 10)
        writer.close()
        _, r3, w3 = await exit_stream(sock(bridge), "www.cdn.phone.example")
        w3.write(client_hello(None) + b"x")   # no server name at all
        await w3.drain()
        cut_none = await asyncio.wait_for(r3.read(), 10)
        w3.close()
        await asyncio.sleep(0.3)
        return refused, cut, cut_none, bytes(got), list(phone.refused)

    refused, cut, cut_none, got, phone_refused = asyncio.run(with_phone(bridge, steps, judge))
    assert refused == "ERR private" and ("lan.cdn.phone.example", "private") in phone_refused
    assert cut == b"" and cut_none == b"" and got == b""
    reasons = [e["reason"] for e in bridge.logged("phone exit refused")]
    assert "phone:private" in reasons and "sni-mismatch" in reasons and "sni-missing" in reasons


def test_idle_bytes_and_the_kill_switch_end_the_session(bridges):
    bridge = exit_bridge(bridges, idle_minutes=0.05, max_mb=0.3)

    async def idle(phone, got):
        await asyncio.wait_for(phone.closed.wait(), 15)
        return phone.bye

    assert asyncio.run(with_phone(bridge, idle)) == "idle"

    async def too_much(phone, got):
        line, reader, writer = await exit_stream(sock(bridge), "site.phone.example")
        assert line == "OK"
        writer.write(client_hello("site.phone.example") + os.urandom(400_000))
        await writer.drain()
        await asyncio.wait_for(phone.closed.wait(), 15)
        writer.close()
        return phone.bye

    assert asyncio.run(with_phone(bridge, too_much)) == "limit"

    async def stopped(phone, got):
        answer = httpx.delete(f"{bridge.url}/v1/exit/session", headers=AUTH).json()
        await asyncio.wait_for(phone.closed.wait(), 10)
        return answer, phone.bye

    assert asyncio.run(with_phone(bridge, stopped)) == ({"stopped": True}, "user-stop")
    closed = [e["ended"] for e in bridge.logged("phone exit: tunnel closed")]
    assert closed == ["idle", "limit", "user-stop"], bridge.log_tail(30)
