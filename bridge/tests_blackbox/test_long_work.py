"""Long work handed to background tasks (decided 2026-10-22): `start_task` within the direct budget (`[jobs]
direct_budget_eur`, ≈ 0.20 € at the `[notebook]` prices), refused above it; `notebook_fill`; « Remplir mon Carnet »
for every agent in turn (`/v1/notebook/fill`); and the bridge's safety net, which steers a long conversation run once
to hand the rest over (`[jobs] handoff…`): never while a sub-agent works or the agent browses (2026-10-24), and a
steered run that stops without handing over gets its task from the bridge (started, or proposed), said in the run."""
import threading
import time

import pytest

from bbkit.common import AUTH, read_sse, sse
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


# -- 2026-10-24: never during a sub-agent's work or browsing, and never ending silently ---------------------------------

TOKEN = "f" * 64
NOW = "2026-10-24T08:00:00.000Z"
ASKED = "Cherche-moi une veste en laine bleue, taille M, moins de 150 €"


def with_device(bridges, jobs_config):
    bridge = bridges.new()
    bridge.config["jobs"] = jobs_config
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (TOKEN, "sandbox", "[]", NOW, NOW))])
    return bridge.start()


def follow(bridge, run_id, session="api_c"):
    """The run as the app follows it (through the bridge), until it ends."""
    with bridge.stream("GET", f"/v1/runs/{run_id}/events?agent=wellness&session_id={session}", headers=AUTH) as resp:
        assert resp.status_code == 200
        return read_sse(resp)


@pytest.mark.slow
def test_no_steer_while_a_sub_agent_works_for_the_run(bridges, fakes):
    bridge = bridges.new()
    bridge.config["jobs"] = {"handoff_tool_calls": 3, "handoff_seconds": 5}
    bridge.start()
    gate = threading.Event()
    fakes.hermes.runs["run_d"] = (tools("run_d", 2) + tools("run_d", 1, "delegate_task")
                                  + [sse("subagent.start", run_id="run_d", goal="Chercher des vestes")]
                                  + tools("run_d", 3) + [gate, 6]
                                  + [sse("subagent.complete", run_id="run_d", status="completed"),
                                     sse("tool.completed", run_id="run_d", tool="delegate_task", error=False),
                                     sse("message.delta", run_id="run_d", delta="Voilà trois vestes."),
                                     sse("run.completed", run_id="run_d", output="Voilà trois vestes.")])
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_d"})
    time.sleep(1.0)
    assert fakes.hermes.steers == []
    gate.set()   # the sub-agent works on for 6 s (past handoff_seconds), then answers
    time.sleep(7.5)
    assert fakes.hermes.steers == []
    assert jobs(bridge) == []


def test_a_browsing_run_is_left_alone_unless_its_own_thresholds_say(bridges, fakes):
    bridge = bridges.new()
    bridge.config["jobs"] = {"handoff_tool_calls": 3}
    bridge.start()
    fakes.hermes.runs["run_b"] = (tools("run_b", 1) + tools("run_b", 10, "mcp_stealth_navigate")
                                  + [sse("run.completed", run_id="run_b", output="ok")])
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_b"})
    time.sleep(1.5)
    assert fakes.hermes.steers == []
    other = bridges.new()
    other.config["jobs"] = {"handoff_tool_calls": 3, "handoff_browser_tool_calls": 6}
    other.start()
    fakes.hermes.runs["run_b2"] = tools("run_b2", 10, "mcp_stealth_click_element")
    other.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_b2"})
    assert wait_until(lambda: fakes.hermes.steers)
    time.sleep(0.5)
    assert [s["run_id"] for s in fakes.hermes.steers] == ["run_b2"]
    assert other.logged("long run: handing over to a background task (steer)")[0]["tool_calls"] == 6


