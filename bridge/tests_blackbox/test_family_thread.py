"""The family agent's ONE conversation (family_thread.rs, Alex 12/11), over three linked bridges: Alex's place holds
« Famille »; Marie and Paul write in its common conversation from their own bridges (their `main` and `new` land in
it). Everyone reads every message with who wrote it (`by`, `mine` for one's own); a message while someone else's turn
goes on waits behind it, never steering it, and words steer one's own turn; a member's turn is a normal run (no guest
mark: the agent's memory is written from anyone) with a member's instructions; her approvals are hers; everyone's
devices are told of the others' messages and of every reply; several family agents, each for the whole family or
only some members; the conversations of before go once."""
import json
import threading

from bbkit.common import AUTH, sse
from bbkit.mcp import TOKEN, call, give_token, media_of, text_of
from test_family_agent import make_family_agent
from test_households_and_peers import household
from test_shared_agents import link, wait

TOKENS = {"alex": "a" * 64, "marie": "b" * 64, "paul": "c" * 64}


def join(a, c, name):
    assert c.put("/v1/family/name", headers=AUTH, json={"name": name}).status_code == 200
    invitation = a.post("/v1/family/invite", headers=AUTH).json()
    joined = c.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
    assert joined.status_code == 200, joined.text


def family(bridges, fakes):
    a = household(bridges, fakes, "alex", ["famille", "budget"])
    b = household(bridges, fakes, "marie", ["vie"])
    c = household(bridges, fakes, "paul", ["sport"])
    link(a, b)
    join(a, c, "Paul")
    for bridge, name in ((a, "alex"), (b, "marie"), (c, "paul")):
        assert bridge.post("/v1/devices", headers=AUTH, json={"token": TOKENS[name], "environment": "sandbox",
                                                              "agent_ids": []}).status_code == 200
    return a, b, c


def discover(*members):
    """Each member's app opens the family's screen: her bridge asks the linked households for their family agents."""
    for bridge in members:
        assert bridge.get("/v1/family-agent", headers=AUTH).status_code == 200


def page(bridge, agent):
    answer = bridge.get("/v1/threads/main/timeline", headers=AUTH, params={"agent": agent})
    assert answer.status_code == 200, answer.text
    return answer.json()


def people(answer):
    return {i["text"]: i.get("by") for i in answer["items"] if i.get("role") == "person"}


def pushed(fakes, name):
    return [p for s, p in zip(fakes.apns.pushes(), fakes.apns.payloads()) if fakes.apns.token(s) == TOKENS[name]]


def said(fakes, bridge, name, words):
    """A push to `name`'s phone said `words` (its text is in the notification its bridge keeps, by the push's `n`)."""
    for p in pushed(fakes, name):
        row = bridge.get(f"/v1/notifications/{p.get('n')}", headers=AUTH) if p.get("n") else None
        kept = json.dumps(row.json(), ensure_ascii=False) if row is not None and row.status_code == 200 else ""
        if words in json.dumps(p, ensure_ascii=False) or words in kept:
            return True
    return False


