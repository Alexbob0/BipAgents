"""Two households linked wherever they are, through Kerfy Studio's relay (relay/ family.rs, bridge-rs
family_rendezvous.rs and family_seal.rs): two installs at home on different networks, or a place of the offer
scanning a home's invitation (the « wrong side » before).

Each bridge listens on 127.0.0.1 only and announces a home's address nothing answers at (`*.home.arpa`, not even a
name that resolves): neither has any route to the other. The only thing both reach is the relay (the real binary),
each with a long poll it opens itself. What passes there is sealed with a key of the link only the two bridges hold:
a relay that changes a bit, sends a call again, back to its sender or to another household, gets it refused.

- Two homes link through the relay, whichever scans; their agents list and write to each other, family news and a
  shared agent's conversations pass, both ways.
- A place scanning a home's invitation links (the « wrong side » message is gone): afterwards they need the relay no
  more (the home fetches the place's calls for it, as when it scans the place's).
- The relay switched off: the homes' calls fail at once, plainly; their pages open; back on, it all works again.
- The relay misbehaving: a bit changed in a call or in its answer, a call sent again, back to its sender or to
  another household: refused, nothing reaches an agent."""
import json
import time

import httpx
import pytest
from starlette.responses import JSONResponse

from bbkit.common import AUTH
from bbkit.kerfy_relay import KerfyRelay, RelayProxy, send_again
from bbkit.servers import wait_until
from test_family_history import propose
from test_households_and_peers import give_peer_token

RECEIVED = []


def echo(seen):
    """A Hermes whose Bot Chat is « bot-1 » (the pair's conversation « pair-1 »), answering a run with what it got."""
    if seen.method == "GET" and seen.path.endswith("/api/sessions"):
        return JSONResponse({"data": [{"id": "bot-1", "title": "Bot Chat"}]})
    if seen.method == "POST" and seen.path.endswith("/api/sessions"):
        return JSONResponse({"session": {"id": "pair-1"}})
    if seen.method == "PATCH":
        return JSONResponse({})
    if seen.method == "GET" and "/api/sessions/pair-1/messages" in seen.path:
        return JSONResponse({"messages": [{"role": "assistant", "content": "D'accord."}]})
    if seen.method == "GET" and seen.path.endswith("/api/jobs"):
        return JSONResponse({"jobs": []})
    if seen.method == "GET" and "/v1/runs/" in seen.path:
        return JSONResponse({"status": "running", "session_id": "bot-1"})
    RECEIVED.append((seen.path, seen.json()))
    return JSONResponse({"ok": True, "path": seen.path})


@pytest.fixture
def relay(tmp_path):
    started = KerfyRelay(tmp_path).start()
    yield started
    started.stop()


def install(bridges, fakes, name, agents, relay_url, place=False):
    def change(c):
        c["agents"] = {a.title(): {"hermes_url": f"{fakes.hermes.url}/p/{a}", "hermes_key": f"{a}-key-" + "x" * 24}
                       for a in agents}
        c["family"] = {"relay_url": relay_url}
    bridge = bridges.new(name, change=change)
    if place:
        bridge.config["cloud"] = True
        bridge.config["public_url"] = bridge.url
    else:
        # a home's address on its own network: nothing answers there, not even a name that resolves
        bridge.config["public_url"] = f"http://{name}.home.arpa:8642"
    return bridge.start()


def named(bridge, name):
    assert bridge.put("/v1/family/name", headers=AUTH, json={"name": name}).status_code == 200


def households(bridge):
    return json.loads((bridge.state_dir / "households.json").read_text())["households"]


def own_mailbox(bridge):
    return json.loads((bridge.state_dir / "family-relay.json").read_text())["mailbox"]


def link_agents(bridge):
    return {a["id"] for a in bridge.get("/v1/agent-links", headers=AUTH).json()["agents"]}


def link(scanner, inviter):
    invitation = inviter.post("/v1/family/invite", headers=AUTH).json()
    assert len(invitation.get("relay", "")) == 22, invitation   # the inviter's mailbox at the relay
    joined = scanner.post("/v1/family/join", headers=AUTH,
                          json={"url": invitation["url"], "code": invitation["code"], "relay": invitation["relay"]})
    assert joined.status_code == 200, joined.text
    return joined.json()


def allow_messages(a, a_agent, b, b_agent, a_id, b_id):
    """Both people allow a_agent ⇄ b_agent; each gateway gets a token (as `bipbridge peer-token` gives one)."""
    assert a.put("/v1/agent-links", headers=AUTH, json={"links": [
        {"from": a_agent, "to": f"{b_agent}__{b_id}"}, {"from": f"{b_agent}__{b_id}", "to": a_agent}]}).status_code == 200
    assert b.put("/v1/agent-links", headers=AUTH, json={"links": [
        {"from": b_agent, "to": f"{a_agent}__{a_id}"}, {"from": f"{a_agent}__{a_id}", "to": b_agent}]}).status_code == 200
    tokens = (f"{a_agent}-gateway-" + "a" * 24, f"{b_agent}-gateway-" + "b" * 24)
    give_peer_token(a, f"manual:{a_agent}", tokens[0])
    give_peer_token(b, f"manual:{b_agent}", tokens[1])
    return tokens


