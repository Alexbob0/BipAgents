"""The phone exit (bridge-rs/src/phone_exit/, docs/phone-exit.md protocol v1, behind `[phone_exit] enabled`): the card
« Passer par mon téléphone pour <site> ? » (the agent's `ask_phone_exit`, or the browser tool's `ASK` on a challenge
page), its answers (once, always, no), the session the phone then carries (HELLO / WELCOME), streams carried byte for
byte within their windows, every refusal before the phone is asked (no session, an address, a local name, another
port, no phone: never the datacenter instead), the refusal's reason kept for the browser tool (`WHY`), the phone's own
refusal and pause (Wi-Fi only), the ClientHello's server name, Chrome's preconnects, the limits per session across
reconnections (idle, time), the kill switches (Arrêter on the phone, Settings, the bridge), the journal and the Boîte."""
import asyncio
import hashlib
import json
import os
import time

import httpx
import pytest

from bbkit.common import AUTH, BRIDGE_KEY, boite_items
from bbkit.mcp import call, give_token, rpc, text_of
from bbkit.phone_exit import END, PAUSE, RESUME, SESSIONS, FakePhone, client_hello, echo_server, exit_stream, hub_line
from bbkit.ws import InvalidStatus, open_ws

TOKEN = "phone-debug-token-" + "p" * 24
SITE = "phone.example"
HOST = "site.phone.example"


def exit_bridge(bridges, name="bridge", **extra):
    bridge = bridges.new(name)
    bridge.config["phone_exit"] = {"enabled": True, "token_sha256": hashlib.sha256(TOKEN.encode()).hexdigest(),
                                   "start_wait_seconds": 0.5, **extra}
    bridge.start()
    give_token(bridge)
    return bridge


def sock(bridge) -> str:
    return str(bridge.state_dir / "phone-exit" / "hub.sock")


def start_session(bridge, sites=(SITE,), agent="wellness") -> dict:
    answer = bridge.post("/v1/exit/sessions", headers=AUTH, json={"agent": agent, "sites": list(sites)})
    assert answer.status_code == 200, answer.text
    return answer.json()["session"]


def cards(bridge) -> list:
    return [c for c in bridge.get("/v1/approvals", headers=AUTH).json()["items"] if c["run_id"].startswith("bip-exit-")]


def wait_for(check, timeout=10.0):
    end = time.time() + timeout
    while time.time() < end:
        found = check()
        if found:
            return found
        time.sleep(0.1)
    return check()


async def with_phone(bridge, steps, sessions, judge=lambda h, p: None, token=TOKEN):
    """Runs `steps(phone, received)` with a fake phone carrying `sessions` and a local echo server as every site."""
    server, port, got = await echo_server()
    phone = FakePhone(bridge.ws_url("/v1/exit/tunnel"), token, lambda h, p: ("127.0.0.1", port), judge,
                      sessions=sessions)
    task = asyncio.ensure_future(phone.run())
    await asyncio.wait_for(phone.ready.wait(), 10)
    await asyncio.wait_for(phone.welcomed.wait(), 10)
    try:
        return await steps(phone, got)
    finally:
        task.cancel()
        server.close()


async def carry(bridge, size=0):
    """One stream to HOST through the hub; the bytes echoed back."""
    line, reader, writer = await exit_stream(sock(bridge), HOST)
    assert line == "OK", line
    hello = client_hello(HOST)
    payload = os.urandom(size)
    writer.write(hello + payload)
    await writer.drain()
    back = await asyncio.wait_for(reader.readexactly(len(hello) + size), 30)
    writer.write_eof()
    await asyncio.wait_for(reader.read(), 10)
    writer.close()
    return back[len(hello):] == payload


