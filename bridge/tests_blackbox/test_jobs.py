"""Background jobs (`/v1/jobs…`): a long task in its own conversation with a budget, followed every 10 s (jobs.py's
POLL_SECONDS), paused at its budget or by the person, resumed, forgotten; an agent's proposal; a restart; mailbox
scans listed with them (tests/test_jobs.py, over HTTP; the fake Hermes counts tokens while a run goes on). The app's
names in a task's instructions, in the tools' words and in the pushes are in the agent's language (names.rs,
2026-11-01): English for an agent with none set, like Wellness here; French on a French place (`french_place`)."""
import json
import os
import re

import pytest

from bbkit.common import AUTH, in_session
from bbkit.fakes import split_now
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until

TOKEN = "f" * 64
TASK = {"title": "Écoles du quartier", "prompt": "Compare les trois écoles près de chez nous.", "budget_eur": 0.50,
        "template": "research"}
NOW = "2026-10-21T08:00:00.000Z"



def words(text):
    """A task run's words without its rules' note (every run of a task carries it in its message, not as run
    instructions: background_turn.rs)."""
    head, rules = text.split("\n\n<!--bip:task-rules\n", 1)
    assert rules.endswith("\n-->")
    return head

def with_device(bridges):
    bridge = bridges.new()
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (TOKEN, "sandbox", "[]", NOW, NOW))])
    return bridge.start()


def french_place(bridge):
    """The place's language French (its Carnet's): an agent with no language of its own writes in it."""
    assert bridge.patch("/v1/notebook/settings", json={"language": "fr"}, headers=AUTH).status_code == 200


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
                        "started", "finished", "tokens", "budget_eur", "eur", "max_tokens", "session_id", "report",
                        "proposed_by", "runs", "from_session"}
    assert (job["kind"], job["proposed_by"], job["runs"], job["template"]) == ("task", "person", 1, "research")
    assert fakes.hermes.created == [("", {"title": "🧩 Écoles du quartier"})]
    run = fakes.hermes.seen("/v1/runs", "POST")[0].json()
    now, words = split_now(run["input"])
    assert run["session_id"] == "api_s1" and now
    assert words.startswith("Écoles du quartier\n\nCompare les trois écoles près de chez nous.\n\n<!--bip:task-rules\n")
    assert "model_options" not in run   # the agent's own thinking level (GLM through EUrouter: « medium », agents_admin)
    # the rules in the turn's message, never as run instructions: the system prompt (and its prompt cache) stays the
    # conversations' (background_turn.rs, cache audit #2)
    assert "instructions" not in run
    # the app's part by its English name: English agents repeated « Tâches de fond » to their person (names.rs)
    assert "This is a background task the person gave you from the app (« Background tasks »)" in words
    assert "Tâches de fond" not in words
    stored = bridge.state_dir / "jobs" / f"{job['id']}.json"
    assert oct(os.stat(stored).st_mode & 0o777) == "0o600" and json.loads(stored.read_text())["run_id"] == "r1"
    twice = bridge.post("/v1/agents/wellness/jobs", headers=AUTH, json=TASK)
    assert twice.status_code == 409 and twice.json() == {"detail": "this agent is already on a background task"}
    assert wait_until(lambda: view(bridge, job["id"])["state"] == "done", timeout=25)
    done = view(bridge, job["id"])
    assert done["report"].startswith("Rapport") and done["finished"]
    # The run's own usage at the end, not the counters' guess.
    assert done["tokens"] == {"input": 100_000, "output": 5_000, "cached": 0}
    assert done["eur"] == pytest.approx(0.0575)   # at the install's prices (0.50 / 1.50 € per million)
    assert wait_until(lambda: fakes.apns.pushes())
    payload = fakes.apns.payloads()[0]
    assert payload["aps"]["alert"]["body"] == "notif.task" and payload["r"] == job["id"]
    assert payload["t"] == "task" and in_session(bridge, payload, "api_s1")
    assert fakes.apns.pushes()[0].header("apns-collapse-id") == f"job:{job['id']}"
    listed = bridge.get("/v1/jobs", headers=AUTH).json()
    assert listed["budgets_eur"] == {"small": 0.20, "medium": 0.50, "large": 1.50}
    assert listed["budgets_tokens"] == {"small": 363_636, "medium": 909_090, "large": 2_727_272}
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
    french_place(bridge)   # the instructions and the push in French
    fakes.hermes.rounds, fakes.hermes.step = 50, 60_000
    job = bridge.post("/v1/agents/wellness/jobs", headers=AUTH, json={**TASK, "budget_eur": 0.05}).json()
    assert "This is a background task the person gave you from the app (« Tâches de fond »)" in \
        fakes.hermes.started["r1"]["input"]
    assert wait_until(lambda: view(bridge, job["id"])["state"] == "paused", timeout=45)
    paused = view(bridge, job["id"])
    assert paused["reason"] == "budget" and fakes.hermes.seen("/v1/runs/r1/stop", "POST")
    assert paused["tokens"]["input"] >= 100_000 * 20 // 21   # counted from the conversation's counters
    assert wait_until(lambda: fakes.apns.pushes())
    assert fakes.apns.payloads()[-1]["aps"]["alert"]["body"] == "notif.task"
    # Resuming without more budget is refused; with more, the same conversation goes on.
    refused = bridge.post(f"/v1/jobs/{job['id']}/resume", headers=AUTH)
    assert refused.status_code == 409 and refused.json() == {"detail": "the budget is used up: give a bigger one"}
    resumed = bridge.post(f"/v1/jobs/{job['id']}/resume", headers=AUTH,
                          json={"budget_eur": 3.0, "message": "Continue, et termine par ton rapport."}).json()
    assert resumed["state"] == "running" and resumed["runs"] == 2 and resumed["budget_eur"] == 3.0
    assert words(fakes.hermes.started["r2"]["input"]) == "Continue, et termine par ton rapport."
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
    assert words(fakes.hermes.started["r2"]["input"]) == \
        "Continue the task where you stopped (don't start again), then end with your report."
    assert bridge.delete(f"/v1/jobs/{job['id']}", headers=AUTH).json() == {"ok": True}
    assert not (bridge.state_dir / "jobs" / f"{job['id']}.json").exists()
    assert fakes.hermes.started["r2"]["status"] == "cancelled"
    assert fakes.apns.pushes() == []