def write(bridge, token, target, words, sender):
    return bridge.post(f"/peer/p/{target}/v1/runs", headers={"Authorization": f"Bearer {token}"},
                       json={"session_id": "bot-1", "message": f"Message from {sender.title()} (@{sender}): {words}"})


def two_homes(bridges, fakes, relay_url):
    fakes.hermes.override = echo
    alex = install(bridges, fakes, "alex", ["vie"], relay_url)
    lea = install(bridges, fakes, "lea", ["cuisine"], relay_url)
    named(alex, "Alex")
    named(lea, "Léa")
    return alex, lea


@pytest.mark.parametrize("scanner", ["alex", "lea"])
def test_two_homes_link_through_the_relay_whichever_scans(bridges, fakes, relay, scanner):
    RECEIVED.clear()
    alex, lea = two_homes(bridges, fakes, relay.url)
    # neither reaches the other: the direct way fails at once (no such name), the relay carries the link
    if scanner == "alex":
        assert link(alex, lea) == {"id": "lea", "name": "Léa"}
    else:
        assert link(lea, alex) == {"id": "alex", "name": "Alex"}
    mine, theirs = households(alex)["lea"], households(lea)["alex"]
    for entry in (mine, theirs):
        assert entry["via"] == "relay" and entry["relay_in"] is True, entry
        assert "pull" not in entry and "relay" not in entry, entry      # not the home↔place pull
        assert len(entry["seal"]) >= 43 and len(entry["route"]) >= 16, entry
    # one key for the link, the same on both sides, and each knows the other's mailbox
    assert mine["seal"] == theirs["seal"]
    assert mine["mailbox"] == own_mailbox(lea)["id"] and theirs["mailbox"] == own_mailbox(alex)["id"]
    assert {mine["role"], theirs["role"]} == {"inviter", "joiner"}
    # what the apps see: the household and its address, never the key nor the mailbox
    shown = alex.get("/v1/family", headers=AUTH).json()["households"]
    assert [h["id"] for h in shown] == ["lea"] and "seal" not in json.dumps(shown) and mine["mailbox"] not in json.dumps(shown)

    # each lists the other's agents (the call through the relay, sealed)
    assert wait_until(lambda: "cuisine__lea" in link_agents(alex), timeout=40), alex.log_tail()
    assert wait_until(lambda: "vie__alex" in link_agents(lea), timeout=40), lea.log_tail()

    # their agents write to each other, both ways
    vie, cuisine = allow_messages(alex, "vie", lea, "cuisine", "alex", "lea")
    sent = write(alex, vie, "cuisine__lea", "on dîne ensemble samedi ?", "vie")
    assert sent.status_code == 200 and sent.json() == {"ok": True, "path": "/p/cuisine/v1/runs"}, sent.text
    path, body = RECEIVED[-1]
    assert path == "/p/cuisine/v1/runs" and "Message from Vie (Alex) (@vie__alex): on dîne ensemble samedi ?" in body["message"]
    back = write(lea, cuisine, "vie__alex", "oui, à 20 h", "cuisine")
    assert back.status_code == 200 and back.json() == {"ok": True, "path": "/p/vie/v1/runs"}, back.text
    assert "Message from Cuisine (Léa) (@cuisine__lea): oui, à 20 h" in RECEIVED[-1][1]["message"]

    # family news, both ways
    propose(alex, "n1", [{"id": "lea", "name": "Léa"}], "Je rentre tard")
    told = alex.post("/v1/family/proposals/n1/send", headers=AUTH, json={"text": "Je rentre vers 21 h"})
    assert told.status_code == 200 and told.json() == {"sent": ["Léa"], "failed": []}, told.text
    got = lea.get("/v1/family/history", headers=AUTH).json()["items"]
    assert [(x["direction"], x["text"]) for x in got] == [("received", "Je rentre vers 21 h")], got
    propose(lea, "n2", [{"id": "alex", "name": "Alex"}], "Pain ?")
    assert lea.post("/v1/family/proposals/n2/send", headers=AUTH, json={"text": "Tu prends du pain ?"}).json() == {
        "sent": ["Alex"], "failed": []}

    # a shared agent: Léa shares Cuisine, Alex reads its conversations through the relay
    fakes.hermes.override = None
    fakes.hermes.v0225 = True
    now = time.time()
    fakes.hermes.add_session("api_main", [
        {"id": 1, "role": "user", "content": "Un menu pour samedi ?", "timestamp": now + 60},
        {"id": 2, "role": "assistant", "content": "Des lasagnes.", "timestamp": now + 61},
    ], title="Bot Chat", agent="cuisine", last_active=now + 61, started_at=now - 10)
    shared = lea.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["alex"], "confirmed": True})
    assert shared.status_code == 200 and shared.json()["told"] == {"alex": True}, shared.text
    offered = alex.get("/v1/shared-agents", headers=AUTH).json()["agents"]
    assert [(x["id"], x["state"]) for x in offered] == [("cuisine__lea", "offered")], offered
    assert alex.post("/v1/shared-agents/cuisine__lea/add", headers=AUTH).status_code == 200
    page = alex.get("/v1/threads/main/timeline", headers=AUTH, params={"agent": "cuisine__lea"})
    assert page.status_code == 200, page.text
    assert [(i["role"], i["text"]) for i in page.json()["items"]] == [
        ("person", "Un menu pour samedi ?"), ("agent", "Des lasagnes.")], page.json()["items"]

    # the relay saw sealed bytes only: no name, no words, no key ever in its log
    logged = relay.log_tail(200)
    for secret in ("Léa", "dîne", "lasagnes", "Je rentre", mine["seal"], "cuisine"):
        assert secret not in logged, secret


