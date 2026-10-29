"""Messages between agents through the bridge (`/peer/p/…`), only along the links the person turned on
(`/v1/agent-links`), and two households (two bridge processes) linked by an invitation (`/v1/family…`,
`/family/…`), their agents writing to each other where both people allowed it (tests/test_households.py and
test_peers.py, over HTTP; the agents' Hermes is the fake, echoing what it gets)."""
import hashlib
import json
import os
import time

from starlette.responses import JSONResponse

from bbkit.common import AUTH
from bbkit.fakes import split_now
from conftest import managed_install, profile

RECEIVED = []


def echo(seen):
    """A Hermes whose Bot Chat is « bot-1 », answering any other call with what it received. The conversation the
    bridge makes for two agents (agent_pairs.rs) is « pair-1 », where Cuisine answers « Des lasagnes. »."""
    if seen.method == "GET" and seen.path.endswith("/api/sessions"):
        return JSONResponse({"data": [{"id": "bot-1", "title": "Bot Chat"}, {"id": "s9", "title": "Courses"}]})
    if seen.method == "POST" and seen.path.endswith("/api/sessions"):
        return JSONResponse({"session": {"id": "pair-1"}})
    if seen.method == "PATCH":
        return JSONResponse({})
    if seen.method == "GET" and seen.path.endswith("/api/sessions/pair-1/messages"):
        return JSONResponse({"messages": [{"role": "assistant", "content": "Des lasagnes."}]})
    if seen.method == "GET" and "/v1/runs/" in seen.path:
        return JSONResponse({"status": "running", "session_id": "bot-1" if "run-bot" in seen.path else "s9"})
    RECEIVED.append((seen.path, seen.query, seen.header("authorization"), seen.json()))
    return JSONResponse({"ok": True, "path": seen.path})


def give_peer_token(bridge, gateway, token):
    """peers.json as the bridge keeps it: each sending gateway's token digest (`bipbridge peer-token` writes one)."""
    path = bridge.state_dir / "peers.json"
    data = json.loads(path.read_text()) if path.exists() else {"links": [], "tokens": {}}
    data["tokens"][gateway] = hashlib.sha256(token.encode()).hexdigest()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data))


def household(bridges, fakes, name, agents):
    def change(c):
        c["agents"] = {a.title(): {"hermes_url": f"{fakes.hermes.url}/p/{a}", "hermes_key": f"{a}-key-" + "x" * 24}
                       for a in agents}
    bridge = bridges.new(name, change=change)
    bridge.config["public_url"] = bridge.url
    return bridge.start()