def test_a_steered_run_that_stops_without_handing_over_gets_its_task_started(bridges, fakes):
    bridge = with_device(bridges, {"handoff_tool_calls": 3})
    fakes.hermes.rounds = 1000
    fakes.hermes.add_session("api_c", [{"role": "user", "content": ASKED}])
    fakes.hermes.runs["run_r"] = (tools("run_r", 4)
                                  + [sse("message.delta", run_id="run_r", delta="Je continue en arrière-plan."),
                                     sse("run.completed", run_id="run_r", output="Je continue en arrière-plan.")])
    events = follow(bridge, "run_r")
    assert wait_until(lambda: len(fakes.hermes.steers) == 1)
    assert [name for name, _ in events][-2:] == ["bridge.notice", "run.completed"]
    notice = events[-2][1]
    assert notice["code"] == "handoff_started"
    assert notice["title"] == "Cherche-moi une veste en laine bleue, taille M, moins de 150…"
    job = jobs(bridge)[0]
    assert (job["id"], job["state"], job["proposed_by"], job["max_tokens"]) == (
        notice["job_id"], "running", "asked", DEFAULT_TOKENS)
    assert ASKED in job["prompt"] and "- web_search: x" in job["prompt"]
    assert "Je continue en arrière-plan." in job["prompt"]
    assert ASKED in fakes.hermes.started["r1"]["input"]
    assert wait_until(lambda: fakes.apns.pushes())
    payload = fakes.apns.payloads()[0]
    assert payload["aps"]["alert"]["body"] == "Continue en tâche de fond" and payload["job_id"] == job["id"]
    assert payload["session_id"] == "api_c"
    assert bridge.logged("long run: the bridge started the background task")
    # replayed to an app that comes back
    assert [name for name, _ in follow(bridge, "run_r")][-2:] == ["bridge.notice", "run.completed"]
    assert bridge.delete(f"/v1/jobs/{job['id']}", headers=AUTH).status_code == 200


def test_a_refused_start_task_is_proposed_and_said(bridges, fakes):
    bridge = with_device(bridges, {"handoff_tool_calls": 3})
    fakes.hermes.rounds = 1000
    fakes.hermes.add_session("api_c", [{"role": "user", "content": ASKED}])
    busy = bridge.post("/v1/agents/wellness/jobs", headers=AUTH,
                       json={"title": "Tri des mails", "prompt": "Trie.", "max_tokens": 1_000_000}).json()
    refused = "Not started: this agent is already on a background task. Propose it with propose_task instead."
    fakes.hermes.runs["run_f"] = (tools("run_f", 3)
                                  + [sse("tool.started", run_id="run_f", tool="mcp_bipagents_start_task", preview="x"),
                                     sse("tool.completed", run_id="run_f", tool="mcp_bipagents_start_task", error=True,
                                         preview=refused),
                                     sse("run.completed", run_id="run_f", output="")])
    events = follow(bridge, "run_f")
    notice = dict(events)["bridge.notice"]
    assert notice["code"] == "handoff_proposed" and "already on a background task" in notice["reason"]
    assert "Démarrer" in notice["text"]
    proposed = [j for j in jobs(bridge) if j["state"] == "proposed"]
    assert len(proposed) == 1 and proposed[0]["id"] == notice["job_id"] and ASKED in proposed[0]["prompt"]
    assert bridge.logged("long run: hand-over refused")
    assert bridge.delete(f"/v1/jobs/{busy['id']}", headers=AUTH).status_code == 200


def test_a_steered_run_that_answered_after_all_is_not_rescued(bridges, fakes):
    bridge = bridges.new()
    bridge.config["jobs"] = {"handoff_tool_calls": 3}
    bridge.start()
    answer = "Voici trois vestes en laine bleue. " * 30
    fakes.hermes.runs["run_a"] = tools("run_a", 4) + [sse("message.delta", run_id="run_a", delta=answer),
                                                      sse("run.completed", run_id="run_a", output=answer)]
    events = follow(bridge, "run_a")
    assert wait_until(lambda: len(fakes.hermes.steers) == 1) and "bridge.notice" not in [n for n, _ in events]
    assert jobs(bridge) == []


# -- 2026-10-25: the task's result handed back to the conversation that asked ------------------------------------------

REPORT = "J'ai comparé 12 vestes.\n\n- **Uniqlo** : 79 €, laine mérinos\n- Arket : 129 €\n- COS : 145 €"
MESSAGE = "C'est fait ! Trois vestes retenues :\n- Uniqlo, 79 €\n- Arket, 129 €"