def test_a_place_scanning_a_homes_invitation_links_and_needs_the_relay_no_more(bridges, fakes, relay):
    RECEIVED.clear()
    fakes.hermes.override = echo
    home = install(bridges, fakes, "alex", ["vie"], relay.url)
    place = install(bridges, fakes, "chi", ["cuisine"], relay.url, place=True)
    named(home, "Alex")
    named(place, "Chi")
    # the « wrong side » of before: the place scans the home's invitation
    assert link(place, home) == {"id": "alex", "name": "Alex"}
    # then as when the home scans the place's: the place keeps its calls for the home, the home fetches them
    at_home, at_place = households(home)["chi"], households(place)["alex"]
    assert at_home["pull"] is True and "via" not in at_home, at_home
    assert len(at_place["relay"]) >= 16 and "via" not in at_place and "pull" not in at_place, at_place
    assert at_home["seal"] == at_place["seal"]
    assert wait_until(lambda: "vie__alex" in link_agents(place), timeout=30), place.log_tail()
    assert "cuisine__chi" in link_agents(home)

    # Kerfy's relay switched off: nothing changes for them
    relay.stop()
    cuisine, vie = allow_messages(place, "cuisine", home, "vie", "chi", "alex")
    sent = write(place, cuisine, "vie__alex", "des pâtes ce soir ?", "cuisine")
    assert sent.status_code == 200 and sent.json() == {"ok": True, "path": "/p/vie/v1/runs"}, sent.text
    back = write(home, vie, "cuisine__chi", "oui !", "vie")
    assert back.status_code == 200 and back.json() == {"ok": True, "path": "/p/cuisine/v1/runs"}, back.text


def test_the_relay_off_then_back(bridges, fakes, relay):
    RECEIVED.clear()
    alex, lea = two_homes(bridges, fakes, relay.url)
    link(alex, lea)
    vie, _ = allow_messages(alex, "vie", lea, "cuisine", "alex", "lea")
    assert wait_until(lambda: "cuisine__lea" in link_agents(alex), timeout=40), alex.log_tail()

    relay.stop()
    started = time.monotonic()
    assert alex.get("/v1/family", headers=AUTH).status_code == 200
    assert "cuisine__lea" not in link_agents(alex)
    gone = write(alex, vie, "cuisine__lea", "tu es là ?", "vie")
    assert gone.status_code == 502, gone.text
    assert time.monotonic() - started < 5, "the calls fail at once, never a hang"
    assert RECEIVED == []

    relay.start()   # the same relay back (same keys): the bridges' polls come back by themselves
    assert wait_until(lambda: "cuisine__lea" in link_agents(alex), timeout=45), alex.log_tail()
    again = write(alex, vie, "cuisine__lea", "tu es là ?", "vie")
    assert again.status_code == 200, again.text
    assert RECEIVED[-1][0] == "/p/cuisine/v1/runs"


def test_the_relay_off_when_linking_says_so(bridges, fakes, relay):
    alex, lea = two_homes(bridges, fakes, relay.url)
    invitation = lea.post("/v1/family/invite", headers=AUTH).json()
    relay.stop()
    started = time.monotonic()
    refused = alex.post("/v1/family/join", headers=AUTH,
                        json={"url": invitation["url"], "code": invitation["code"], "relay": invitation["relay"]})
    assert time.monotonic() - started < 15
    assert refused.status_code == 502 and refused.json()["detail"]["code"] == "relay_unreachable", refused.text
    assert households(alex) == {} and households(lea) == {}
    relay.start()
    # the same invitation still works once the relay is back (it was not used)
    assert wait_until(lambda: alex.post("/v1/family/join", headers=AUTH, json={
        "url": invitation["url"], "code": invitation["code"], "relay": invitation["relay"]}).status_code == 200,
        timeout=40, interval=2), alex.log_tail()


