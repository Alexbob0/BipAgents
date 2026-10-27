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
    assert {"family_notebook_read", "family_notebook_write", "weather"} <= names
    assert not names & {"mail_search", "create_page", "propose_task", "notebook_read", "upcoming_dates"}
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

    # Marie talks to it in her own conversation, as a family member
    conv = b.post("/v1/shared-agents/famille__alex/conversations", headers=AUTH, json={"title": "Menus"})
    assert conv.status_code == 200, conv.text
    cid = conv.json()["id"]
    fakes.hermes.runs["r1"] = [sse("run.completed", output="Des crêpes !")]
    path = f"/v1/shared-agents/famille__alex/conversations/{cid}/messages"
    sent = b.post(path, headers=AUTH, json={"text": "Une idée pour dimanche ?"})
    assert sent.status_code == 200, sent.text
    run = fakes.hermes.started["r1"]
    assert run["agent"] == "famille" and "You are the family's agent" in run["instructions"]
    fakes.hermes.started["r1"]["status"] = "completed"
    assert wait(lambda: b.get(path, headers=AUTH).json()["run"] is None)

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
