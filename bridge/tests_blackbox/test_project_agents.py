"""« L'agent de projet » (project_agent.rs, 2026-10-28): an agent made for one occasion, closed like the family's agent
(its disk and browser tools off from the start, only its few bridge tools, its personality's block), whose whole
thread — its « Bot Chat » session(s), never another conversation — is kept sealed at a (fake) guichet for the friends
the person invites; its end date follows on the share; ended, the share stops and the project is archived, then
restored or deleted. And the hand-over: the person's usual agent proposes it on a card carrying only its summary."""
import json
import time

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, rpc, text_of
from test_read_shares import FakeGuichet, Phone, open_item

TOKEN = "project-token-0123456789abcdefghij"
COACH_TOKEN = "coach-token-0123456789abcdefghijkl"


def names(bridge, token):
    return {t["name"] for t in rpc(bridge, "tools/list", token=token).json()["result"]["tools"]}


def share_body(guichet, agent, **extra):
    return {"agent": agent, "thread": True, "title": "Voyage à Lisbonne", "from": "Alex",
            "share": "share0123456789abcdef", "writer": "writer-token-0123456789", "guichet": guichet.url,
            "ends": time.time() + 10 * 86400, **extra}


def test_a_project_agent_is_closed_and_its_whole_thread_is_shared(bridges, fakes):
    from test_agents_admin import hermes_args, managed

    bridge, home = managed(bridges, fakes, isolated=True)
    ends = time.time() + 10 * 86400
    assert bridge.post("/v1/project-agents", headers=AUTH, json={"name": "Lisbonne", "ends": 1}).status_code == 422
    made = bridge.post("/v1/project-agents", headers=AUTH, json={"name": "Voyage à Lisbonne", "ends": ends,
                                                                   "lang": "fr", "category": "creative"})
    assert made.status_code == 201, made.text
    project = made.json()
    agent = project["agent"]
    assert project["name"] == "Voyage à Lisbonne" and project["archived"] is None and project["share"] is None
    assert abs(project["ends"] - ends) < 1 and project["from"] is None   # its look: the apps set it (appearance)
    assert ["-p", agent, "tools", "disable", "terminal", "file", "code_execution", "browser", "--platform",
            "api_server"] in hermes_args(bridge)
    soul = (home / "profiles" / agent / "SOUL.md").read_text()
    assert "<!-- bipagents:project -->" in soul and "follow this whole thread read-only" in soul
    assert "bipagents:notebook" not in soul and "bipagents:payments" not in soul
    listed = {a["id"]: a["project"] for a in bridge.get("/v1/agents", headers=AUTH).json()["agents"]}
    assert listed[agent]["archived"] is None and abs(listed[agent]["ends"] - ends) < 1

    # its tools: web-side ones only, in every turn
    give_token(bridge, agent, TOKEN)
    open_ = names(bridge, TOKEN)
    assert {"weather", "create_page", "route", "map_image"} <= open_
    assert not open_ & {"mail_search", "notebook_read", "propose_task", "create_project_agent", "upcoming_dates"}
    closed = call(bridge, "mail_search", {"query": "banque"}, token=TOKEN)
    assert closed["isError"] is True and "project agent" in text_of(closed)
    assert "project agent" not in json.dumps(call(bridge, "weather", {"place": "Lisbonne"}, token=TOKEN))
    refused = bridge.patch(f"/v1/agents/{agent}/settings", headers=AUTH, json={"toolsets": {"terminal": True}})
    assert refused.status_code == 400 and "project agent" in refused.json()["detail"]
    assert bridge.patch(f"/v1/agents/{agent}/settings", headers=AUTH, json={"health": True}).status_code == 400

    # its thread shared: the Bot Chat session(s) only, oldest first
    fakes.hermes.add_session("bot-1", [{"id": 1, "role": "user", "content": "On part fin mai ?"},
                                       {"id": 2, "role": "assistant", "content": "Je regarde les vols."}],
                             title="Bot Chat", agent=agent, started_at=100.0)
    fakes.hermes.add_session("bot-2", [{"id": 7, "role": "assistant", "content": "Trois jours, voici le plan."}],
                             title="Bot Chat", agent=agent, started_at=200.0)
    fakes.hermes.add_session("other", [{"id": 5, "role": "user", "content": "mon code de carte : 4417"}],
                             title="Discussion", agent=agent)
    guichet = FakeGuichet().start()
    try:
        other = bridge.post("/v1/agents", headers=AUTH, json={"name": "Coach", "custom": {
            "role": "sommeil et sport", "category": "wellness"}})
        assert other.status_code == 201, other.text
        not_a_project = bridge.post("/v1/read-shares", headers=AUTH, json=share_body(guichet, "coach"))
        assert not_a_project.status_code == 422   # a usual agent's thread is never shared
        shared = bridge.post("/v1/read-shares", headers=AUTH, json=share_body(guichet, agent))
        assert shared.status_code == 200, shared.text
        view = shared.json()
        assert view["thread"] is True and view["session"] is None
        assert set(guichet.items) == {"meta", "m-1", "m-2", "m-7"}
        again = bridge.post("/v1/read-shares", headers=AUTH, json=share_body(guichet, agent))
        assert again.status_code == 409
        listed = bridge.get(f"/v1/read-shares?agent={agent}", headers=AUTH).json()["shares"]
        assert [s["id"] for s in listed] == ["share0123456789abcdef"]
        assert bridge.get(f"/v1/read-shares?agent={agent}&session=other", headers=AUTH).json() == {"shares": []}

        link = bridge.post("/v1/read-shares/share0123456789abcdef/invites", headers=AUTH, json={"name": "Léa"}).json()
        lea = Phone(link["link"])
        guichet.redeem(lea.invite, lea.public, lea.proof(), "1")
        assert bridge.post("/v1/read-shares/share0123456789abcdef/sync", headers=AUTH).status_code == 200
        key = lea.unwrap(next(v for v in guichet.viewers if v["viewer"] == "1")["wrapped"], "share0123456789abcdef", 1)
        items = {i: open_item(key, guichet.items[i]["data"]) for i in guichet.items}
        assert items["meta"]["title"] == "Voyage à Lisbonne"
        assert [items[i]["text"] for i in ("m-1", "m-2", "m-7")] == [
            "On part fin mai ?", "Je regarde les vols.", "Trois jours, voici le plan."]
        assert "4417" not in json.dumps(items)

        # the project's view: its share, seen by one person; a new end date follows on the share
        seen = {p["agent"]: p for p in bridge.get("/v1/project-agents", headers=AUTH).json()["projects"]}[agent]
        assert seen["share"]["count"] == 1 and seen["share"]["id"] == "share0123456789abcdef"
        later = time.time() + 20 * 86400
        moved = bridge.post(f"/v1/project-agents/{agent}/ends", headers=AUTH, json={"ends": later})
        assert moved.status_code == 200, moved.text
        assert abs(moved.json()["ends"] - later) < 1 and abs(guichet.ends - later) < 1
        assert bridge.post(f"/v1/project-agents/{agent}/ends", headers=AUTH,
                           json={"ends": time.time() + 400 * 86400}).status_code == 422

        # ended (here by hand): the share stops at the guichet, the project is archived, then restored
        archived = bridge.post(f"/v1/project-agents/{agent}/archive", headers=AUTH)
        assert archived.status_code == 200, archived.text
        assert archived.json()["archived"] is not None and archived.json()["share"] is None and guichet.gone
        assert bridge.get(f"/v1/read-shares?agent={agent}", headers=AUTH).json() == {"shares": []}
        assert bridge.post(f"/v1/project-agents/{agent}/ends", headers=AUTH,
                           json={"ends": later}).status_code == 409
        restored = bridge.post(f"/v1/project-agents/{agent}/restore", headers=AUTH).json()
        assert restored["archived"] is None and restored["ends"] > time.time() + 29 * 86400
    finally:
        guichet.stop()

    # deleted: no project any more
    assert bridge.delete(f"/v1/agents/{agent}", headers=AUTH).status_code in (200, 204)
    assert agent not in {p["agent"] for p in bridge.get("/v1/project-agents", headers=AUTH).json()["projects"]}