def test_a_relay_that_changes_or_sends_again_what_passes_is_refused(bridges, fakes, relay):
    RECEIVED.clear()
    proxy = RelayProxy(relay.url)
    try:
        alex, lea = two_homes(bridges, fakes, proxy.url)
        paul = install(bridges, fakes, "paul", ["coach"], proxy.url)
        named(paul, "Paul")
        link(alex, lea)
        link(paul, alex)
        vie, _ = allow_messages(alex, "vie", lea, "cuisine", "alex", "lea")
        assert wait_until(lambda: "cuisine__lea" in link_agents(alex), timeout=40), alex.log_tail()
        assert wait_until(lambda: "coach__paul" in link_agents(alex), timeout=40), alex.log_tail()
        lea_box, alex_box, paul_box = own_mailbox(lea), own_mailbox(alex), own_mailbox(paul)

        sent = write(alex, vie, "cuisine__lea", "on dîne ensemble ?", "vie")
        assert sent.status_code == 200, sent.text
        delivered = [r for r in RECEIVED if r[0] == "/p/cuisine/v1/runs"]
        assert len(delivered) == 1
        path, headers, body = [c for c in proxy.calls if c[0] == f"/v1/family/call/{lea_box['id']}"][-1]

        # sent again as it was: Léa's bridge refuses it (seen already), nothing reaches Cuisine twice
        replayed = send_again(relay.url, path, headers, body)
        assert replayed.status_code == 200 and len(replayed.content) == 0, replayed.content
        # back to its sender (with Léa's token, as the relay could): Alex's bridge refuses its own call
        reflected = send_again(relay.url, f"/v1/family/call/{alex_box['id']}", headers, body, token=lea_box["token"])
        assert reflected.status_code == 200 and len(reflected.content) == 0
        # to another household Alex is linked with: Paul's bridge can't open it
        elsewhere = send_again(relay.url, f"/v1/family/call/{paul_box['id']}", headers, body)
        assert elsewhere.status_code == 200 and len(elsewhere.content) == 0
        assert len([r for r in RECEIVED if r[0] in ("/p/cuisine/v1/runs", "/p/vie/v1/runs", "/p/coach/v1/runs")]) == 1
        reasons = lambda b: {e.get("reason") for e in b.logged("family relay: call refused")}  # noqa: E731
        assert "replay" in reasons(lea), lea.log_tail()
        assert wait_until(lambda: reasons(alex) & {"unsealed"}, timeout=5), alex.log_tail()
        assert wait_until(lambda: reasons(paul) & {"unsealed"}, timeout=5), paul.log_tail()

        # one bit changed in what Léa's bridge takes: refused, Alex's call fails plainly, nothing reaches Cuisine
        proxy.flip_next = lea_box["token"]
        tampered = write(alex, vie, "cuisine__lea", "un autre soir ?", "vie")
        assert tampered.status_code == 502, tampered.text
        assert len([r for r in RECEIVED if r[0] == "/p/cuisine/v1/runs"]) == 1
        # one bit changed in Léa's answer: Alex's bridge refuses it
        proxy.flip_answer = lea_box["token"]
        answered = write(alex, vie, "cuisine__lea", "dimanche ?", "vie")
        assert answered.status_code == 502, answered.text
        # nothing changed any more: it goes through again
        fine = write(alex, vie, "cuisine__lea", "lundi ?", "vie")
        assert fine.status_code == 200, fine.text
    finally:
        proxy.stop()


def test_nobody_reads_a_mailbox_without_its_token(relay):
    """The relay's side of it (relay/tests/family.rs has the rest): a mailbox is read with its own token only."""
    made = httpx.post(f"{relay.url}/v1/family/mailbox", json={}).json()
    other = httpx.post(f"{relay.url}/v1/family/mailbox", json={}).json()
    assert httpx.get(f"{relay.url}/v1/family/next", headers={"Authorization": "Bearer fm1.nope"}).status_code == 401
    call = httpx.post(f"{relay.url}/v1/family/call/{made['mailbox']}", content=b"x")
    assert call.status_code == 401
    away = httpx.post(f"{relay.url}/v1/family/call/{made['mailbox']}", content=b"x",
                      headers={"Authorization": f"Bearer {other['token']}"})
    assert away.status_code == 503 and away.json()["code"] == "away"