def test_two_households_write_to_each_other_only_where_both_allowed(bridges, fakes):
    RECEIVED.clear()
    fakes.hermes.override = echo
    a = household(bridges, fakes, "alex", ["vie", "budget"])
    b = household(bridges, fakes, "lea", ["cuisine"])
    refused = a.post("/v1/family/invite", headers=AUTH)
    assert refused.status_code == 409 and refused.json() == {"detail": "set this household's name first"}
    assert a.put("/v1/family/name", headers=AUTH, json={"name": "Alex"}).json() == {"name": "Alex"}
    assert b.put("/v1/family/name", headers=AUTH, json={"name": "Léa"}).status_code == 200
    assert a.put("/v1/family/name", headers=AUTH, json={"name": " "}).status_code == 400
    invitation = a.post("/v1/family/invite", headers=AUTH).json()
    assert invitation["kind"] == "family" and invitation["url"] == a.url and invitation["v"] == 1
    assert invitation["name"] == "Alex" and len(invitation["code"]) >= 32
    joined = b.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
    assert joined.status_code == 200 and joined.json() == {"id": "alex", "name": "Alex"}
    again = b.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
    assert again.status_code == 403 and again.json() == {"detail": "the invitation was refused (expired?)"}
    assert a.get("/v1/family", headers=AUTH).json() == {"name": "Alex", "url": a.url,
                                                        "households": [{"id": "lea", "name": "Léa", "url": b.url, "relink": False}]}
    stored = json.loads((a.state_dir / "households.json").read_text())
    assert oct(os.stat(a.state_dir / "households.json").st_mode & 0o777) == "0o600"
    assert stored["households"]["lea"]["key_in"] == hashlib.sha256(
        json.loads((b.state_dir / "households.json").read_text())["households"]["alex"]["key_out"].encode()).hexdigest()
    # Each side lists the other's agents among its own.
    names = {x["id"]: x for x in a.get("/v1/agent-links", headers=AUTH).json()["agents"]}
    assert names["cuisine__lea"] == {"id": "cuisine__lea", "name": "Cuisine (Léa)", "managed": False, "household": "lea"}
    assert names["vie"] == {"id": "vie", "name": "Vie", "managed": False}

    # Alex lets Vie write to Cuisine; Léa has not allowed it yet: her bridge refuses.
    set_links = a.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "vie", "to": "cuisine__lea"}]})
    assert set_links.json() == {"links": [{"from": "vie", "to": "cuisine__lea"}], "needs_setup": ["vie"]}
    token = "vie-gateway-token-" + "z" * 20
    give_peer_token(a, "manual:vie", token)
    message = {"session_id": "bot-1", "message": "Message from Vie (@vie): qu'est-ce qu'on mange ce soir ?"}
    send = lambda: a.post("/peer/p/cuisine__lea/v1/runs", headers={"Authorization": f"Bearer {token}"}, json=message)  # noqa: E731
    first = send()
    assert first.status_code == 403 and RECEIVED == []
    assert first.json() == {"detail": "Cuisine does not take messages from vie__alex"}
    # Léa allows « Vie (Alex) → Cuisine »: delivered to Cuisine, signed with its household and its answer address.
    assert b.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "vie__alex", "to": "cuisine"}]}).status_code == 200
    delivered = send()
    assert delivered.status_code == 200 and delivered.json() == {"ok": True, "path": "/p/cuisine/v1/runs"}
    path, _, auth, body = RECEIVED[-1]
    assert path == "/p/cuisine/v1/runs" and auth == "Bearer cuisine-key-" + "x" * 24
    now, words = split_now(body["message"])
    note, words = words.split("\n-->\n", 1)
    assert note.startswith("<!--bip:pair\nThis is your own conversation with Vie (Alex) (@vie__alex)")
    assert now and words == "Message from Vie (Alex) (@vie__alex): qu'est-ce qu'on mange ce soir ?"   # the time on top
    # Into Cuisine's own conversation with Vie (Alex), not its Bot Chat (the id the other side had): what Léa said to
    # Cuisine stays out of that turn's history.
    assert body["session_id"] == "pair-1"
    # Léa's app reads it as « Vie (Alex) ⇄ Cuisine »: the ask, then Cuisine's answer.
    pair = b.get("/v1/agent-pairs", headers=AUTH, params={"agent": "cuisine"}).json()["pairs"][0]
    assert {"id": "vie__alex", "name": "Vie (Alex)", "category": None, "color": None, "gone": False,
            "household": True} in pair["agents"]
    for _ in range(50):
        thread = b.get(f"/v1/agent-pairs/{pair['id']}", headers=AUTH).json()["thread"]
        if len(thread) == 2:
            break
        time.sleep(0.1)
    assert [(m["from"], m["kind"], m["text"]) for m in thread] == [
        ("vie__alex", "ask", "qu'est-ce qu'on mange ce soir ?"), ("cuisine", "answer", "Des lasagnes.")]
    # Another household's agent writes: the turn is closed like a relative's (patch 0018's mark)
    assert body["guest_turn"]["tools"][-1] == "web_search" and "mcp__bipagents__weather" in body["guest_turn"]["tools"]
    assert not {"terminal", "session_search", "mcp__bipagents__mail_search"} & set(body["guest_turn"]["tools"])
    # Only Cuisine's Bot Chat: not another of its conversations, nor its sessions' list or its runs' approvals.
    other = dict(message, session_id="s9")
    assert a.post("/peer/p/cuisine__lea/v1/runs", headers={"Authorization": f"Bearer {token}"}, json=other).status_code == 403
    peek = a.get("/peer/p/cuisine__lea/api/sessions", headers={"Authorization": f"Bearer {token}"})
    assert peek.status_code == 403
    listing = a.get("/peer/p/cuisine__lea/api/sessions?title=Bot%20Chat&include_hidden=1",
                    headers={"Authorization": f"Bearer {token}"})
    assert listing.json() == {"object": "list", "data": [{"id": "pair-1", "title": "Bot Chat"}]}   # the pair's
    assert a.post("/peer/p/cuisine__lea/v1/runs/run-1/approval", headers={"Authorization": f"Bearer {token}"},
                  json={"choice": "always"}).status_code == 403
    # Budget (its own gateway, not linked) cannot, nor can anything without the household's key.
    budget_token = "budget-gateway-token-" + "w" * 20
    give_peer_token(a, "manual:budget", budget_token)
    assert a.post("/peer/p/cuisine__lea/v1/runs", headers={"Authorization": f"Bearer {budget_token}"},
                  json={"message": "Message from Budget (@budget): coucou"}).status_code == 403
    nope = b.post("/family/p/cuisine/v1/runs", headers={"Authorization": "Bearer nope"}, json=message)
    assert nope.status_code == 401 and nope.json() == {"detail": "unknown household"}
    assert b.get("/family/agents", headers={"Authorization": "Bearer nope"}).status_code == 401

    # Unlinking the household drops its links.
    assert b.delete("/v1/family/alex", headers=AUTH).status_code == 204
    assert b.delete("/v1/family/alex", headers=AUTH).status_code == 404
    assert b.get("/v1/agent-links", headers=AUTH).json()["links"] == []
    assert send().status_code in (401, 403)


