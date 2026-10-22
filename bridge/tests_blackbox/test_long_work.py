"""Long work handed to background tasks (decided 2026-10-22): `start_task` within the direct budget (`[jobs]
direct_budget_eur`, ≈ 0.20 € at the `[notebook]` prices), refused above it; `notebook_fill`; « Remplir mon Carnet »
for every agent in turn (`/v1/notebook/fill`); and the bridge's safety net, which steers a long conversation run once
to hand the rest over (`[jobs] handoff…`)."""
import time

import pytest

from bbkit.common import AUTH, sse
from bbkit.mcp import call, give_token, rpc, text_of
from bbkit.servers import wait_until

# 0.20 € at 0.95 × 0.50 € + 0.05 × 1.50 € per million tokens
DEFAULT_TOKENS = 363_636


def jobs(bridge):
    return bridge.get("/v1/jobs", headers=AUTH).json()["jobs"]


def test_start_task_starts_a_task_within_the_direct_budget(bridges, fakes):
    bridge = bridges.new().start()
    give_token(bridge)
    fakes.hermes.rounds = 1000
    names = [t["name"] for t in rpc(bridge, "tools/list").json()["result"]["tools"]]
    assert "start_task" in names and "notebook_fill" in names and "propose_task" in names
    result = call(bridge, "start_task", {"title": "Tri des mails", "instructions": "Trie mes mails de la semaine."})
    assert not result["isError"], text_of(result)
    assert text_of(result).startswith("Started in the background (budget about 0.20 €, 363,636 tokens)")
    job = jobs(bridge)[0]
    assert (job["state"], job["proposed_by"], job["max_tokens"]) == ("running", "asked", DEFAULT_TOKENS)
    assert job["title"] == "Tri des mails" and job["prompt"] == "Trie mes mails de la semaine."
    run = fakes.hermes.started["r1"]
    assert run["input"] == "Tri des mails\n\nTrie mes mails de la semaine."
    assert run["instructions"].startswith("This is a background task")
    # one task per agent: the next one is refused, with what to do instead
    again = call(bridge, "start_task", {"title": "Autre", "instructions": "Encore."})
    assert again["isError"] and text_of(again) == ("Not started: this agent is already on a background task. "
                                                  "Propose it with propose_task instead, and tell your person so.")
    assert bridge.delete(f"/v1/jobs/{job['id']}", headers=AUTH).status_code == 200


def test_start_task_refuses_more_than_the_direct_budget(bridges, fakes):
    bridge = bridges.new().start()
    give_token(bridge)
    fakes.hermes.rounds = 1000
    above = call(bridge, "start_task", {"title": "Grosse recherche", "instructions": "Cherche.", "budget_eur": 1.5})
    assert above["isError"] and "more than the 0.20 € a task may take without your person's go" in text_of(above)
    assert "propose_task" in text_of(above)
    assert not fakes.hermes.seen("/v1/runs", "POST") and jobs(bridge) == []
    smaller = call(bridge, "start_task", {"title": "Petite recherche", "instructions": "Cherche.", "budget_eur": 0.1})
    assert not smaller["isError"] and jobs(bridge)[0]["max_tokens"] == DEFAULT_TOKENS // 2
    assert bridge.delete(f"/v1/jobs/{jobs(bridge)[0]['id']}", headers=AUTH).status_code == 200


def test_the_direct_budget_is_configured(bridges, fakes):
    bridge = bridges.new()
    bridge.config["jobs"] = {"direct_budget_eur": 0.55}
    bridge.start()
    give_token(bridge)
    fakes.hermes.rounds = 1000
    result = call(bridge, "start_task", {"title": "Comparatif", "instructions": "Compare 10 appartements."})
    assert text_of(result).startswith("Started in the background (budget about 0.55 €, 1,000,000 tokens)")
    assert bridge.delete(f"/v1/jobs/{jobs(bridge)[0]['id']}", headers=AUTH).status_code == 200


def test_notebook_fill_starts_the_carnets_task(bridges, fakes):
    bridge = bridges.new().start()
    give_token(bridge)
    fakes.hermes.rounds = 1000
    result = call(bridge, "notebook_fill", {})
    assert not result["isError"] and text_of(result).startswith("Started in the background (budget about 0.20 €")
    job = jobs(bridge)[0]
    assert (job["title"], job["template"], job["proposed_by"]) == ("Fill the Notebook", "notebook_fill", "asked")
    assert job["prompt"].startswith("Fill your person's Carnet from what you already know about them")
    assert "session_search" in fakes.hermes.started["r1"]["input"]
    assert fakes.hermes.created == [("", {"title": "🧩 Fill the Notebook"})]
    assert bridge.delete(f"/v1/jobs/{job['id']}", headers=AUTH).status_code == 200