def test_the_family_writes_in_one_conversation(bridges, fakes):
    a, b, c = family(bridges, fakes)
    make_family_agent(a)
    discover(b, c)
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), sse("message.delta", run_id="r1", delta="Je regarde"),
                               gate, sse("run.completed", run_id="r1", output="Des crêpes !")]
    fakes.hermes.runs["r2"] = [sse("run.started", run_id="r2"), sse("run.completed", run_id="r2", output="Va pour samedi.")]

    # one conversation in every list, the family's
    assert [t["kind"] for t in b.get("/v1/threads", headers=AUTH, params={"agent": "famille__alex"}).json()["threads"]] == ["family"]
    # Marie writes: her `new` lands in the family conversation
    sent = b.post("/v1/threads/new/messages", headers=AUTH,
                  json={"agent": "famille__alex", "client_id": "C0FFEE00-0000-4000-8000-0000000000A1",
                        "text": "Une idée pour dimanche ?"})
    assert sent.status_code == 200, sent.text
    assert sent.json()["state"] == "started" and sent.json()["thread"] == "family"
    run = fakes.hermes.started["r1"]
    # a normal run of the agent (its memory is written from anyone), with a member's instructions and her name
    assert run["agent"] == "famille" and run["guest_turn"] is None
    assert "Marie, a member of the family, writes to you now in the family's common conversation" in run["instructions"]
    assert "[Marie, a member of the family (household « marie »), writes to you:]\nUne idée pour dimanche ?" in run["input"]
    alex_threads = a.get("/v1/threads", headers=AUTH, params={"agent": "famille"}).json()["threads"]
    assert [t["kind"] for t in alex_threads] == ["family"]

    # Paul writes while Marie's turn goes on: his message waits behind it, it never steers her turn
    paul = c.post("/v1/threads/main/messages", headers=AUTH,
                  json={"agent": "famille__alex", "client_id": "C0FFEE00-0000-4000-8000-0000000000B1",
                        "text": "Et samedi ?"})
    assert paul.status_code == 200 and paul.json()["state"] == "queued", paul.text
    assert fakes.hermes.steers == []
    # Marie's words go into her own turn
    more = b.post("/v1/threads/main/messages", headers=AUTH,
                  json={"agent": "famille__alex", "client_id": "C0FFEE00-0000-4000-8000-0000000000A2",
                        "text": "Avec les enfants"})
    assert more.status_code == 200 and more.json()["state"] == "steered", more.text
    assert len(fakes.hermes.steers) == 1 and "Marie" in json.dumps(fakes.hermes.steers[0]["body"], ensure_ascii=False)

    # the others are told (their conversation not on their screen): Alex's devices and Paul's of Marie's message,
    # Marie's of Paul's, never one's own
    assert wait(lambda: said(fakes, a, "alex", "Marie : Une idée pour dimanche ?"), 15), pushed(fakes, "alex")
    assert wait(lambda: said(fakes, c, "paul", "Marie : Une idée pour dimanche ?"), 15), pushed(fakes, "paul")
    assert wait(lambda: said(fakes, b, "marie", "Paul : Et samedi ?"), 15), pushed(fakes, "marie")
    assert not said(fakes, b, "marie", "Marie : ") and not said(fakes, c, "paul", "Paul : ")

    # the creator reads everyone's words, with who wrote them; Paul's message waiting shows too, his
    alex = page(a, "famille")
    assert alex["thread"]["kind"] == "family"
    assert people(alex)["Une idée pour dimanche ?"] == {"name": "Marie", "mine": False}
    assert people(alex)["Et samedi ?"] == {"name": "Paul", "mine": False}
    assert alex["pending"]["run"]["by"] == {"name": "Marie", "mine": False}
    assert any(q.get("client_id") == "C0FFEE00-0000-4000-8000-0000000000B1" for q in alex["pending"]["queue"]["items"]), alex["pending"]

    # Marie's turn ends: Paul's message starts its turn, his, framed with his name; every reply told to everyone
    gate.set()
    assert wait(lambda: "r2" in fakes.hermes.started, 20)
    second = fakes.hermes.started["r2"]
    assert "[Paul, a member of the family (household « paul »), writes to you:]\nEt samedi ?" in second["input"]
    assert "Paul, a member of the family" in second["instructions"] and second["guest_turn"] is None
    for bridge, name in ((b, "marie"), (c, "paul")):
        assert wait(lambda: said(fakes, bridge, name, "Des crêpes !"), 15), (name, pushed(fakes, name))
    assert wait(lambda: page(a, "famille")["pending"]["run"] is None, 20)

    # each member reads the same conversation, her own words hers (reading it, she is on it: no push meanwhile)
    marie = page(b, "famille__alex")
    assert marie["thread"]["kind"] == "family"
    assert people(marie)["Une idée pour dimanche ?"] == {"name": "Marie", "mine": True}
    assert people(marie)["Et samedi ?"] == {"name": "Paul", "mine": False}
    paul_page = page(c, "famille__alex")
    assert people(paul_page)["Et samedi ?"] == {"name": "Paul", "mine": True}
    assert people(paul_page)["Une idée pour dimanche ?"] == {"name": "Marie", "mine": False}
    # nobody's frame shows
    assert not any("writes to you" in (i.get("text") or "") for i in alex["items"] + marie["items"] + paul_page["items"])

    # her turn is counted for her household (the family's credit, per member)
    assert wait(lambda: any(m["id"] == "marie" for m in a.get("/v1/family-agent/usage", headers=AUTH).json()["members"]), 10)
    # the family conversation is never deleted from a phone
    assert b.delete("/v1/threads/main", headers=AUTH, params={"agent": "famille__alex"}).status_code == 409
    assert a.delete("/v1/threads/main", headers=AUTH, params={"agent": "famille"}).status_code == 409


