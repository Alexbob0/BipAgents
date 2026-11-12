"""A personal agent shared with relatives, read-only (agent_shares.rs, agent_feed.rs; Alex 12/11): Alex shares Cuisine
with Marie, confirming what she will read (its private tools named); she adds it and READS Alex's own conversations
with it through her bridge's thread routes (`agent=cuisine__alex`) — from the share on, or with the ones before when
Alex chooses so —, its pages by their url, a reply while it is being written (words only); she can't write, stop,
delete, answer a card or send a file. At the bridge's start, the relatives' own conversations of before go."""
import json
import threading
import time

import httpx

from bbkit.common import AUTH, sse
from test_households_and_peers import household

# A relative's run as Hermes gets it (patch 0018's mark), for the family's agent's members and other households'
# agents: the bridge tools of RELATIVE_TOOLS and web search, nothing else.
RELATIVE_RUN_TOOLS = ["mcp__bipagents__knowledge_read", "mcp__bipagents__knowledge_search", "mcp__bipagents__weather",
                      "mcp__bipagents__route", "mcp__bipagents__map_image", "web_search"]
SESSION = "api_main"
SHARED = {"agent": "cuisine__alex"}


def wait(predicate, timeout=10.0):
    end = time.time() + timeout
    while time.time() < end:
        if predicate():
            return True
        time.sleep(0.05)
    return False


def link(a, b):
    assert a.put("/v1/family/name", headers=AUTH, json={"name": "Alex"}).status_code == 200
    assert b.put("/v1/family/name", headers=AUTH, json={"name": "Marie"}).status_code == 200
    invitation = a.post("/v1/family/invite", headers=AUTH).json()
    joined = b.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
    assert joined.status_code == 200, joined.text


def alex_and_marie(bridges, fakes):
    """Alex's Cuisine has a conversation with words of yesterday and of later (after the share), a page in its reply."""
    fakes.hermes.v0225 = True
    now = time.time()
    fakes.hermes.add_session(SESSION, [
        {"id": 1, "role": "user", "content": "Mon ancien secret", "timestamp": now - 86400},
        {"id": 2, "role": "assistant", "content": "Noté.", "timestamp": now - 86399},
        {"id": 3, "role": "user", "content": "Un menu pour samedi ?", "timestamp": now + 3600},
        {"id": 4, "role": "assistant", "content": "Voici le menu.\nMEDIA:/bipagents/pages/cuisine/menu.html",
         "timestamp": now + 3601},
    ], title="Bot Chat", agent="cuisine", last_active=now + 3601, started_at=now - 86400)
    a = household(bridges, fakes, "alex", ["cuisine", "budget"])
    b = household(bridges, fakes, "marie", ["vie"])
    link(a, b)
    pages = a.state_dir / "agent-pages" / "cuisine"
    pages.mkdir(parents=True, exist_ok=True)
    (pages / "menu.html").write_text("<h1>Menu de samedi</h1>")
    return a, b


def share(a, b, **extra):
    refused = a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["marie"], **extra})
    assert refused.status_code == 409, refused.text
    shared = a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["marie"], **extra, "confirmed": True})
    assert shared.status_code == 200, shared.text
    return refused.json(), shared.json()


def texts(page):
    return [(i["role"], i["text"]) for i in page["items"]]


