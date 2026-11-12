"""The family's agent (family_agent.rs), over two linked bridges: Alex's place holds « Famille »; Marie gets it at
once (no « Ajouter »), talks to it in her own conversation with a family member's instructions, edits the family
Carnet from her app (relayed to Alex's place), may bring it into her discussions without a link; its tools are only
the family's, in Alex's own turns too; usage is counted per member; when Alex unlinks Marie, she keeps what is the
family's and is offered to take it over."""
import json

from bbkit.common import AUTH, sse
from bbkit.mcp import call, give_token, rpc
from test_households_and_peers import household
from test_shared_agents import link, wait

TOKEN = "agent-token-for-famille-0123456789"


def make_family_agent(bridge, agent="famille"):
    (bridge.state_dir / "family-agent.json").write_text(json.dumps({"agent": agent, "created": 1.0, "lang": "fr"}))


def test_every_member_gets_the_familys_agent(bridges, fakes):
    a = household(bridges, fakes, "alex", ["famille", "budget"])
    b = household(bridges, fakes, "marie", ["vie"])
    link(a, b)
    make_family_agent(a)

    mine = a.get("/v1/family-agent", headers=AUTH).json()
    assert mine["agent"]["id"] == "famille" and mine["agent"]["mine"] is True and mine["can_create"] is False
    listed = {x["id"]: x["family"] for x in a.get("/v1/agents", headers=AUTH).json()["agents"]}
    assert listed == {"famille": True, "budget": False}

    theirs = b.get("/v1/family-agent", headers=AUTH).json()
    assert theirs["agent"]["id"] == "famille__alex" and theirs["agent"]["mine"] is False
    assert theirs["agent"]["owner"] == "Alex" and theirs["agent"]["new"] is True
    shared = b.get("/v1/shared-agents", headers=AUTH).json()["agents"]
    assert [(x["id"], x["state"], x["family"]) for x in shared] == [("famille__alex", "added", True)]
    assert b.post("/v1/family-agent/seen", headers=AUTH).json() == {"ok": True}
    assert b.get("/v1/family-agent", headers=AUTH).json()["agent"]["new"] is False
    # the shares view lists Marie without any sharing step
    assert a.get("/v1/agents/famille/shares", headers=AUTH).json()["households"][0]["shared"] is True

    # the family Carnet, from both apps
    view = b.post("/v1/family-agent/notebook", headers=AUTH, json={"op": "list_save", "title": "Courses"}).json()
    lid = view["lists"][0]["id"]
    view = b.post("/v1/family-agent/notebook", headers=AUTH, json={"op": "item_add", "list": lid, "text": "Lait"}).json()
    item = view["lists"][0]["items"][0]
    assert item["text"] == "Lait" and item["by"] == "Marie" and item["done"] is False
    bad = b.post("/v1/family-agent/notebook", headers=AUTH, json={"op": "item_add", "list": lid, "text": "x", "due": "demain"})
    assert bad.status_code == 400
    seen = a.get("/v1/family-agent/notebook", headers=AUTH).json()
    assert seen["lists"][0]["items"][0]["text"] == "Lait"
    a.post("/v1/family-agent/notebook", headers=AUTH, json={"op": "item_save", "list": lid, "id": item["id"], "done": True})
    assert b.get("/v1/family-agent/notebook", headers=AUTH).json()["lists"][0]["items"][0]["done"] is True

    # its tools: only the family's, whoever talks (here: no member's turn going on, Alex's own)
    give_token(a, "famille", TOKEN)
    names = {t["name"] for t in rpc(a, "tools/list", token=TOKEN).json()["result"]["tools"]}
    # a page for the family since 12/11 (its pages are everyone's, never in a personal Carnet)
    assert {"family_notebook_read", "family_notebook_write", "weather", "create_page"} <= names
    assert not names & {"mail_search", "propose_task", "notebook_read", "upcoming_dates", "list_pages", "read_page"}
    closed = call(a, "mail_search", {"query": "x"}, token=TOKEN)
    assert closed["isError"] is True and "family's agent" in closed["content"][0]["text"]
    read = call(a, "family_notebook_read", {}, token=TOKEN)
    assert read["isError"] is False and "Lait" in read["content"][0]["text"]
    wrote = call(a, "family_notebook_write", {"op": "item_add", "list": lid, "text": "Pain"}, token=TOKEN)
    assert wrote["isError"] is False
    assert [i["by"] for i in b.get("/v1/family-agent/notebook", headers=AUTH).json()["lists"][0]["items"]] == [
        "Alex", "Famille"]  # Alex ticked « Lait »
    # its settings never open the disk, the browser, Santé (the profile isn't managed here: refused either way)
    assert a.patch("/v1/agents/famille/settings", headers=AUTH, json={"toolsets": {"terminal": True}}).status_code >= 400

    # Marie talks to it in the family's common conversation, as a family member
    fakes.hermes.runs["r1"] = [sse("run.completed", output="Des crêpes !")]
    sent = b.post("/v1/threads/new/messages", headers=AUTH,
                  json={"agent": "famille__alex", "client_id": "C0FFEE00-0000-4000-8000-0000000000F1",
                        "text": "Une idée pour dimanche ?"})
    assert sent.status_code == 200, sent.text
    timeline = f"/v1/threads/{sent.json()['thread']}/timeline"
    run = fakes.hermes.started["r1"]
    assert run["agent"] == "famille" and "You are the family's agent" in run["instructions"]
    # the family's common conversation since 12/11 (test_family_thread.py): a normal run, its memory written from
    # anyone; the family's tools are the bridge's in every turn
    assert run["guest_turn"] is None and sent.json()["thread"] == "family"
    fakes.hermes.started["r1"]["status"] = "completed"
    assert wait(lambda: b.get(timeline, headers=AUTH, params={"agent": "famille__alex"}).json()["pending"]["run"] is None)

    usage = a.get("/v1/family-agent/usage", headers=AUTH).json()["members"]
    assert [m["id"] for m in usage] == ["self", "marie"]
    assert b.get("/v1/family-agent/usage", headers=AUTH).json()["members"][1]["name"] == "Marie"

    # Marie's discussions may have it without a « qui peut écrire à qui » link
    room = b.post("/v1/rooms", headers=AUTH, json={"participants": ["vie", "famille@alex"], "title": "Vacances"})
    assert room.status_code == 200, room.text

    # Alex unlinks Marie: she keeps what is the family's and may take it over
    assert a.delete("/v1/family/marie", headers=AUTH).status_code == 204
    assert wait(lambda: b.get("/v1/family-agent", headers=AUTH).json()["handover"] is not None)
    after = b.get("/v1/family-agent", headers=AUTH).json()
    assert after["agent"] is None and after["handover"]["from"] == "Alex" and after["handover"]["name"] == "Famille"
    kept = json.loads((b.state_dir / "family-agent.json").read_text())["handover"]
    assert [i["text"] for i in kept["notebook"]["lists"][0]["items"]] == ["Lait", "Pain"]
    assert "Famille" in kept["soul"] or kept["soul"] == ""