def test_off_by_default(bridge):
    assert bridge.get("/v1/exit/tunnel", headers=AUTH).status_code == 404
    assert not (bridge.state_dir / "phone-exit").exists()
    assert bridge.get("/v1/exit/session", headers=AUTH).json()["enabled"] is False
    assert bridge.get("/v1/exit", headers=AUTH).json()["enabled"] is False
    assert bridge.post("/v1/exit/sessions", headers=AUTH, json={"agent": "wellness", "sites": [SITE]}).status_code == 404
    give_token(bridge)
    names = [t["name"] for t in rpc(bridge, "tools/list").json()["result"]["tools"]]
    assert "ask_phone_exit" not in names


def test_the_tunnel_needs_its_key_and_a_session(bridges):
    bridge = exit_bridge(bridges)
    for wrong in (None, "nope", BRIDGE_KEY[:-1]):
        with pytest.raises(InvalidStatus) as refused:
            open_ws(bridge.ws_url("/v1/exit/tunnel"), {"Authorization": f"Bearer {wrong}"} if wrong else {})
        assert refused.value.response.status_code == 401
    assert bridge.get("/v1/exit/session").status_code == 401   # the state and the kill switch: the bridge key

    async def no_session(token):
        phone = FakePhone(bridge.ws_url("/v1/exit/tunnel"), token, lambda h, p: ("127.0.0.1", 9),
                          sessions=["unknown-session"])
        await asyncio.wait_for(phone.run(), 10)
        return phone.welcomes, phone.bye

    for good in (TOKEN, BRIDGE_KEY):
        welcomes, bye = asyncio.run(no_session(good))
        assert welcomes == [{"v": 1, "sessions": [], "refused": [{"id": "unknown-session", "why": "unknown"}]}]
        assert bye == "no_session"


def test_the_card_flow(bridges):
    """The agent asks; the card; « once » makes the session the phone carries; « always » sets the site; « Non »
    makes nothing; the tool says what happened and never lets the agent retry blindly."""
    bridge = exit_bridge(bridges)
    names = [t["name"] for t in rpc(bridge, "tools/list").json()["result"]["tools"]]
    assert "ask_phone_exit" in names
    asked = call(bridge, "ask_phone_exit", {"site": f"https://www.{SITE}/espace?x=1", "reason": "Cloudflare"})
    assert not asked.get("isError") and "got a card" in text_of(asked), asked
    again = call(bridge, "ask_phone_exit", {"site": f"www.{SITE}"})
    assert "already has a card" in text_of(again)
    [card] = cards(bridge)
    assert card["agent"] == "wellness" and card["choices"] == ["once", "always", "deny"]
    assert card["bip_exit"] == {"site": SITE, "host": f"www.{SITE}", "reason": "Cloudflare", "kind": "ask",
                                "wifi_only": False}
    assert SITE in card["description"]
    answer = bridge.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": card["run_id"],
                                                             "choice": "once", "request_id": card["request_id"]})
    assert answer.status_code == 200, answer.text
    body = answer.json()
    session = body["exit"]["session"]
    assert body["choice"] == "once" and body["exit"]["tunnel"] == "/v1/exit/tunnel"
    assert session["agent"] == "wellness" and session["sites"] == [SITE] and session["state"] == "accepted"
    assert session["used_bytes"] == 0 and session["max_minutes"] == 120 and "cap_mb" not in session
    assert cards(bridge) == []
    # answered already
    assert bridge.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": card["run_id"],
                                                           "choice": "once"}).status_code == 404
    # the tool now knows: a session covers the site (not relaying yet)
    assert "not relaying" in text_of(call(bridge, "ask_phone_exit", {"site": SITE}))
    # the decision is kept with the person's others
    log = (bridge.state_dir / "approvals" / "wellness.jsonl").read_text()
    assert card["run_id"] in log and '"once"' in log

    async def steps(phone, got):
        assert phone.welcomes[0]["sessions"][0]["id"] == session["id"]
        assert phone.welcomes[0]["sessions"][0]["state"] == "active"
        return await carry(bridge, 200_000)

    assert asyncio.run(with_phone(bridge, steps, [session["id"]]))
    # « Toujours pour ce site » on another card; « Non » on a third
    call(bridge, "ask_phone_exit", {"site": "other.example"})
    call(bridge, "ask_phone_exit", {"site": "third.example"})
    other, third = sorted(cards(bridge), key=lambda c: c["command"])
    ok = bridge.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": other["run_id"],
                                                         "choice": "always"}).json()
    assert ok["choice"] == "always" and ok["exit"]["session"]["sites"] == ["other.example"]
    no = bridge.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": third["run_id"],
                                                         "choice": "deny"}).json()
    assert no == {"ok": True, "choice": "deny"}
    overview = bridge.get("/v1/exit", headers=AUTH).json()
    assert overview["sites"] == [{"site": "other.example", "mode": "always", "minimal": True,
                                  "at": overview["sites"][0]["at"]}]
    assert {s["sites"][0] for s in overview["sessions"]} == {SITE, "other.example"}
    # « Jamais »: no card, the agent told to hand over
    bridge.put("/v1/exit/sites/never.example", headers=AUTH, json={"mode": "never"})
    assert "never to use their phone" in text_of(call(bridge, "ask_phone_exit", {"site": "never.example"}))
    # done with a site: its session ends
    assert "no longer goes through" in text_of(call(bridge, "ask_phone_exit", {"site": SITE, "done": True}))
    journal = bridge.get("/v1/exit/journal", headers=AUTH).json()["sessions"]
    assert journal[0]["id"] == session["id"] and journal[0]["ended"] == "task_done"
    assert journal[0]["used_bytes"] >= 400_000 and journal[0]["hosts"][HOST] >= 400_000
    # never an address or a local name
    assert "not a site name" in text_of(call(bridge, "ask_phone_exit", {"site": "192.168.1.1"}))