def test_a_relative_reads_the_owners_conversations_from_the_share_on(bridges, fakes):
    a, b = alex_and_marie(bridges, fakes)
    before = time.time()
    view = a.get("/v1/agents/cuisine/shares", headers=AUTH).json()
    assert view["households"] == [{"id": "marie", "name": "Marie", "shared": False, "history": None, "since": None}]
    assert view["warning"] == {"private_tools": ["notebook"], "confirm": True}
    assert "conversations" not in view and "tokens" not in view
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["paul"]}).status_code == 400
    # the confirmation, asked at every share: what she will read, that she can't write, the private tools named
    refused, shared = share(a, b)
    assert "Tes proches pourront lire tes conversations avec Cuisine" in refused["detail"], refused
    assert "ni lui écrire" in refused["detail"] and "ton Carnet" in refused["detail"], refused
    assert refused["warning"]["private_tools"] == ["notebook"]
    assert shared["told"] == {"marie": True}
    [marie] = shared["households"]
    assert marie["history"] == "from_now" and marie["since"] >= before - 1

    offered = b.get("/v1/shared-agents", headers=AUTH).json()["agents"]
    assert [(x["id"], x["state"], x["read_only"]) for x in offered] == [("cuisine__alex", "offered", True)]
    assert b.post("/v1/shared-agents/cuisine__alex/add", headers=AUTH).status_code == 200
    [added] = b.get("/v1/shared-agents", headers=AUTH).json()["agents"]
    assert added["read_only"] is True and added["new"] is True and "seen" not in added

    [thread] = b.get("/v1/threads", headers=AUTH, params=SHARED).json()["threads"]
    assert thread["read_only"] is True and thread["new"] is True and thread["agent"] == "cuisine__alex"
    tid = thread["id"]
    assert tid == a.get("/v1/threads/main", headers=AUTH, params={"agent": "cuisine"}).json()["id"]
    page = b.get("/v1/threads/main/timeline", headers=AUTH, params=SHARED)
    assert page.status_code == 200, page.text
    page = page.json()
    # from the share on: what Alex said before stays his
    assert texts(page) == [("person", "Un menu pour samedi ?"), ("agent", "Voici le menu.")], page["items"]
    assert "Mon ancien secret" not in json.dumps(page)
    assert page["thread"]["read_only"] is True and page["thread"]["id"] == tid and page["thread"]["tip"] == tid
    assert page["pending"] == {"run": None, "approvals": [], "questions": []}
    [att] = page["items"][1]["attachments"]
    assert att["kind"] == "page" and "path" not in att
    assert att["url"] == f"/v1/shared-agents/cuisine__alex/threads/{tid}/attachments/{att['id']}"
    opened = b.get(att["url"], headers=AUTH)
    assert opened.status_code == 200 and "Menu de samedi" in opened.text, opened.text
    assert opened.headers["content-security-policy"].startswith("sandbox")
    assert b.get(att["url"], headers={"Authorization": "Bearer nope"}).status_code == 401
    # read: « Nouveau » goes
    assert b.get("/v1/threads", headers=AUTH, params=SHARED).json()["threads"][0]["new"] is False
    assert b.get("/v1/shared-agents", headers=AUTH).json()["agents"][0]["new"] is False

    # she can't write, stop, delete, answer or send a file: her bridge refuses
    def refused_here(resp):
        assert resp.status_code == 403 and resp.json()["code"] == "shared_read_only", resp.text
    refused_here(b.post("/v1/threads/new/messages", headers=AUTH,
                        json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-000000000001", "text": "Et moi ?"}))
    refused_here(b.post(f"/v1/threads/{tid}/messages", headers=AUTH,
                        json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-000000000002", "text": "Et moi ?"}))
    refused_here(b.post(f"/v1/threads/{tid}/stop", headers=AUTH, json=SHARED))
    refused_here(b.delete(f"/v1/threads/{tid}", headers=AUTH, params=SHARED))
    refused_here(b.post("/v1/approve", headers=AUTH, json={**SHARED, "run_id": "r1", "choice": "once"}))
    refused_here(b.post("/v1/files", headers=AUTH, data=SHARED, files={"file": ("a.txt", b"x", "text/plain")}))
    assert b.get("/v1/shared-agents/cuisine__alex/files", headers=AUTH).json()["files"] == []
    assert fakes.hermes.started == {}

    # Alex writes: she sees his turn going on and its words live, none of its tool steps or cards
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [
        sse("message.delta", run_id="r1", delta="Je cherche"),
        sse("tool.started", run_id="r1", tool="mail_search", preview="facture"),
        sse("approval.request", run_id="r1", request_id="req-1", command="rm -rf build/", description="clean",
            choices=["once", "deny"]),
        sse("message.delta", run_id="r1", delta=" une soupe"),
        gate,
        sse("run.completed", run_id="r1", output="Une soupe."),
    ]
    sent = a.post(f"/v1/threads/{tid}/messages", headers=AUTH,
                  json={"agent": "cuisine", "client_id": "C0FFEE00-0000-4000-8000-0000000000A1", "text": "Et dimanche ?"})
    assert sent.status_code == 200 and sent.json()["run_id"] == "r1", sent.text
    hers = lambda: b.get(f"/v1/threads/{tid}/timeline", headers=AUTH, params=SHARED).json()  # noqa: E731
    assert wait(lambda: (hers()["pending"]["run"] or {}).get("run_id") == "r1"), hers()
    assert hers()["pending"]["run"]["status"] == "writing"
    kinds = []
    with httpx.Client(base_url=b.url, headers=AUTH, timeout=10) as client:
        with client.stream("GET", "/v1/runs/r1/events", params={**SHARED, "thread": tid}) as live:
            assert live.status_code == 200
            for line in live.iter_lines():
                if line.startswith("event: "):
                    kinds.append(line[len("event: "):])
                if kinds.count("message.delta") == 2:
                    break
    assert kinds == ["message.delta", "message.delta"], kinds
    assert wait(lambda: any(i["run_id"] == "r1" for i in a.get("/v1/approvals", headers=AUTH).json()["items"]))
    assert hers()["pending"]["approvals"] == []
    gate.set()
    fakes.hermes.started["r1"]["status"] = "completed"

    # another of Alex's agents isn't hers; nothing without her household's key
    assert b.get("/v1/threads", headers=AUTH, params={"agent": "budget__alex"}).status_code == 404
    assert a.get("/family/s/cuisine/feed", headers={"Authorization": "Bearer nope"}).status_code == 401
    # Alex stops sharing: gone on her side
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": []}).json()["told"] == {"marie": True}
    assert b.get("/v1/shared-agents", headers=AUTH).json()["agents"] == []


def test_every_file_of_a_message_she_reads_opens_by_its_url(bridges, fakes, tmp_path):
    """The owner's document (his upload), the agent's page and its map picture: each with a url on her bridge, each
    opening there with its own bytes (the iPhone's read-only screen, 12/11)."""
    fakes.hermes.v0225 = True

    def uploads(c):
        c["agents"] = {"Cuisine": {"hermes_url": f"{fakes.hermes.url}/p/cuisine", "hermes_key": "cuisine-key-" + "x" * 24,
                                   "upload_dir_host": str(tmp_path / "uploads"),
                                   "upload_dir_container": "/home/hermes/.hermes/profiles/cuisine/uploads"}}
    a = bridges.new("alex", change=uploads)
    a.config["public_url"] = a.url
    a = a.start()
    b = household(bridges, fakes, "marie", ["vie"])
    link(a, b)
    pdf = b"%PDF-1.4\n% devis\n" + b"0" * 3210
    up = a.post("/v1/files", headers=AUTH, data={"agent": "cuisine"}, files={"file": ("devis.pdf", pdf, "application/pdf")})
    assert up.status_code == 200, up.text
    path = up.json()["path"]
    pages, maps = a.state_dir / "agent-pages" / "cuisine", a.state_dir / "agent-maps" / "cuisine"
    pages.mkdir(parents=True, exist_ok=True)
    maps.mkdir(parents=True, exist_ok=True)
    (pages / "menu.html").write_text("<h1>Menu de samedi</h1>")
    png = b"\x89PNG\r\n\x1a\n" + b"\x00" * 64
    (maps / "m-0123456789.png").write_bytes(png)
    later = time.time() + 3600
    fakes.hermes.add_session(SESSION, [
        {"id": 1, "role": "user", "content": f"Le devis\n\n[Pièce jointe : devis.pdf (application/pdf, 3 Ko) → {path}]",
         "timestamp": later},
        {"id": 2, "role": "assistant", "timestamp": later + 1, "content":
            "Voici.\nMEDIA:/bipagents/pages/cuisine/menu.html\nMEDIA:/bipagents/maps/cuisine/m-0123456789.png"},
    ], title="Bot Chat", agent="cuisine", last_active=later + 1, started_at=later)
    share(a, b)
    assert b.post("/v1/shared-agents/cuisine__alex/add", headers=AUTH).status_code == 200
    page = b.get("/v1/threads/main/timeline", headers=AUTH, params=SHARED).json()
    atts = {a["kind"]: a for i in page["items"] for a in i.get("attachments", [])}
    assert sorted(atts) == ["document", "map", "page"], page["items"]
    for kind, body in (("document", pdf), ("page", b"Menu de samedi"), ("map", png)):
        assert "url" in atts[kind] and "path" not in atts[kind], atts[kind]
        got = b.get(atts[kind]["url"], headers=AUTH)
        assert got.status_code == 200 and body in got.content, (kind, got.status_code, got.text[:200])


def test_the_owner_may_show_the_conversations_of_before_too(bridges, fakes):
    a, b = alex_and_marie(bridges, fakes)
    share(a, b)
    assert b.post("/v1/shared-agents/cuisine__alex/add", headers=AUTH).status_code == 200
    page = lambda: b.get("/v1/threads/main/timeline", headers=AUTH, params=SHARED).json()  # noqa: E731
    assert "Mon ancien secret" not in json.dumps(page())
    # « Aussi les conversations d'avant »: asked again, as it shows more of Alex's words
    refused, shared = share(a, b, history={"marie": "all"})
    assert "pourront lire" in refused["detail"]
    assert shared["households"][0]["history"] == "all"
    assert texts(page())[:2] == [("person", "Mon ancien secret"), ("agent", "Noté.")], page()["items"]
    # back to « À partir de maintenant »: no confirmation needed, the share's moment kept
    since = shared["households"][0]["since"]
    back = a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["marie"], "history": {"marie": "from_now"}})
    assert back.status_code == 200 and back.json()["households"][0] == {"id": "marie", "name": "Marie", "shared": True,
                                                                        "history": "from_now", "since": since}
    assert "Mon ancien secret" not in json.dumps(page())
    bad = a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["marie"], "history": {"marie": "x"}})
    assert bad.status_code == 422