def test_invitations_are_one_time_and_expire(bridges, fakes):
    a = household(bridges, fakes, "alex", ["vie"])
    a.put("/v1/family/name", headers=AUTH, json={"name": "Alex"})
    code = a.post("/v1/family/invite", headers=AUTH).json()["code"]
    key = "k" * 40
    answer = a.post("/family/accept", json={"code": code, "name": "Léa", "url": "https://b.ts.net", "key": key})
    assert answer.status_code == 200 and answer.json()["name"] == "Alex" and len(answer.json()["key"]) >= 32
    assert key not in (a.state_dir / "households.json").read_text()        # only a digest of their key
    used = a.post("/family/accept", json={"code": code, "name": "Paul", "url": "https://c.ts.net", "key": key})
    assert used.status_code == 403 and used.json() == {"detail": "this invitation is unknown or expired"}
    agents = a.get("/family/agents", headers={"Authorization": f"Bearer {key}"}).json()
    assert agents == {"name": "Alex", "agents": [{"id": "vie", "name": "Vie"}]}
    late = a.post("/v1/family/invite", headers=AUTH).json()["code"]
    path = a.state_dir / "households.json"
    data = json.loads(path.read_text())
    data["invites"] = {k: time.time() - 1 for k in data["invites"]}
    path.write_text(json.dumps(data))
    assert a.post("/family/accept", json={"code": late, "name": "Paul", "url": "https://c.ts.net",
                                          "key": "p" * 40}).status_code == 403
    no_url = bridges.new("nourl").start()
    refused = no_url.post("/v1/family/invite", headers=AUTH)
    assert refused.status_code == 409 and refused.json() == {"detail": "this bridge has no address on the tailnet (public_url)"}
    cannot = no_url.post("/v1/family/join", headers=AUTH, json={"url": a.url, "code": "x"})
    assert cannot.status_code == 409