def test_a_members_cards_are_hers(bridges, fakes):
    a, b, c = family(bridges, fakes)
    make_family_agent(a)
    discover(b, c)
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"),
                               sse("approval.request", run_id="r1", request_id="req-1", command="curl meteo.fr",
                                   description="la météo", choices=["once", "deny"]),
                               gate, sse("run.completed", run_id="r1", output="Il fera beau.")]
    sent = b.post("/v1/threads/main/messages", headers=AUTH,
                  json={"agent": "famille__alex", "client_id": "C0FFEE00-0000-4000-8000-0000000000C1",
                        "text": "Quel temps samedi ?"})
    assert sent.status_code == 200 and sent.json()["state"] == "started", sent.text
    # the card shows in everyone's conversation, said whose
    assert wait(lambda: page(a, "famille")["pending"]["approvals"], 15)
    assert page(a, "famille")["pending"]["approvals"][0]["by"] == {"name": "Marie", "mine": False}
    assert wait(lambda: page(b, "famille__alex")["pending"]["approvals"], 15)
    marie = page(b, "famille__alex")
    assert marie["pending"]["approvals"][0]["by"] == {"name": "Marie", "mine": True}
    assert marie["pending"]["run"]["by"] == {"name": "Marie", "mine": True}
    paul = page(c, "famille__alex")
    assert paul["pending"]["run"]["by"] == {"name": "Marie", "mine": False}
    assert paul["pending"]["approvals"][0]["by"] == {"name": "Marie", "mine": False}
    # Marie's, not Alex's: out of his Boîte, never pushed to him, never answered by his level; in her Boîte
    assert all(x.get("run_id") != "r1" for x in a.get("/v1/approvals", headers=AUTH).json()["items"])
    assert wait(lambda: any(x.get("run_id") == "r1" for x in b.get("/v1/approvals", headers=AUTH).json()["items"]), 15)
    refused = a.post("/v1/approve", headers=AUTH, json={"agent": "famille", "run_id": "r1", "request_id": "req-1", "choice": "once"})
    assert refused.status_code == 403
    assert not any(p.get("t") == "approval" for p in pushed(fakes, "alex")), pushed(fakes, "alex")
    answered = b.post("/v1/approve", headers=AUTH,
                      json={"agent": "famille__alex", "run_id": "r1", "request_id": "req-1", "choice": "once"})
    assert answered.status_code == 200, answered.text
    assert fakes.hermes.approvals and fakes.hermes.approvals[-1]["run_id"] == "r1"
    gate.set()