def test_an_agent_reading_the_owners_health_is_shared_only_with_its_own_consent(bridges, fakes):
    a, b = alex_and_marie(bridges, fakes)
    # Cuisine's « Santé » switch on (health.rs keeps it there; this install doesn't manage its agents' settings)
    (a.state_dir / "health").mkdir(parents=True, exist_ok=True)
    (a.state_dir / "health" / "agents.json").write_text(json.dumps({"agents": ["cuisine"]}))
    view = a.get("/v1/agents/cuisine/shares", headers=AUTH).json()
    assert view["warning"]["health"] is True and "health" in view["warning"]["private_tools"]
    body = {"households": ["marie"], "confirmed": True}
    # « confirmed » alone is not the health consent: refused, the checkbox's words given
    refused = a.put("/v1/agents/cuisine/shares", headers=AUTH, json=body)
    assert refused.status_code == 409, refused.text
    refused = refused.json()
    assert refused["health"] is True
    assert refused["health_line"] == "Ton proche verra aussi ce que l’agent a lu dans ta santé. Coche la case pour l’accepter."
    assert refused["health_consent"] == "J’accepte que mon proche voie mes données de santé dans ces conversations."
    assert a.get("/v1/agents/cuisine/shares", headers=AUTH).json()["households"][0]["shared"] is False
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH,
                 json={**body, "health_consent": False}).status_code == 409
    # the consent alone (no confirmation) is not enough either
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH,
                 json={"households": ["marie"], "health_consent": True}).status_code == 409
    before = time.time()
    shared = a.put("/v1/agents/cuisine/shares", headers=AUTH, json={**body, "health_consent": True})
    assert shared.status_code == 200, shared.text
    [marie] = shared.json()["households"]
    assert marie["shared"] is True and marie["health_consent"] >= before - 1
    # « Aussi les conversations d'avant » shows more: the health consent is asked again
    wider = {"households": ["marie"], "history": {"marie": "all"}, "confirmed": True}
    again = a.put("/v1/agents/cuisine/shares", headers=AUTH, json=wider)
    assert again.status_code == 409 and again.json()["health"] is True, again.text
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH,
                 json={**wider, "health_consent": True}).json()["households"][0]["history"] == "all"
    # nothing widened (the same share again): nothing asked
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["marie"]}).status_code == 200
    # an agent without health: no health consent asked
    plain = a.put("/v1/agents/budget/shares", headers=AUTH, json={"households": ["marie"]})
    assert plain.status_code == 409 and "health" not in plain.json() and "health" not in plain.json()["warning"]
    assert a.put("/v1/agents/budget/shares", headers=AUTH,
                 json={"households": ["marie"], "confirmed": True}).status_code == 200