def test_an_agent_proposes_a_task_the_person_starts(bridges, fakes):
    bridge = with_device(bridges)
    give_token(bridge)
    result = call(bridge, "propose_task", {"title": "Dossier location", "prompt": "Prépare le dossier.", "budget": "small"})
    assert not result["isError"] and "« Background tasks »" in text_of(result)   # the agent's language: English
    assert text_of(result).startswith("Proposed (budget about 363,636 tokens).")   # « small », 0.20 €, at home
    proposed = bridge.get("/v1/jobs", headers=AUTH).json()["jobs"][0]
    assert proposed["state"] == "proposed" and proposed["proposed_by"] == "agent" and proposed["budget_eur"] == 0.20
    assert not fakes.hermes.seen("/v1/runs")                  # nothing spent before the go
    assert wait_until(lambda: fakes.apns.pushes())
    payload = fakes.apns.payloads()[0]
    assert payload["aps"]["alert"]["body"] == "notif.task" and "c" not in payload
    assert payload["t"] == "task" and payload["r"] == proposed["id"]
    started = bridge.post(f"/v1/jobs/{proposed['id']}/start", headers=AUTH, json={"budget_eur": 0.30})
    assert started.json()["state"] == "running" and started.json()["budget_eur"] == 0.30
    again = bridge.post(f"/v1/jobs/{proposed['id']}/start", headers=AUTH)
    assert again.status_code == 409 and again.json() == {"detail": "this task isn't waiting to start"}
    # no task in it (5/11, a call meant for ask_secret ended on this name): nothing proposed, the right tools named
    jobs_before = len(bridge.get("/v1/jobs", headers=AUTH).json()["jobs"])
    junk = call(bridge, "propose_task", {"title": "x", "prompt": "No"})
    assert junk["isError"] and "ask_secret" in text_of(junk) and "sandbox_sites_request" in text_of(junk), junk
    assert len(bridge.get("/v1/jobs", headers=AUTH).json()["jobs"]) == jobs_before
    for _ in range(5):
        call(bridge, "propose_task", {"title": "Encore", "prompt": "Encore un dossier à préparer."})
    too_many = call(bridge, "propose_task", {"title": "Encore", "prompt": "Encore un dossier à préparer."})
    # (a turn no run carries: the brake's note that the same tool comes back often, after the answer)
    assert too_many["isError"] and text_of(too_many).startswith("enough of your proposals are waiting for the person already")


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
    assert answer.status_code == 200 and answer.json()["jobs"] == [] and answer.json()["budgets_eur"]["small"] == 0.20
    missing = bridge.post("/v1/jobs/t000000000000/pause", headers=AUTH)
    assert missing.status_code == 404 and missing.json() == {"detail": "unknown job"}
    nothing = bridge.post("/v1/jobs/nonsense/resume", headers=AUTH)
    assert nothing.status_code == 409 and nothing.json() == {"detail": "nothing to resume"}
    assert bridge.delete("/v1/jobs/t000000000000", headers=AUTH).status_code == 404
    assert bridge.post("/v1/agents/ghost/jobs", headers=AUTH, json=TASK).status_code == 404
    assert bridge.post("/v1/agents/wellness/jobs", headers=AUTH, json={**TASK, "budget_eur": 0.001}).status_code == 422
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


