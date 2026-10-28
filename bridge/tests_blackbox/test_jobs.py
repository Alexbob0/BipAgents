"""Background jobs (`/v1/jobs…`): a long task in its own conversation with a budget, followed every 10 s (jobs.py's
POLL_SECONDS), paused at its budget or by the person, resumed, forgotten; an agent's proposal; a restart; mailbox
scans listed with them (tests/test_jobs.py, over HTTP; the fake Hermes counts tokens while a run goes on)."""
import json
import os
import re

import pytest

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until

TOKEN = "f" * 64
TASK = {"title": "Écoles du quartier", "prompt": "Compare les trois écoles près de chez nous.", "max_tokens": 1_000_000,
        "template": "research"}
NOW = "2026-10-21T08:00:00.000Z"


def with_device(bridges):
    bridge = bridges.new()
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (TOKEN, "sandbox", "[]", NOW, NOW))])
    return bridge.start()


def view(bridge, job_id):
    return next(j for j in bridge.get("/v1/jobs", headers=AUTH).json()["jobs"] if j["id"] == job_id)


@pytest.mark.slow
def test_a_task_runs_in_its_own_conversation_and_ends_with_its_report(bridges, fakes):
    bridge = with_device(bridges)
    fakes.hermes.rounds = 1
    started = bridge.post("/v1/agents/wellness/jobs", headers=AUTH, json=TASK)
    assert started.status_code == 200, started.text
    job = started.json()
    assert re.fullmatch(r"t[0-9a-f]{12}", job["id"]) and job["state"] == "running" and job["session_id"] == "api_s1"
    assert set(job) == {"id", "kind", "agent", "title", "prompt", "template", "state", "reason", "error", "created",
                        "started", "finished", "tokens", "max_tokens", "session_id", "report", "proposed_by", "runs",
                        "from_session"}
    assert (job["kind"], job["proposed_by"], job["runs"], job["template"]) == ("task", "person", 1, "research")
    assert fakes.hermes.created == [("", {"title": "🧩 Écoles du quartier"})]
    run = fakes.hermes.seen("/v1/runs", "POST")[0].json()
    assert run["session_id"] == "api_s1" and run["input"] == "Écoles du quartier\n\nCompare les trois écoles près de chez nous."
    assert "model_options" not in run   # the agent's own thinking level (GLM through EUrouter: « medium », agents_admin)
    assert run["instructions"].startswith("This is a background task the person gave you from the app (« Tâches de fond »)")
    stored = bridge.state_dir / "jobs" / f"{job['id']}.json"
    assert oct(os.stat(stored).st_mode & 0o777) == "0o600" and json.loads(stored.read_text())["run_id"] == "r1"
    twice = bridge.post("/v1/agents/wellness/jobs", headers=AUTH, json=TASK)
    assert twice.status_code == 409 and twice.json() == {"detail": "this agent is already on a background task"}
    assert wait_until(lambda: view(bridge, job["id"])["state"] == "done", timeout=25)
    done = view(bridge, job["id"])
    assert done["report"].startswith("Rapport") and done["finished"]
    # The run's own usage at the end, not the counters' guess.
    assert done["tokens"] == {"input": 100_000, "output": 5_000, "cached": 0}
    assert wait_until(lambda: fakes.apns.pushes())
    payload = fakes.apns.payloads()[0]
    assert payload["aps"]["alert"]["body"] == "Tâche de fond terminée" and payload["job_id"] == job["id"]
    assert payload["session_id"] == "api_s1" and payload["run_id"] == "session:api_s1"
    assert fakes.apns.pushes()[0].header("apns-collapse-id") == f"job:{job['id']}"
    listed = bridge.get("/v1/jobs", headers=AUTH).json()
    assert listed["budgets"] == {"small": 300_000, "medium": 1_000_000, "large": 3_000_000}
    assert "base" not in listed["jobs"][0] and "run_id" not in listed["jobs"][0]


@pytest.mark.slow
def test_a_task_asked_again_gets_a_numbered_conversation_when_its_title_is_taken(bridges, fakes):
    bridge = with_device(bridges)
    fakes.hermes.unique_titles = True
    fakes.hermes.add_session("api_old", [], title="🧩 Écoles du quartier")   # the same task, cancelled before
    started = bridge.post("/v1/agents/wellness/jobs", headers=AUTH, json=TASK)
    assert started.status_code == 200, started.text
    assert fakes.hermes.created == [("", {"title": "🧩 Écoles du quartier (2)"})]