def test_a_new_link_cannot_take_a_linked_households_name(bridges, fakes):
    """Audit 2026-10-27 #9: a second household introducing itself under a linked household's name is refused (409),
    the first link stays as it was and the invitation is still good; the same household, with its current key, may
    link again."""
    a = household(bridges, fakes, "alex", ["vie"])
    a.put("/v1/family/name", headers=AUTH, json={"name": "Alex"})
    key = "k" * 40
    code = a.post("/v1/family/invite", headers=AUTH).json()["code"]
    assert a.post("/family/accept", json={"code": code, "name": "Léa", "url": "https://b.ts.net",
                                          "key": key}).status_code == 200
    before = json.loads((a.state_dir / "households.json").read_text())["households"]
    code = a.post("/v1/family/invite", headers=AUTH).json()["code"]
    intruder = {"code": code, "name": "LÉA", "url": "https://evil.ts.net", "key": "e" * 40}
    taken = a.post("/family/accept", json=intruder)
    assert taken.status_code == 409
    wrong_key = a.post("/family/accept", json=intruder, headers={"Authorization": "Bearer " + "x" * 40})
    assert wrong_key.status_code == 409
    assert json.loads((a.state_dir / "households.json").read_text())["households"] == before
    assert a.get("/family/agents", headers={"Authorization": f"Bearer {key}"}).status_code == 200
    assert a.get("/family/agents", headers={"Authorization": "Bearer " + "e" * 40}).status_code == 401
    # Léa's bridge, with the key it calls us with today, links again (same code: the refusals did not use it)
    again = a.post("/family/accept", headers={"Authorization": f"Bearer {key}"},
                   json={"code": code, "name": "Léa", "url": "https://b2.ts.net", "key": "n" * 40})
    assert again.status_code == 200
    assert a.get("/family/agents", headers={"Authorization": "Bearer " + "n" * 40}).status_code == 200
    assert a.get("/family/agents", headers={"Authorization": f"Bearer {key}"}).status_code == 401
    assert [h["url"] for h in a.get("/v1/family", headers=AUTH).json()["households"]] == ["https://b2.ts.net"]


def test_two_linked_households_can_link_again(bridges, fakes):
    """Scanning a new invitation of a household already linked (e.g. a link to be redone) still works on both sides."""
    a = household(bridges, fakes, "alex", ["vie"])
    b = household(bridges, fakes, "lea", ["cuisine"])
    a.put("/v1/family/name", headers=AUTH, json={"name": "Alex"})
    b.put("/v1/family/name", headers=AUTH, json={"name": "Léa"})
    for _ in range(2):
        invitation = a.post("/v1/family/invite", headers=AUTH).json()
        joined = b.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
        assert joined.status_code == 200 and joined.json() == {"id": "alex", "name": "Alex"}
    key = json.loads((b.state_dir / "households.json").read_text())["households"]["alex"]["key_out"]
    assert a.get("/family/agents", headers={"Authorization": f"Bearer {key}"}).status_code == 200


def test_the_bridge_refuses_what_the_person_did_not_allow(bridge, fakes):
    out = bridge.cli("peer-token", "--agent", "wellness")
    assert out.returncode == 0, out.stderr
    lines = out.stdout.splitlines()
    assert lines[1] == f"hermes config set bot_peers.bip.url http://127.0.0.1:{bridge.port}/peer"
    token = lines[3].split("HERMES_PEER_BIP_KEY=", 1)[1]
    stored = json.loads((bridge.state_dir / "peers.json").read_text())
    assert stored["tokens"] == {"manual:wellness": hashlib.sha256(token.encode()).hexdigest()}
    assert oct(os.stat(bridge.state_dir / "peers.json").st_mode & 0o777) == "0o600"
    assert bridge.get("/peer/p/wellness/api/sessions", headers={"Authorization": "Bearer nope"}).status_code == 401
    refused = bridge.get("/peer/p/wellness/api/sessions", headers={"Authorization": f"Bearer {token}"})
    assert refused.status_code == 403 and refused.json() == {"detail": "not allowed to write to wellness (ask the person)"}
    assert bridge.get("/peer/p/nobody/api/sessions", headers={"Authorization": f"Bearer {token}"}).status_code == 404
    assert bridge.get("/v1/agent-links", headers=AUTH).json() == {
        "agents": [{"id": "wellness", "name": "Wellness", "managed": False}], "links": []}
    assert bridge.cli("peer-token").returncode == 2