def test_a_task_counts_its_euros_as_the_month_s_credit_does(bridges, fakes):
    """Alex, 2026-11-04 (offer place): a task paused at « 5 % of the month », his credit still about 97 % left
    afterwards. Its budget counted every prompt token at the full input price; the router bills the cache reads (most
    of a task's prompt) at a fifth of it, and the month's credit is the router's count. Now: fresh input, cache reads
    and output at the model's prices (here `[jobs] price_*`, the guichet's list on an offer place, task_cost.rs), the
    hub stopping the run before a call past its euros; « Continuer (+0,20 €) » gives it more at once."""
    from bbkit.common import sse

    bridge = bridges.new()
    bridge.config["jobs"] = {"price_in": 0.10, "price_cached": 0.02, "price_out": 0.35}   # GLM-5.3 Flash at EUrouter
    bridge.start()
    fakes.hermes.rounds = 1000

    def usage(call, last, total_in, cached, out):
        return sse("model.usage", run_id="r1", call=call, input_tokens=last, output_tokens=300,
                   totals={"input_tokens": total_in, "output_tokens": out, "cache_read_tokens": cached})

    fakes.hermes.runs["r1"] = [usage(1, 95_000, 95_000, 90_000, 900),
                               sse("tool.started", run_id="r1", tool="read_file", preview="a.md"),
                               usage(2, 300_000, 400_000, 380_000, 3_000),
                               sse("tool.started", run_id="r1", tool="read_file", preview="b.md")]
    job = bridge.post("/v1/agents/wellness/jobs", headers=AUTH,
                      json={**TASK, "title": "Connecteur", "budget_eur": 0.015}).json()
    assert wait_until(lambda: view(bridge, job["id"])["state"] == "paused", timeout=45)
    paused = view(bridge, job["id"])
    # counted in full, its first call alone (95.9k tokens) was 0.048 € at the old prices, three times this budget;
    # billed, it is 0.0026 €: it went on to its second call, whose next one would pass 0.015 €
    assert paused["reason"] == "budget" and fakes.hermes.seen("/v1/runs/r1/stop", "POST")
    assert paused["tokens"] == {"input": 400_000, "output": 3_000, "cached": 380_000}
    bill = (20_000 * 0.10 + 380_000 * 0.02 + 3_000 * 0.35) / 1e6
    assert paused["eur"] == pytest.approx(bill, abs=1e-6) and paused["budget_eur"] == 0.015
    resumed = bridge.post(f"/v1/jobs/{job['id']}/resume", headers=AUTH, json={"budget_eur": 0.215}).json()
    assert resumed["state"] == "running" and resumed["budget_eur"] == 0.215 and resumed["runs"] == 2
    assert bridge.delete(f"/v1/jobs/{job['id']}", headers=AUTH).json() == {"ok": True}