def test_a_task_stops_at_its_budget_and_goes_on_with_a_bigger_one(bridges, fakes):
    bridge = with_device(bridges)
    fakes.hermes.rounds, fakes.hermes.step = 50, 60_000
    job = bridge.post("/v1/agents/wellness/jobs", headers=AUTH, json={**TASK, "max_tokens": 100_000}).json()
    assert wait_until(lambda: view(bridge, job["id"])["state"] == "paused", timeout=45)
    paused = view(bridge, job["id"])
    assert paused["reason"] == "budget" and fakes.hermes.seen("/v1/runs/r1/stop", "POST")
    assert paused["tokens"]["input"] >= 100_000 * 20 // 21   # counted from the conversation's counters
    assert wait_until(lambda: fakes.apns.pushes())
    assert fakes.apns.payloads()[-1]["aps"]["alert"]["body"] == "Tâche de fond en pause"
    # Resuming without more budget is refused; with more, the same conversation goes on.
    refused = bridge.post(f"/v1/jobs/{job['id']}/resume", headers=AUTH)
    assert refused.status_code == 409 and refused.json() == {"detail": "the budget is used up: give a bigger one"}
    resumed = bridge.post(f"/v1/jobs/{job['id']}/resume", headers=AUTH,
                          json={"max_tokens": 5_000_000, "message": "Continue, et termine par ton rapport."}).json()
    assert resumed["state"] == "running" and resumed["runs"] == 2 and resumed["max_tokens"] == 5_000_000
    assert fakes.hermes.started["r2"]["input"] == "Continue, et termine par ton rapport."
    assert len(fakes.hermes.created) == 1                      # the same conversation
    gone = bridge.delete(f"/v1/jobs/{job['id']}", headers=AUTH)
    assert gone.json() == {"ok": True} and fakes.hermes.started["r2"]["status"] == "cancelled"
    assert bridge.get("/v1/jobs", headers=AUTH).json()["jobs"] == []