@pytest.mark.slow
def test_fill_my_carnet_goes_through_each_agent_in_turn(bridges, fakes):
    bridge = bridges.new()
    bridge.config["agents"]["Coach"] = {"display_name": "Coach", "hermes_url": f"{fakes.hermes.url}/p/coach",
                                        "hermes_key": "coach-key"}
    bridge.start()
    assert bridge.get("/v1/notebook/fill", headers=AUTH).json()["state"] == "idle"
    fakes.hermes.rounds = 1
    started = bridge.post("/v1/notebook/fill", headers=AUTH)
    assert started.status_code == 200, started.text
    view = started.json()
    assert (view["state"], view["total"], view["done"]) == ("running", 2, 0)
    assert [a["agent"] for a in view["agents"]] == ["wellness", "coach"]
    twice = bridge.post("/v1/notebook/fill", headers=AUTH)
    assert twice.status_code == 409 and twice.json() == {"detail": "the Carnet is being filled already"}

    def state():
        return bridge.get("/v1/notebook/fill", headers=AUTH).json()

    assert wait_until(lambda: state()["current"] == "wellness", timeout=10)
    assert wait_until(lambda: state()["state"] == "done", timeout=60)
    done = state()
    assert done["done"] == 2 and [a["state"] for a in done["agents"]] == ["done", "done"]
    # one after the other: the coach's task started once the first one ended
    assert [r["agent"] for r in fakes.hermes.started.values()] == ["", "coach"]
    assert {j["agent"]: j["template"] for j in jobs(bridge)} == {"wellness": "notebook_fill", "coach": "notebook_fill"}
    assert bridge.post("/v1/notebook/fill", headers=AUTH, json={"agents": ["ghost"]}).status_code == 404


def test_fill_my_carnet_stops(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    assert bridge.post("/v1/notebook/fill", headers=AUTH, json={"agents": ["wellness"]}).json()["total"] == 1
    stopped = bridge.delete("/v1/notebook/fill", headers=AUTH).json()
    assert stopped["state"] == "stopped"
    for job in jobs(bridge):
        bridge.delete(f"/v1/jobs/{job['id']}", headers=AUTH)


def tools(run_id, count, tool="web_search"):
    return [sse("tool.started", run_id=run_id, tool=tool, preview="x") for _ in range(count)]


def test_a_long_conversation_run_is_steered_once_to_hand_over(bridges, fakes):
    bridge = bridges.new()
    bridge.config["jobs"] = {"handoff_tool_calls": 3}
    bridge.start()
    fakes.hermes.runs["run_l"] = tools("run_l", 8) + [sse("run.completed", run_id="run_l", output="ok")]
    assert bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_l"}).status_code == 200
    assert wait_until(lambda: fakes.hermes.steers)
    time.sleep(0.5)
    assert len(fakes.hermes.steers) == 1
    steer = fakes.hermes.steers[0]
    assert steer["run_id"] == "run_l" and steer["body"]["input"].startswith("[bipagents:handoff] This is long:")
    assert "start_task" in steer["body"]["input"]
    logged = bridge.logged("long run: handing over to a background task (steer)")
    assert len(logged) == 1 and logged[0]["tool_calls"] == 3


def test_a_run_that_handed_over_or_a_short_one_is_left_alone(bridges, fakes):
    bridge = bridges.new()
    bridge.config["jobs"] = {"handoff_tool_calls": 3}
    bridge.start()
    fakes.hermes.runs["run_h"] = (tools("run_h", 1) + tools("run_h", 1, "mcp_bipagents_start_task") + tools("run_h", 5)
                                  + [sse("run.completed", run_id="run_h", output="ok")])
    fakes.hermes.runs["run_s"] = tools("run_s", 2) + [sse("run.completed", run_id="run_s", output="ok")]
    for run in ("run_h", "run_s"):
        bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": run})
    time.sleep(1.5)
    assert fakes.hermes.steers == []


def test_the_safety_net_can_be_turned_off(bridges, fakes):
    bridge = bridges.new()
    bridge.config["jobs"] = {"handoff": False, "handoff_tool_calls": 3}
    bridge.start()
    fakes.hermes.runs["run_o"] = tools("run_o", 8) + [sse("run.completed", run_id="run_o", output="ok")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_o"})
    time.sleep(1.5)
    assert fakes.hermes.steers == []


def test_a_background_tasks_own_run_is_never_steered(bridges, fakes):
    bridge = bridges.new()
    bridge.config["jobs"] = {"handoff_tool_calls": 3}
    bridge.start()
    fakes.hermes.rounds = 1000
    fakes.hermes.runs["r1"] = tools("r1", 8)
    job = bridge.post("/v1/agents/wellness/jobs", headers=AUTH,
                      json={"title": "Long", "prompt": "Fais-le.", "max_tokens": 1_000_000}).json()
    assert wait_until(lambda: len(fakes.hermes.seen("/v1/runs/r1/events")) >= 1)
    time.sleep(1.5)
    assert fakes.hermes.steers == []
    assert bridge.delete(f"/v1/jobs/{job['id']}", headers=AUTH).status_code == 200


@pytest.mark.slow
def test_a_run_going_on_for_minutes_is_steered_too(bridges, fakes):
    bridge = bridges.new()
    bridge.config["jobs"] = {"handoff_seconds": 5}
    bridge.start()
    fakes.hermes.runs["run_t"] = tools("run_t", 1) + [6] + tools("run_t", 1)
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_t"})
    time.sleep(2)
    assert fakes.hermes.steers == []
    assert wait_until(lambda: fakes.hermes.steers, timeout=15)