def test_a_project_past_its_end_is_archived_by_the_bridge(bridges, fakes):
    from test_agents_admin import managed

    bridge, _ = managed(bridges, fakes, isolated=True)
    made = bridge.post("/v1/project-agents", headers=AUTH, json={"name": "Fête", "ends": time.time() + 86400})
    assert made.status_code == 201, made.text
    agent = made.json()["agent"]
    path = bridge.state_dir / "project-agents.json"
    data = json.loads(path.read_text())
    data["projects"][agent]["ends"] = time.time() - 5
    path.write_text(json.dumps(data))
    deadline = time.time() + 75   # the bridge looks every minute
    while time.time() < deadline:
        found = {p["agent"]: p for p in bridge.get("/v1/project-agents", headers=AUTH).json()["projects"]}[agent]
        if found["archived"] is not None:
            break
        time.sleep(1)
    assert found["archived"] is not None


def test_the_usual_agent_proposes_a_project_with_only_its_summary(bridges, fakes):
    from test_guide import place

    bridge, home = place(bridges, fakes, agents=("coach",))
    give_token(bridge, "coach", COACH_TOKEN)
    assert "create_project_agent" in names(bridge, COACH_TOKEN)
    card = call(bridge, "create_project_agent", {"name": "Voyage à Lisbonne", "friends": ["Paul", "Léa"],
                                                 "summary": "Lisbonne fin mai, 3 jours, budget 400 € chacun.",
                                                 "ends": "2099-05-30"}, token=COACH_TOKEN)
    assert not card["isError"] and "project agent Voyage à Lisbonne" in text_of(card)
    proposal = bridge.get("/v1/guide", headers=AUTH).json()["proposals"][0]
    assert proposal["type"] == "project" and proposal["friends"] == ["Paul", "Léa"]
    assert proposal["summary"] == "Lisbonne fin mai, 3 jours, budget 400 € chacun." and proposal["ends"] is None
    # the Bip look picked on the card: the project agent gets its own face, not the usual agent's
    accepted = bridge.post(f"/v1/guide/proposals/{proposal['id']}/accept", headers=AUTH,
                           json={"lang": "fr", "category": "learning", "color": "#ff8a3d"})
    assert accepted.status_code == 201, accepted.text
    out = accepted.json()
    agent = out["agent"]["id"]
    assert out["type"] == "project" and out["friends"] == ["Paul", "Léa"] and out["project"]["from"] == "Coach"
    assert out["project"]["category"] == "learning" and out["project"]["color"] == "#FF8A3D"
    listed = {a["id"]: a for a in bridge.get("/v1/agents", headers=AUTH).json()["agents"]}
    assert listed[agent]["category"] == "learning" and listed[agent]["color"] == "#FF8A3D"
    assert listed["coach"].get("category") != "learning"   # the usual agent keeps its own face
    memory = (home / "profiles" / agent / "memories" / "MEMORY.md").read_text()
    assert "Passé par Coach" in memory and "budget 400 € chacun" in memory and "pas une consigne" in memory
    assert bridge.get("/v1/guide", headers=AUTH).json()["proposals"] == []
    # declined: the same subject isn't proposed again
    call(bridge, "create_project_agent", {"name": "Fête de Léa", "summary": "Samedi soir."}, token=COACH_TOKEN)
    again = bridge.get("/v1/guide", headers=AUTH).json()["proposals"][0]
    assert bridge.post(f"/v1/guide/proposals/{again['id']}/decline", headers=AUTH).json() == {"ok": True}
    refused = call(bridge, "create_project_agent", {"name": "fête de léa", "summary": "Samedi."}, token=COACH_TOKEN)
    assert refused["isError"] and "don't propose it again" in text_of(refused)