def test_a_stream_crosses_the_phone_byte_for_byte(bridges):
    bridge = exit_bridge(bridges)
    session = start_session(bridge)
    payload = os.urandom(1_500_000)   # past the windows both ways

    async def steps(phone, got):
        line, reader, writer = await exit_stream(sock(bridge), HOST)
        assert line == "OK"
        state = httpx.get(f"{bridge.url}/v1/exit/session", headers=AUTH).json()
        assert state["connected"] and state["device"] == "fake" and state["streams"] == 1
        assert state["sessions"] == [session["id"]]
        assert phone.opens[0] == {"host": HOST, "port": 443, "agent": "wellness", "session": session["id"]}
        hello = client_hello(HOST)
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
        await asyncio.sleep(5.5)   # USAGE every 5 s, for the notification's MB
        return back, phone.usages

    back, usages = asyncio.run(with_phone(bridge, steps, [session["id"]]))
    assert back[-len(payload):] == payload and back.startswith(b"\x16\x03\x01")
    assert usages and usages[-1]["sessions"][0]["id"] == session["id"]
    assert usages[-1]["sessions"][0]["used_bytes"] >= 3_000_000
    line = [e for e in bridge.logged("phone exit stream") if e["ended"] != "closed_before_hello"][0]
    assert line["host"] == HOST and line["agent"] == "wellness" and line["session"] == session["id"]
    assert line["up"] >= 1_500_000 and line["down"] >= 1_500_000 and line["ended"] == "fin/fin"
    assert payload[:64].hex() not in bridge.log_path.read_text(errors="replace")