def test_several_family_agents_each_for_whom_its_creator_chose(bridges, fakes):
    a, b, c = family(bridges, fakes)
    # « Famille » for the whole family, « Budget » for Marie only
    (a.state_dir / "family-agent.json").write_text(json.dumps(
        {"agent": "famille", "agents": ["famille", "budget"], "created": 1.0, "lang": "fr",
         "members": {"budget": ["marie"]}, "common": {"famille": 1.0, "budget": 1.0}}))
    mine = a.get("/v1/family-agent", headers=AUTH).json()
    assert [x["id"] for x in mine["agents"]] == ["famille", "budget"] and mine["agent"]["id"] == "famille"
    assert mine["can_create"] is False   # an install that doesn't manage its agents (by hand)
    budget = mine["agents"][1]
    assert budget["everyone"] is False
    assert sorted((m["id"], m["in"]) for m in budget["members"]) == [("marie", True), ("paul", False)]
    assert mine["agents"][0]["everyone"] is True

    ids = lambda bridge: sorted(x["id"] for x in bridge.get("/v1/family-agent", headers=AUTH).json()["agents"])
    assert ids(b) == ["budget__alex", "famille__alex"]
    assert ids(c) == ["famille__alex"]
    assert sorted(x["id"] for x in c.get("/v1/shared-agents", headers=AUTH).json()["agents"]) == ["famille__alex"]
    # « Nouveau » per agent
    assert b.post("/v1/family-agent/seen", headers=AUTH, json={"agent": "budget__alex"}).json() == {"ok": True}
    news = {x["id"]: x["new"] for x in b.get("/v1/family-agent", headers=AUTH).json()["agents"]}
    assert news == {"famille__alex": True, "budget__alex": False}

    # the whole family, then Paul only: Marie loses it at once, her bridge told
    whole = a.put("/v1/family-agent/budget/members", headers=AUTH, json={"members": None})
    assert whole.status_code == 200 and whole.json()["everyone"] is True, whole.text
    assert wait(lambda: "budget__alex" in [x["id"] for x in c.get("/v1/shared-agents", headers=AUTH).json()["agents"]])
    only = a.put("/v1/family-agent/budget/members", headers=AUTH, json={"members": ["paul"]})
    assert only.status_code == 200 and only.json()["everyone"] is False
    assert wait(lambda: "budget__alex" not in [x["id"] for x in b.get("/v1/shared-agents", headers=AUTH).json()["agents"]])
    refused = b.post("/v1/threads/main/messages", headers=AUTH,
                     json={"agent": "budget__alex", "client_id": "C0FFEE00-0000-4000-8000-0000000000D1", "text": "Hello"})
    assert refused.status_code in (403, 404, 409), refused.text
    assert a.put("/v1/family-agent/budget/members", headers=AUTH, json={"members": ["ghost"]}).status_code == 422
    assert a.put("/v1/family-agent/vie/members", headers=AUTH, json={"members": None}).status_code == 404


def test_the_conversations_of_before_go_once(bridges, fakes):
    def change(c):
        c["agents"] = {"Famille": {"hermes_url": f"{fakes.hermes.url}/p/famille", "hermes_key": "famille-key-" + "x" * 24}}

    bridge = bridges.new("alex", change=change)
    bridge.config["public_url"] = bridge.url
    bridge.state_dir.mkdir(parents=True, exist_ok=True)
    (bridge.state_dir / "family-agent.json").write_text(json.dumps({"agent": "famille", "created": 1.0, "lang": "fr"}))
    (bridge.state_dir / "agent-shares.json").write_text(json.dumps(
        {"conversations": {"c1": {"agent": "famille", "household": "marie", "session": "s-marie", "run": None}},
         "tokens": {"marie": {"famille": {"input": 10, "output": 2}}}}))
    fakes.hermes.add_session("s-marie", [{"role": "user", "content": "Mon secret"}], title="BipAgents · proche marie c1",
                             agent="famille")
    fakes.hermes.add_session("s-main", [{"role": "user", "content": "Bonjour"}], title="Bot Chat", agent="famille")
    bridge.start()
    assert wait(lambda: {"s-marie", "s-main"} <= set(fakes.hermes.deleted), 20), fakes.hermes.deleted
    state = json.loads((bridge.state_dir / "family-agent.json").read_text())
    assert "famille" in state["common"]
    shares = json.loads((bridge.state_dir / "agent-shares.json").read_text())
    assert shares["conversations"] == {} and shares["tokens"]["marie"]["famille"]["input"] == 10   # her credit kept