@pytest.mark.slow
def test_the_person_pauses_resumes_and_forgets_a_task(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds, fakes.hermes.step = 1000, 10
    job = bridge.post("/v1/agents/wellness/jobs", headers=AUTH, json={**TASK, "title": "Long", "prompt": "Fais-le."}).json()
    stopping = bridge.post(f"/v1/jobs/{job['id']}/pause", headers=AUTH)
    assert stopping.json()["state"] == "stopping" and fakes.hermes.started["r1"]["status"] == "cancelled"
    assert wait_until(lambda: view(bridge, job["id"])["state"] == "paused", timeout=25)
    assert view(bridge, job["id"])["reason"] == "stopped"
    assert bridge.post(f"/v1/jobs/{job['id']}/resume", headers=AUTH).json()["state"] == "running"
    assert fakes.hermes.started["r2"]["input"] == "Continue the task where you stopped (don't start again), then end with your report."
    assert bridge.delete(f"/v1/jobs/{job['id']}", headers=AUTH).json() == {"ok": True}
    assert not (bridge.state_dir / "jobs" / f"{job['id']}.json").exists()
    assert fakes.hermes.started["r2"]["status"] == "cancelled"
    assert fakes.apns.pushes() == []


def test_an_agent_proposes_a_task_the_person_starts(bridges, fakes):
    bridge = with_device(bridges)
    give_token(bridge)
    result = call(bridge, "propose_task", {"title": "Dossier location", "prompt": "Prépare le dossier.", "budget": "small"})
    assert not result["isError"] and "Tâches de fond" in text_of(result)
    assert text_of(result).startswith("Proposed (budget about 300,000 tokens).")
    proposed = bridge.get("/v1/jobs", headers=AUTH).json()["jobs"][0]
    assert proposed["state"] == "proposed" and proposed["proposed_by"] == "agent" and proposed["max_tokens"] == 300_000
    assert not fakes.hermes.seen("/v1/runs")                  # nothing spent before the go
    assert wait_until(lambda: fakes.apns.pushes())
    payload = fakes.apns.payloads()[0]
    assert payload["aps"]["alert"]["body"] == "Tâche de fond proposée" and "session_id" not in payload
    assert payload["run_id"] == "session:job" and payload["job_id"] == proposed["id"]
    started = bridge.post(f"/v1/jobs/{proposed['id']}/start", headers=AUTH, json={"max_tokens": 500_000})
    assert started.json()["state"] == "running" and started.json()["max_tokens"] == 500_000
    again = bridge.post(f"/v1/jobs/{proposed['id']}/start", headers=AUTH)
    assert again.status_code == 409 and again.json() == {"detail": "this task isn't waiting to start"}
    for _ in range(5):
        call(bridge, "propose_task", {"title": "Encore", "prompt": "x"})
    too_many = call(bridge, "propose_task", {"title": "Encore", "prompt": "x"})
    assert too_many["isError"] and text_of(too_many) == "enough of your proposals are waiting for the person already"


@pytest.mark.slow
def test_after_a_restart_a_finished_run_is_read_from_its_conversation(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    job = bridge.post("/v1/agents/wellness/jobs", headers=AUTH, json={**TASK, "title": "Recherche", "prompt": "Cherche."}).json()
    bridge.stop()
    fakes.hermes.forget_runs = True             # Hermes restarted meanwhile: the run is gone, its turns stay
    fakes.hermes.messages[("", job["session_id"])] = [{"role": "user", "content": "Recherche\n\nCherche."},
                                                      {"role": "assistant", "content": "Voici le rapport."}]
    bridge.start()
    assert wait_until(lambda: view(bridge, job["id"])["state"] == "done", timeout=25)
    assert view(bridge, job["id"])["report"] == "Voici le rapport."


def test_mail_scans_are_listed_and_controlled_with_the_tasks(bridges):
    bridge = bridges.new()
    folder = bridge.data / "mail-scans"
    folder.mkdir(parents=True)
    (folder / "wellness.json").write_text(json.dumps({
        "agent": "wellness", "account": "a1", "label": "Gmail", "state": "paused", "error": "budget", "created": 1.0,
        "ranges": [], "tokens": {"input": 9000, "output": 900, "cached": 0}, "processed": 30, "pages": 0,
        "step": {"since": "2026-07-21", "until": "2026-10-21", "max_tokens": 10_000, "mails": 80, "processed": 30}}))
    bridge.start()
    listed = bridge.get("/v1/jobs", headers=AUTH).json()["jobs"]
    assert listed == [{"id": "mail-scan:wellness", "kind": "mail_scan", "agent": "wellness", "title": "Gmail",
                       "state": "paused", "reason": "budget", "error": None, "created": 1.0, "finished": None,
                       "tokens": {"input": 9000, "output": 900, "cached": 0}, "max_tokens": 10_000,
                       "progress": {"done": 30, "total": 80}, "processed": 30, "pages": 0}]
    assert bridge.post("/v1/jobs/mail-scan:wellness/pause", headers=AUTH).json()["state"] == "paused"
    assert bridge.delete("/v1/jobs/mail-scan:wellness", headers=AUTH).json() == {"ok": True}
    assert not (folder / "wellness.json").exists() and bridge.get("/v1/jobs", headers=AUTH).json()["jobs"] == []
    assert bridge.post("/v1/jobs/mail-scan:wellness/pause", headers=AUTH).status_code == 404


def test_the_routes_need_the_bridge_key_and_check_what_they_get(bridge, fakes):
    assert bridge.get("/v1/jobs").status_code == 401
    answer = bridge.get("/v1/jobs", headers=AUTH)
    assert answer.status_code == 200 and answer.json() == {"jobs": [], "budgets": {"small": 300_000, "medium": 1_000_000,
                                                                                   "large": 3_000_000}}
    missing = bridge.post("/v1/jobs/t000000000000/pause", headers=AUTH)
    assert missing.status_code == 404 and missing.json() == {"detail": "unknown job"}
    nothing = bridge.post("/v1/jobs/nonsense/resume", headers=AUTH)
    assert nothing.status_code == 409 and nothing.json() == {"detail": "nothing to resume"}
    assert bridge.delete("/v1/jobs/t000000000000", headers=AUTH).status_code == 404
    assert bridge.post("/v1/agents/ghost/jobs", headers=AUTH, json=TASK).status_code == 404
    assert bridge.post("/v1/agents/wellness/jobs", headers=AUTH, json={**TASK, "max_tokens": 10}).status_code == 422
    fakes.hermes.keys[""] = "other"
    refused = bridge.post("/v1/agents/wellness/jobs", headers=AUTH, json=TASK)
    assert refused.status_code == 409 and refused.json() == {"detail": "the agent refused the task (401)"}
    assert bridge.get("/v1/jobs", headers=AUTH).json()["jobs"] == []


@pytest.mark.slow
def test_a_running_task_says_what_it_does_now_and_jobs_list_by_agent(bridges, fakes):
    from bbkit.common import sse

    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    fakes.hermes.runs["r1"] = [sse("tool.started", tool="web_search", preview="mutuelle famille 4")]
    job = bridge.post("/v1/agents/wellness/jobs", headers=AUTH, json=TASK).json()

    def activity():
        jobs = bridge.get("/v1/jobs?agent=wellness", headers=AUTH).json()["jobs"]
        return jobs[0].get("activity") if jobs else None

    assert wait_until(lambda: activity() == {"tool": "web_search", "preview": "mutuelle famille 4"}, timeout=15)
    missing = bridge.get("/v1/jobs?agent=nobody", headers=AUTH)
    assert missing.status_code == 404 and missing.json() == {"detail": "unknown agent"}
    assert bridge.delete(f"/v1/jobs/{job['id']}", headers=AUTH).status_code == 200