def test_refused_before_the_phone_is_asked(bridges):
    bridge = exit_bridge(bridges)
    session = start_session(bridge)

    async def steps(phone, got):
        out = {}
        for host, port, agent in [
                ("other.example", 443, "wellness"), ("10.0.0.1", 443, "wellness"), ("192.168.1.1", 443, "wellness"),
                ("[::1]", 443, "wellness"), ("64:ff9b::c0a8:101", 443, "wellness"),
                ("::ffff:192.168.1.1", 443, "wellness"), ("2130706433", 443, "wellness"),
                ("localhost", 443, "wellness"), ("nas.local", 443, "wellness"), (HOST, 80, "wellness"),
                (HOST, 443, "someone-else"), ("evil-phone.example", 443, "wellness")]:
            line, _, writer = await exit_stream(sock(bridge), host, port, agent)
            writer.close()
            out[f"{host}:{port}:{agent}"] = line
        return out, list(phone.opens)

    out, opens = asyncio.run(with_phone(bridge, steps, [session["id"]]))
    assert out == {
        "other.example:443:wellness": "ERR not_allowed", "10.0.0.1:443:wellness": "ERR literal_ip",
        "192.168.1.1:443:wellness": "ERR literal_ip", "[::1]:443:wellness": "ERR literal_ip",
        "64:ff9b::c0a8:101:443:wellness": "ERR literal_ip", "::ffff:192.168.1.1:443:wellness": "ERR literal_ip",
        "2130706433:443:wellness": "ERR literal_ip", "localhost:443:wellness": "ERR local_name",
        "nas.local:443:wellness": "ERR local_name", f"{HOST}:80:wellness": "ERR port",
        f"{HOST}:443:someone-else": "ERR not_allowed",   # not an agent of this place
        "evil-phone.example:443:wellness": "ERR not_allowed",
    }
    assert opens == []   # the phone was never asked
    reasons = {(e["host"], e["reason"]) for e in bridge.logged("phone exit refused")}
    assert ("10.0.0.1", "literal_ip") in reasons and ("other.example", "not_allowed") in reasons


def test_no_phone_never_the_datacenter_and_the_agent_told_why(bridges):
    """A session accepted but no phone relaying: refused `paused` (after a short wait for a phone starting), the
    reason kept for the browser tool; a « Toujours » site with no session makes a card to restart the relay."""
    bridge = exit_bridge(bridges)
    start_session(bridge)
    bridge.put("/v1/exit/sites/always.example", headers=AUTH, json={"mode": "always"})

    async def steps():
        line, _, writer = await exit_stream(sock(bridge), HOST)
        writer.close()
        why = await hub_line(sock(bridge), f"WHY wellness www.{SITE}")
        none = await hub_line(sock(bridge), "WHY wellness www.elsewhere.example")
        always, _, w2 = await exit_stream(sock(bridge), "www.always.example")
        w2.close()
        return line, why, none, always

    line, why, none, always = asyncio.run(steps())
    assert line == "ERR paused" and why.split()[0] == "paused" and none == "NONE"
    assert always == "ERR paused"
    [card] = cards(bridge)
    assert card["bip_exit"]["kind"] == "restart" and card["choices"] == ["once", "deny"]
    assert card["bip_exit"]["site"] == "always.example"
    assert "Toujours" in card["description"] or "Always" in card["description"]
    assert bridge.logged("phone exit refused")[-1]["reason"] == "paused"


def test_a_challenge_page_asks_the_person(bridges):
    """The browser tool's `ASK` (a challenge page): a card once, then « waiting »; « live » when a session covers it;
    « never » when the person said Jamais; never for an agent that is not the person's own."""
    bridge = exit_bridge(bridges)

    async def steps():
        first = await hub_line(sock(bridge), "ASK wellness www.blocked.example DataDome")
        second = await hub_line(sock(bridge), "ASK wellness www.blocked.example DataDome")
        stranger = await hub_line(sock(bridge), "ASK someone-else www.blocked.example DataDome")
        literal = await hub_line(sock(bridge), "ASK wellness 192.168.1.1 DataDome")
        return first, second, stranger, literal

    assert asyncio.run(steps()) == ("CARD", "WAITING", "NO not_own", "NO literal_ip")
    [card] = cards(bridge)
    assert card["bip_exit"]["reason"] == "DataDome" and card["bip_exit"]["site"] == "blocked.example"
    bridge.put("/v1/exit/sites/blocked.example", headers=AUTH, json={"mode": "never"})
    assert cards(bridge) == []   # « Jamais » withdraws its card
    start_session(bridge, ["live.example"])

    async def more():
        return (await hub_line(sock(bridge), "ASK wellness www.blocked.example DataDome"),
                await hub_line(sock(bridge), "ASK wellness www.live.example DataDome"))

    assert asyncio.run(more()) == ("NO never", "LIVE")