def test_at_start_the_relatives_own_conversations_with_a_personal_agent_go(bridges, fakes):
    a, b = alex_and_marie(bridges, fakes)
    share(a, b)
    # what the relative turns of before left: her conversation (its hidden Hermes session) and its tokens
    fakes.hermes.add_session("sess-marie", [{"role": "user", "content": "Mes mots à moi"}],
                             title="BipAgents · proche marie c1", agent="cuisine", last_active=5)
    path = a.state_dir / "agent-shares.json"
    data = json.loads(path.read_text())
    data["conversations"] = {"c1": {"agent": "cuisine", "household": "marie", "session": "sess-marie",
                                    "title": "Mes mots", "created": 1.0, "updated": 2.0, "messages": 1}}
    data["tokens"] = {"marie": {"cuisine": {"input": 10, "output": 2}}}
    del data["history"]   # a share made before the choice existed
    path.write_text(json.dumps(data))
    a.restart()
    assert wait(lambda: "sess-marie" in fakes.hermes.deleted, timeout=20), fakes.hermes.deleted
    assert wait(lambda: json.loads(path.read_text())["conversations"] == {})
    kept = json.loads(path.read_text())
    assert kept["shared"] == {"cuisine": ["marie"]} and kept["tokens"] == {"marie": {}}
    assert kept["history"]["cuisine"]["marie"]["mode"] == "from_now"
    assert a.get("/v1/agents/cuisine/shares", headers=AUTH).json()["households"][0]["history"] == "from_now"