def test_each_sending_gateway_gets_a_token_and_each_sender_learns_whom_it_may_write_to(bridges, fakes):
    """peers.apply through `PUT /v1/agent-links`: the hub's and a space's gateways get their token in their own .env
    and the bridge as their peer; a skill lists each sender's targets."""
    RECEIVED.clear()
    bridge = bridges.new()
    hub = managed_install(bridge, fakes, agents=("budget", "maison"))
    for name in ("budget", "maison"):
        profile(hub, name)
    space = bridge.root / "spaces" / "jardin"
    profile(space, "jardin")
    bridge.config["agents"]["jardin"] = {"hermes_url": f"{fakes.hermes.url}/p/jardin", "hermes_key": "j" * 32,
                                         "isolated": True, "sandboxed": True, "space": str(space)}
    bridge.start()
    unknown = bridge.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "budget", "to": "ghost"}]})
    assert unknown.status_code == 400 and unknown.json() == {"detail": "unknown link budget → ghost"}
    assert bridge.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "budget", "to": "budget"}]}).status_code == 400
    links = [{"from": "Budget", "to": "jardin"}, {"from": "jardin", "to": "wellness"}, {"from": "budget", "to": "maison"}]
    result = bridge.put("/v1/agent-links", headers=AUTH, json={"links": links}).json()
    assert result == {"links": [{"from": "budget", "to": "jardin"}, {"from": "budget", "to": "maison"},
                                {"from": "jardin", "to": "wellness"}], "needs_setup": []}
    hub_token = (hub / ".env").read_text().split("HERMES_PEER_BIP_KEY=")[1].strip()
    space_token = (space / ".env").read_text().split("HERMES_PEER_BIP_KEY=")[1].strip()
    assert oct(os.stat(hub / ".env").st_mode & 0o777) == "0o600"
    stored = json.loads((bridge.state_dir / "peers.json").read_text())
    assert stored["tokens"] == {str(hub): hashlib.sha256(hub_token.encode()).hexdigest(),
                                str(space): hashlib.sha256(space_token.encode()).hexdigest()}
    assert hub_token not in (bridge.state_dir / "peers.json").read_text()
    commands = [(e["argv"], e["hermes_home"]) for e in bridge.commands.entries("hermes")]
    assert (["config", "set", "bot_peers.bip.url", f"http://127.0.0.1:{bridge.port}/peer"], str(space)) in commands
    assert (["config", "set", "bot_peers.bip.note", "Tes autres agents, par BipAgents (cible : bip/<agent>)"],
            str(hub)) in commands
    contacts = (hub / "profiles" / "budget" / "skills" / "agent-contacts" / "SKILL.md").read_text()
    assert "`bip/jardin`: Jardin" in contacts and "`bip/maison`: Maison" in contacts
    assert not (hub / "profiles" / "maison" / "skills" / "agent-contacts").exists()   # writes to nobody
    managed = {a["id"]: a["managed"] for a in bridge.get("/v1/agent-links", headers=AUTH).json()["agents"]}
    assert managed == {"wellness": False, "budget": True, "maison": True, "jardin": True}
    # The hub's agents write to the space's Jardin with the hub's token, signed by Budget.
    fakes.hermes.override = echo
    sent = bridge.post("/peer/p/jardin/v1/runs", headers={"Authorization": f"Bearer {hub_token}"},
                       json={"session_id": "bot-1", "message": "Message from 🤖 Budget (@budget): bonjour"})
    assert sent.status_code == 200 and RECEIVED[-1][2] == "Bearer " + "j" * 32
    now, words = split_now(RECEIVED[-1][3]["message"])
    assert now and words == "Message from 🤖 Budget (@budget): bonjour"     # same household: as it is, the time on top
    as_maison = bridge.post("/peer/p/jardin/v1/runs", headers={"Authorization": f"Bearer {hub_token}"},
                            json={"session_id": "bot-1", "message": "Message from Maison (@maison): salut"})
    assert as_maison.status_code == 403                                              # maison → jardin: not allowed
    run = bridge.get("/peer/p/jardin/v1/runs/run-bot-1", headers={"Authorization": f"Bearer {hub_token}"})
    assert run.status_code == 200
    assert bridge.get("/peer/p/jardin/v1/runs/run-other", headers={"Authorization": f"Bearer {hub_token}"}).status_code == 403
    # Wellness (set up by hand) sends: the administrator gives it a token; the hub sends nothing any more.
    again = bridge.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "wellness", "to": "budget"}]}).json()
    assert again["needs_setup"] == ["wellness"]
    assert "HERMES_PEER_BIP_KEY" not in (hub / ".env").read_text()
    assert str(hub) not in json.loads((bridge.state_dir / "peers.json").read_text())["tokens"]
    assert not (hub / "profiles" / "budget" / "skills" / "agent-contacts" / "SKILL.md").exists()