def test_the_phone_has_the_last_word_and_the_server_name_must_match(bridges):
    bridge = exit_bridge(bridges)
    session = start_session(bridge)

    def judge(host, port):
        return "private" if host.startswith("lan.") else None

    async def steps(phone, got):
        refused, _, w1 = await exit_stream(sock(bridge), f"lan.{SITE}")
        w1.close()
        why = await hub_line(sock(bridge), f"WHY wellness lan.{SITE}")
        # a tracker refused (`private`) is not a reason for the site's other pages
        assert await hub_line(sock(bridge), f"WHY wellness {HOST}") == "NONE"
        line, reader, writer = await exit_stream(sock(bridge), HOST)
        assert line == "OK"
        # a ClientHello naming another site on the same CDN: cut before anything reaches it
        writer.write(client_hello("bank.example") + b"secret request")
        await writer.drain()
        cut = await asyncio.wait_for(reader.read(), 10)
        writer.close()
        _, r3, w3 = await exit_stream(sock(bridge), HOST)
        w3.write(client_hello(None) + b"x")   # no server name at all
        await w3.drain()
        cut_none = await asyncio.wait_for(r3.read(), 10)
        w3.close()
        # Chrome's preconnect: opened, closed before its ClientHello: not a refusal
        pre, _, w4 = await exit_stream(sock(bridge), HOST)
        w4.close()
        await asyncio.sleep(0.5)
        return refused, why, cut, cut_none, pre, bytes(got), list(phone.refused)

    refused, why, cut, cut_none, pre, got, phone_refused = asyncio.run(with_phone(bridge, steps, [session["id"]], judge))
    assert refused == "ERR private" and why.split()[0] == "private" and (f"lan.{SITE}", "private") in phone_refused
    assert cut == b"" and cut_none == b"" and got == b"" and pre == "OK"
    reasons = [e["reason"] for e in bridge.logged("phone exit refused")]
    assert "phone:private" in reasons and "sni_mismatch" in reasons and "sni_missing" in reasons
    assert "not_tls" not in reasons   # the preconnect is not one
    assert [e for e in bridge.logged("phone exit stream") if e["ended"] == "closed_before_hello"]


def test_wifi_only_pause_and_the_phone_s_stop(bridges):
    bridge = exit_bridge(bridges)
    session = start_session(bridge)

    async def steps(phone, got):
        await phone.control(PAUSE, {"why": "wifi_only"})
        await asyncio.sleep(0.3)
        paused, _, w = await exit_stream(sock(bridge), HOST)
        w.close()
        state = httpx.get(f"{bridge.url}/v1/exit/sessions", headers=AUTH).json()["sessions"][0]["state"]
        await phone.control(RESUME, {})
        await asyncio.sleep(0.3)
        resumed = await carry(bridge, 1000)
        # « Arrêter » on the phone: END for its session, then the tunnel has nothing left
        await phone.control(END, {"id": session["id"], "why": "user_stop"})
        await asyncio.wait_for(phone.closed.wait(), 10)
        return paused, state, resumed, phone.bye

    paused, state, resumed, bye = asyncio.run(with_phone(bridge, steps, [session["id"]]))
    assert paused == "ERR wifi_only" and state == "paused" and resumed and bye == "no_session"
    [ended] = bridge.get("/v1/exit/journal", headers=AUTH).json()["sessions"]
    assert ended["ended"] == "user_stop" and ended["network"] == "wifi"


def test_limits_follow_the_session_across_reconnections(bridges):
    """The MB and the time go on when the phone reconnects (a network change); idle and time end the session, not
    the socket; a SESSIONS frame adds one while relaying."""
    bridge = exit_bridge(bridges)
    session = start_session(bridge)

    async def first(phone, got):
        return await carry(bridge, 300_000)

    assert asyncio.run(with_phone(bridge, first, [session["id"]]))

    async def again(phone, got):
        used = phone.welcomes[0]["sessions"][0]["used_bytes"]
        second = start_session(bridge, ["second.example"])
        await phone.control(SESSIONS, {"sessions": [session["id"], second["id"]]})
        await asyncio.sleep(0.3)
        return used, [s["id"] for s in phone.welcomes[-1]["sessions"]], second

    used, carried, second = asyncio.run(with_phone(bridge, again, [session["id"]]))
    assert used >= 600_000 and carried == [session["id"], second["id"]]
    # the phone gone: its sessions wait for it, not ended
    assert wait_for(lambda: {s["state"] for s in bridge.get("/v1/exit/sessions", headers=AUTH).json()["sessions"]}
                    == {"paused"})


