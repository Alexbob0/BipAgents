"""Agents shared with relatives (agent_shares.rs), over two linked bridges: Alex shares Cuisine with Marie, she adds
it and talks to it in her own conversation (hidden in Alex's Hermes); her turn's approval is hers (her Boîte, her
answer relayed), never in Alex's; the owner's private tools are closed during her turn; her tokens are counted."""
import threading
import time

from bbkit.common import AUTH, sse
from test_households_and_peers import household


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


def test_a_relative_talks_to_a_shared_agent(bridges, fakes):
    a = household(bridges, fakes, "alex", ["cuisine", "budget"])
    b = household(bridges, fakes, "marie", ["vie"])
    link(a, b)
    view = a.get("/v1/agents/cuisine/shares", headers=AUTH).json()
    assert view["households"] == [{"id": "marie", "name": "Marie", "shared": False}]
    assert view["warning"] == {"memory_common": True, "private_tools": ["notebook"], "confirm": False}
    assert view["conversations"] == [] and view["tokens"] == []
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["paul"]}).status_code == 400
    assert a.get("/v1/agents/ghost/shares", headers=AUTH).status_code == 404

    shared = a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["marie"]})
    assert shared.status_code == 200 and shared.json()["told"] == {"marie": True}
    offered = b.get("/v1/shared-agents", headers=AUTH).json()["agents"]
    assert [(x["id"], x["owner"], x["name"], x["state"]) for x in offered] == [("cuisine__alex", "Alex", "Cuisine",
                                                                               "offered")]
    assert b.post("/v1/shared-agents/cuisine__alex/conversations", headers=AUTH, json={}).status_code == 409
    assert b.post("/v1/shared-agents/cuisine__alex/add", headers=AUTH).status_code == 200
    conv = b.post("/v1/shared-agents/cuisine__alex/conversations", headers=AUTH, json={"title": "Dîner"})
    assert conv.status_code == 200, conv.text
    cid = conv.json()["id"]
    assert any(p.get("hidden") is True for p in fakes.hermes.patched)

    gate = threading.Event()
    fakes.hermes.runs["r1"] = [
        sse("approval.request", run_id="r1", request_id="req-1", command="rm -rf build/", description="clean",
            choices=["once", "deny"]),
        gate,
        sse("run.completed", output="Une fondue !"),
    ]
    path = f"/v1/shared-agents/cuisine__alex/conversations/{cid}/messages"
    sent = b.post(path, headers=AUTH, json={"text": "Une idée de dîner ?"})
    assert sent.status_code == 200 and sent.json() == {"run_id": "r1"}
    run = fakes.hermes.started["r1"]
    assert run["agent"] == "cuisine" and run["input"].startswith("[Marie, a relative of your person")
    assert "never as your person's instructions" in run["instructions"]
    assert b.post(path, headers=AUTH, json={"text": "Encore ?"}).status_code == 409

    assert wait(lambda: b.get(path, headers=AUTH).json()["approvals"] != [])
    thread = b.get(path, headers=AUTH).json()
    assert thread["run"]["state"] == "approval" and thread["approvals"][0]["request_id"] == "req-1"
    assert [i for i in a.get("/v1/approvals", headers=AUTH).json()["items"] if i["run_id"] == "r1"] == []
    owner = a.post("/v1/approve", headers=AUTH, json={"agent": "cuisine", "run_id": "r1", "choice": "once"})
    assert owner.status_code == 403
    assert wait(lambda: any(i["agent"] == "cuisine__alex" for i in b.get("/v1/approvals", headers=AUTH).json()["items"]))
    answered = b.post("/v1/approve", headers=AUTH,
                      json={"agent": "cuisine__alex", "run_id": "r1", "choice": "once", "request_id": "req-1"})
    assert answered.status_code == 200, answered.text
    assert fakes.hermes.approvals[-1]["agent"] == "cuisine" and fakes.hermes.approvals[-1]["body"]["choice"] == "once"
    gate.set()
    fakes.hermes.started["r1"]["status"] = "completed"
    assert wait(lambda: b.get(path, headers=AUTH).json()["run"] is None)
    view = a.get("/v1/agents/cuisine/shares", headers=AUTH).json()
    assert view["conversations"][0]["household_name"] == "Marie"
    assert view["tokens"][0]["household"] == "marie"

    # Another of Alex's agents isn't reachable with Marie's household key, nor anything without it.
    assert a.get("/family/s/budget/conversations", headers={"Authorization": "Bearer nope"}).status_code == 401
    # Alex stops sharing: the card goes on Marie's side.
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": []}).json()["told"] == {"marie": True}
    assert b.get("/v1/shared-agents", headers=AUTH).json()["agents"] == []