def test_the_familys_pages_audio_and_routines_are_everyones(bridges, fakes):
    """In Marie's turn: a page (the family's, marked hers, never in anyone's Carnet, served to every member), an audio
    (into the agent's pool: every member's bridge serves it), a routine (her card: hers to start, not Alex's)."""
    a, b, c = family(bridges, fakes)
    make_family_agent(a)
    discover(b, c)
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), gate, sse("run.completed", run_id="r1", output="Voilà.")]
    sent = b.post("/v1/threads/main/messages", headers=AUTH,
                  json={"agent": "famille__alex", "client_id": "C0FFEE00-0000-4000-8000-0000000000E1",
                        "text": "Le menu de Noël en page, lu à voix haute, et chaque lundi les menus ?"})
    assert sent.status_code == 200 and sent.json()["state"] == "started", sent.text
    give_token(a, "famille", TOKEN)

    # a page: the family's, made at Marie's request, without the Carnet link it was given
    made = call(a, "create_page", {"html": "<h1>Menu de Noël</h1><p>Dinde, bûche.</p>", "title": "Menu de Noël",
                                   "notebook": "Courses"})
    assert made["isError"] is False, text_of(made)
    path = media_of(made)
    page = path.rsplit("/", 1)[1][:-5]
    listed = {p["page"]: p for p in a.get("/v1/pages", headers=AUTH, params={"agent": "famille"}).json()["pages"]}
    assert listed[page]["group"] is True and listed[page]["asked_by"] == "Marie" and "notebook" not in listed[page]
    # every member's bridge opens it, Paul's too (not only the household that asked)
    for bridge in (b, c):
        shown = bridge.get("/v1/media", headers=AUTH, params={"path": f"/bipagents/pages/famille__alex/{page}.html"})
        assert shown.status_code == 200 and "Menu de Noël" in shown.text and "Marie" in shown.text, shown.text

    # an audio: into the agent's pool, sent by the agent, served by every member's bridge
    audio = call(a, "create_audio", {"text": "Le menu de Noël : dinde et bûche.", "title": "Menu de Noël"})
    assert audio["isError"] is False, text_of(audio)
    files = c.get("/v1/shared-agents/famille__alex/files", headers=AUTH).json()["files"]
    made_audio = [f for f in files if f["name"].startswith("Menu_de_No") and f["mime"].startswith("audio/")]
    assert made_audio and made_audio[0]["mine"] is False, files
    fetched = c.get(made_audio[0]["url"], headers=AUTH)
    assert fetched.status_code == 200 and len(fetched.content) > 0

    # a routine she asks for: her card (in her conversation, not in Alex's Boîte, not his to answer)
    asked = call(a, "routine_schedule", {"action": "create", "name": "Menus du lundi", "schedule": "every monday 18h",
                                         "prompt": "Propose les menus de la semaine à la famille."})
    assert asked["isError"] is False, text_of(asked)
    cards = lambda bridge, agent: [x for x in page_of(bridge, agent)["pending"]["approvals"]  # noqa: E731
                                   if str(x.get("run_id", "")).startswith("bip-rout-")]
    mine = cards(b, "famille__alex")
    assert len(mine) == 1 and mine[0]["by"]["mine"] is True, page_of(b, "famille__alex")["pending"]
    assert cards(c, "famille__alex") == [] and cards(a, "famille") == []
    assert not [i for i in a.get("/v1/approvals", headers=AUTH).json()["items"] if i["run_id"].startswith("bip-rout-")]
    refused = a.post("/v1/approve", headers=AUTH, json={"agent": "famille", "run_id": mine[0]["run_id"], "choice": "once"})
    assert refused.status_code == 403, refused.text
    answered = b.post("/v1/approve", headers=AUTH,
                      json={"agent": "famille__alex", "run_id": mine[0]["run_id"], "choice": "once"})
    assert answered.status_code == 200, answered.text
    jobs = a.get("/v1/agents/famille/routines", headers=AUTH).json()["jobs"]
    assert [j["name"] for j in jobs if j.get("enabled")] == ["Menus du lundi"], jobs
    assert cards(b, "famille__alex") == []
    gate.set()


def page_of(bridge, agent):
    return page(bridge, agent)