def test_idle_and_time_end_the_session(bridges):
    bridge = exit_bridge(bridges, idle_minutes=0.05)
    session = start_session(bridge)

    async def idle(phone, got):
        await asyncio.wait_for(phone.closed.wait(), 15)
        return phone.ends, phone.bye

    ends, bye = asyncio.run(with_phone(bridge, idle, [session["id"]]))
    assert ends == [{"id": session["id"], "why": "idle"}] and bye == "no_session"

    timed = exit_bridge(bridges, "timed", max_minutes=0.05)
    session = start_session(timed)

    async def expired(phone, got):
        while not phone.closed.is_set():
            try:
                await carry(timed, 1000)   # busy all along: the time limit still ends it
            except AssertionError:
                break   # refused: it ended between two pages
            await asyncio.sleep(0.2)
        await asyncio.wait_for(phone.closed.wait(), 10)
        return phone.ends

    assert asyncio.run(with_phone(timed, expired, [session["id"]]))[0]["why"] == "expired"

    async def after():
        line, _, w = await exit_stream(sock(timed), HOST)
        w.close()
        return line

    assert asyncio.run(after()) == "ERR expired"   # said, not « paused »


def test_the_kill_switches_the_journal_and_the_boite(bridges):
    bridge = exit_bridge(bridges)
    one, two = start_session(bridge), start_session(bridge, ["second.example"])

    async def stopped(phone, got):
        await carry(bridge, 5000)
        stopped_one = httpx.delete(f"{bridge.url}/v1/exit/sessions/{two['id']}", headers=AUTH).json()
        await asyncio.sleep(0.3)
        answer = httpx.delete(f"{bridge.url}/v1/exit/session", headers=AUTH).json()
        await asyncio.wait_for(phone.closed.wait(), 10)
        return stopped_one, answer, phone.ends, phone.bye

    stopped_one, answer, ends, bye = asyncio.run(with_phone(bridge, stopped, [one["id"], two["id"]]))
    assert stopped_one == {"stopped": True} and answer["stopped"] is True
    assert ends == [{"id": two["id"], "why": "user_stop"}, {"id": one["id"], "why": "user_stop"}]
    assert bye in ("no_session", "user_stop")
    journal = bridge.get("/v1/exit/journal", headers=AUTH).json()["sessions"]
    assert [j["ended"] for j in journal] == ["user_stop", "user_stop"]
    # « Autoriser » off: every session ends `off`, streams refused `off`
    start_session(bridge)
    bridge.put("/v1/exit/settings", headers=AUTH, json={"allowed": False})
    assert bridge.get("/v1/exit/journal", headers=AUTH).json()["sessions"][0]["ended"] == "off"
    assert bridge.post("/v1/exit/sessions", headers=AUTH, json={"agent": "wellness", "sites": [SITE]}).status_code == 409
    settings = bridge.put("/v1/exit/settings", headers=AUTH, json={"allowed": True, "wifi_only": True}).json()["settings"]
    assert settings == {"allowed": True, "wifi_only": True, "minimal_route": True}
    assert bridge.put("/v1/exit/settings", headers=AUTH, json={"wifi_only": "yes"}).status_code == 422
    # one Boîte line per session (silent), in the agent's language
    lines = wait_for(lambda: [i for i in boite_items(bridge) if "phone.example" in i["text"]
                              or "second.example" in i["text"]], 10)
    assert len(lines) >= 3, boite_items(bridge)
    carried = [i for i in lines if " · " in i["text"] and "phone.example" in i["text"]][0]["text"]
    assert ("Mo" in carried or "MB" in carried) and "min" in carried