def test_creating_the_familys_agent(bridges, fakes):
    """Made from the app on an install that manages its Hermes, isolated or not: its disk and browser tools off from
    the start, its personality with the family's block, the Carnet's three lists; several, each for the whole family
    or some members; deleted, it can be made again."""
    from test_agents_admin import hermes_args, managed

    bridge, home = managed(bridges, fakes, isolated=True)
    assert bridge.get("/v1/family-agent", headers=AUTH).json() == {"agent": None, "agents": [], "can_create": True,
                                                                   "handover": None, "members": []}
    made = bridge.post("/v1/family-agent", headers=AUTH, json={"name": "Famille", "lang": "fr"})
    assert made.status_code == 201, made.text
    assert made.json()["id"] == "famille" and made.json()["mine"] is True
    assert ["-p", "famille", "tools", "disable", "terminal", "file", "code_execution", "browser", "--platform",
            "api_server"] in hermes_args(bridge)
    soul = (home / "profiles" / "famille" / "SOUL.md").read_text()
    assert "<!-- bipagents:family-agent -->" in soul and "not even those of the member who created you" in soul
    assert "bipagents:notebook" not in soul and "bipagents:payments" not in soul
    lists = bridge.get("/v1/family-agent/notebook", headers=AUTH).json()["lists"]
    assert [(x["title"], x["kind"]) for x in lists] == [("Courses", "list"), ("Tâches", "tasks"), ("Dates", "dates")]
    # another one (12/11: one per topic), for some members only (none linked here: a list of no one)
    again = bridge.post("/v1/family-agent", headers=AUTH, json={"name": "Maison", "members": []})
    assert again.status_code == 201, again.text
    assert again.json()["id"] == "maison" and again.json()["everyone"] is False
    assert bridge.post("/v1/family-agent", headers=AUTH, json={"name": "X", "members": ["ghost"]}).status_code == 422
    views = bridge.get("/v1/family-agent", headers=AUTH).json()
    assert [x["id"] for x in views["agents"]] == ["famille", "maison"] and views["agent"]["id"] == "famille"
    # a common conversation from their start: never cleared at a restart (family_thread.rs `migrate`)
    assert json.loads((bridge.state_dir / "family-agent.json").read_text())["common"].keys() == {"famille", "maison"}
    refused = bridge.patch("/v1/agents/famille/settings", headers=AUTH, json={"toolsets": {"terminal": True}})
    assert refused.status_code == 400 and "private" in refused.json()["detail"]
    assert bridge.patch("/v1/agents/famille/settings", headers=AUTH, json={"health": True}).status_code == 400
    assert bridge.delete("/v1/agents/famille", headers=AUTH).status_code in (200, 204)
    assert bridge.get("/v1/family-agent", headers=AUTH).json()["can_create"] is True