@pytest.mark.slow
def test_a_finished_task_is_handed_back_to_the_conversation_that_asked(bridges, fakes):
    """start_task called from a conversation (the run the app follows there): when the task ends, the agent takes one
    short turn in that conversation, fed with the report as a note the apps hide, and writes the message itself; the
    full report is kept as one of its pages, linked; the message is in the Boîte, tagged with the conversation, and
    pushed. No « Tâche de fond terminée » towards the task's own conversation."""
    bridge = with_device(bridges, {})
    give_token(bridge)
    hermes = fakes.hermes
    hermes.rounds = 1
    hermes.add_session("api_c", [{"role": "user", "content": ASKED}], title="Veste")
    gate = threading.Event()
    hermes.runs["run_c"] = [sse("tool.started", run_id="run_c", tool="mcp_bipagents_start_task", preview="x"), gate,
                            sse("run.completed", run_id="run_c", output="Je m'en occupe en arrière-plan.")]
    watched = bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_c", "session_id": "api_c"})
    assert watched.status_code == 200
    assert wait_until(lambda: hermes.seen("/v1/runs/run_c/events"))
    time.sleep(0.3)
    result = call(bridge, "start_task", {"title": "Veste en laine", "instructions": ASKED})
    assert not result["isError"], text_of(result)
    gate.set()
    job = jobs(bridge)[0]
    assert job["from_session"] == "api_c" and job["session_id"] == "api_s1"
    hermes.run_outputs = {"r1": REPORT, "r2": MESSAGE}
    # the task (r1, its own conversation), then the agent's turn where the person asked (r2)
    assert wait_until(lambda: "r2" in hermes.started, timeout=40)
    turn = hermes.started["r2"]
    page = f"/bipagents/pages/wellness/rapport-{job['id']}.html"
    assert turn["session_id"] == "api_c" and turn["instructions"].startswith("Background work you started")
    assert turn["input"].startswith("<!--bip:task\nYour background work « Veste en laine » is finished.")
    assert "- Arket : 129 €" in turn["input"] and turn["input"].endswith(f"MEDIA:{page}\n-->")

    def handed():
        return [i for i in bridge.get("/v1/outbox", headers=AUTH).json()["items"] if i["session_id"] == "api_c"]

    assert wait_until(handed, timeout=20)
    item = handed()[0]
    assert item["text"] == MESSAGE and item["title"] == "Veste en laine" and item["agent"] == "wellness"
    # the full report, one of the agent's pages (served like any MEDIA file)
    shown = bridge.get("/v1/media", headers=AUTH, params={"path": page})
    assert shown.status_code == 200 and "<li><strong>Uniqlo</strong> : 79 €, laine mérinos</li>" in shown.text
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if p.get("outbox_id") == item["id"]])
    told = [p for p in fakes.apns.payloads() if p.get("outbox_id") == item["id"]][0]
    assert told["session_id"] == "api_c"
    assert not [p for p in fakes.apns.payloads() if p.get("job_id") == job["id"]]
    done = jobs(bridge)[0]
    assert done["state"] == "done" and done["tokens"]["input"] == 100_000 + 105_000
    assert bridge.logged("handback delivered")[0]["by_agent"] is True


@pytest.mark.slow
def test_a_task_whose_conversation_is_gone_is_told_by_the_template(bridges, fakes):
    """The agent's turn refused (the conversation deleted meanwhile): the templated message goes to the Boîte, opening
    the task's own conversation (where the full report is), with its key points."""
    bridge = with_device(bridges, {})
    give_token(bridge)
    hermes = fakes.hermes
    hermes.rounds = 1
    hermes.run_outputs = {"r1": REPORT}
    gate = threading.Event()
    hermes.runs["run_g"] = [sse("tool.started", run_id="run_g", tool="mcp_bipagents_start_task", preview="x"), gate,
                            sse("run.completed", run_id="run_g", output="ok")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_g", "session_id": "api_gone"})
    assert wait_until(lambda: hermes.seen("/v1/runs/run_g/events"))
    time.sleep(0.3)
    assert not call(bridge, "start_task", {"title": "Veste", "instructions": ASKED})["isError"]

    def refuse(seen):
        from starlette.responses import JSONResponse
        body = seen.json() if seen.method == "POST" and seen.path == "/v1/runs" else None
        if body and body.get("session_id") == "api_gone":
            return JSONResponse({"error": "Session not found"}, status_code=404)
        return None

    hermes.override = refuse
    gate.set()

    def told():
        return bridge.get("/v1/outbox", headers=AUTH).json()["items"]

    assert wait_until(told, timeout=40)
    item = told()[0]
    assert item["session_id"] == "api_s1"
    # its words in the agent's language (English here), its key points from the report
    assert item["text"].startswith("Done: “Veste”.") or item["text"].startswith("C'est fait : « Veste ».")
    assert "\n- Uniqlo : 79 €, laine mérinos\n- Arket : 129 €\n- COS : 145 €" in item["text"]
    assert bridge.logged("handback delivered")[0]["by_agent"] is False