def test_browsers_set_up_before_learn_where_the_hub_is(bridges):
    """An agent's browser tool set up before the phone exit was switched on (its mcp.json) is told the hub's socket
    at the bridge's start, so it can tell the agent why a page failed."""
    bridge = bridges.new()
    config = bridge.state_dir / "browsers" / "wellness" / "mcp.json"
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text(json.dumps({"server": ["true"], "busy": "/nonexistent"}))
    bridge.config["phone_exit"] = {"enabled": True}
    bridge.start()
    written = wait_for(lambda: json.loads(config.read_text()).get("exit_sock"))
    assert written == sock(bridge)
    assert json.loads(config.read_text())["agent"] == "wellness"
    assert json.loads(config.read_text())["server"] == ["true"]   # the rest kept


def live_sessions(bridge, **query) -> list:
    return bridge.get("/v1/exit/sessions", headers=AUTH, params=query).json()["sessions"]


def test_a_pause_outlives_the_tunnel_until_the_phone_comes_back(bridges):
    """An iPhone put away says `PAUSE background`, then closes its tunnel (no BYE): its session stays `paused` with
    that reason (streams refused at once, never direct) until the phone comes back; « Seulement en Wi-Fi » on mobile
    data the same, refused `wifi_only`."""
    bridge = exit_bridge(bridges)
    away, wifi = start_session(bridge), start_session(bridge, ["second.example"])

    async def put_away(phone, got):
        await phone.control(PAUSE, {"why": "background"})
        await asyncio.sleep(0.3)
        await phone.ws.close()
        await asyncio.wait_for(phone.closed.wait(), 10)
        await asyncio.sleep(0.5)
        started = time.time()
        line, _, w = await exit_stream(sock(bridge), HOST)
        w.close()
        return line, time.time() - started

    line, took = asyncio.run(with_phone(bridge, put_away, [away["id"]]))
    [kept] = [s for s in live_sessions(bridge) if s["id"] == away["id"]]
    assert kept["state"] == "paused" and kept["paused_why"] == "background", kept
    assert line == "ERR paused" and took < 2.0

    async def mobile_data(phone, got):
        await phone.control(PAUSE, {"why": "wifi_only"})
        await asyncio.sleep(0.3)
        await phone.ws.close()
        await asyncio.wait_for(phone.closed.wait(), 10)
        await asyncio.sleep(0.5)
        line, _, w = await exit_stream(sock(bridge), "www.second.example")
        w.close()
        return line

    assert asyncio.run(with_phone(bridge, mobile_data, [wifi["id"]])) == "ERR wifi_only"
    assert [s for s in live_sessions(bridge) if s["id"] == wifi["id"]][0]["paused_why"] == "wifi_only"

    async def back(phone, got):
        state = [s for s in live_sessions(bridge) if s["id"] == away["id"]][0]
        return state, await carry(bridge, 1000)

    state, carried = asyncio.run(with_phone(bridge, back, [away["id"]]))
    assert state["state"] == "active" and state["paused_why"] is None and carried
    assert bridge.get("/v1/exit/journal", headers=AUTH).json()["sessions"] == []   # never ended


def test_a_paused_session_ends_at_the_idle_limit(bridges):
    bridge = exit_bridge(bridges, idle_minutes=0.05)
    session = start_session(bridge)

    async def put_away(phone, got):
        await phone.control(PAUSE, {"why": "background"})
        await phone.ws.close()
        await asyncio.wait_for(phone.closed.wait(), 10)

    asyncio.run(with_phone(bridge, put_away, [session["id"]]))
    ended = wait_for(lambda: bridge.get("/v1/exit/journal", headers=AUTH).json()["sessions"], 15)
    assert ended[0]["id"] == session["id"] and ended[0]["ended"] == "idle" and ended[0]["paused_why"] == "background"


def test_an_answered_card_s_session_is_listed_at_once(bridges):
    """The card answered through the common card path: its `accepted` session is in GET /v1/exit/sessions at once, so
    the phone that answered finds it (and never starts a second one)."""
    bridge = exit_bridge(bridges)
    call(bridge, "ask_phone_exit", {"site": f"https://www.{SITE}/espace", "reason": "Cloudflare"})
    [card] = cards(bridge)
    assert live_sessions(bridge) == []
    answer = bridge.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": card["run_id"],
                                                             "choice": "once", "request_id": card["request_id"]})
    assert answer.status_code == 200, answer.text
    [listed] = live_sessions(bridge)
    assert listed["id"] == answer.json()["exit"]["session"]["id"] and listed["state"] == "accepted"
    assert listed["card"] == card["run_id"] and listed["sites"] == [SITE] and listed["agent"] == "wellness"
    # found by the phone's own words: the agent, the card's host or URL, the card
    assert [s["id"] for s in live_sessions(bridge, agent="wellness", site=f"www.{SITE}")] == [listed["id"]]
    assert [s["id"] for s in live_sessions(bridge, site=f"https://{HOST}/x?y=1")] == [listed["id"]]
    assert [s["id"] for s in live_sessions(bridge, card=card["run_id"])] == [listed["id"]]
    assert live_sessions(bridge, site="other.example") == [] and live_sessions(bridge, agent="nobody") == []
    # started from the app: no card on it
    assert start_session(bridge, ["other.example"]).get("card") is None


def test_welcome_only_narrows_the_phone_s_list(bridges):
    """WELCOME lists only the sessions the phone named, never another; the bridge opens streams only for the sessions
    the tunnel carries."""
    bridge = exit_bridge(bridges)
    mine, other = start_session(bridge), start_session(bridge, ["other.example"])

    async def steps(phone, got):
        first = phone.welcomes[0]
        refused, _, w = await exit_stream(sock(bridge), "www.other.example")
        w.close()
        await phone.control(SESSIONS, {"sessions": [mine["id"], "made-up"]})
        await asyncio.sleep(0.3)
        ok = await carry(bridge, 1000)
        return first, refused, phone.welcomes[-1], [o["host"] for o in phone.opens], ok

    first, refused, last, opened, ok = asyncio.run(with_phone(bridge, steps, [mine["id"]]))
    assert [s["id"] for s in first["sessions"]] == [mine["id"]] and first["refused"] == []
    assert refused == "ERR paused" and ok   # the other session is not carried: never opened on this phone
    assert [s["id"] for s in last["sessions"]] == [mine["id"]]
    assert last["refused"] == [{"id": "made-up", "why": "unknown"}]
    assert opened == [HOST] and other["id"] not in str(first) + str(last)


def test_settings_and_sites(bridges):
    bridge = exit_bridge(bridges)
    over = bridge.get("/v1/exit", headers=AUTH).json()
    assert over["enabled"] is True and over["settings"] == {"allowed": True, "wifi_only": False, "minimal_route": True}
    assert "cap_choices" not in over
    put = bridge.put("/v1/exit/sites/www.Leboncoin.fr.", headers=AUTH,
                     json={"mode": "always", "minimal": False})
    assert put.status_code == 200, put.text
    assert put.json()["sites"][0]["site"] == "leboncoin.fr" and put.json()["sites"][0]["minimal"] is False
    assert bridge.put("/v1/exit/sites/10.0.0.1", headers=AUTH, json={"mode": "always"}).status_code == 422
    assert bridge.put("/v1/exit/sites/x.example", headers=AUTH, json={"mode": "maybe"}).status_code == 422
    assert bridge.delete("/v1/exit/sites/leboncoin.fr", headers=AUTH).json()["sites"] == []
    assert bridge.post("/v1/exit/sessions", headers=AUTH, json={"agent": "nobody", "sites": [SITE]}).status_code == 404
    stored = json.loads((bridge.state_dir / "phone-exit.json").read_text())
    assert oct((bridge.state_dir / "phone-exit.json").stat().st_mode & 0o777) == "0o600" and "sites" in stored
