"""Routines on the bridge's own clock (bridge-rs/src/routines.rs, 2026-11-03): Hermes' cron runs none. Made by the
agent (`routine_schedule`) in a conversation or by the app (`/v1/agents/{agent}/routines`), kept by the bridge, run as
one background turn with the conversations' prompt (its rules in a hidden note of its message, never as run
instructions), in a hidden conversation of its own; its report a Boîte message where it was made (muted: stored
without a push), `[SILENT]` nothing; each agent's Hermes jobs read once and moved."""
import threading
import time

from starlette.responses import JSONResponse

from bbkit.common import AUTH, sse
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until

TOKEN = "c" * 64
NOW = "2026-11-03T08:00:00.000Z"


def turns(fakes):
    return [r for r in fakes.hermes.started.values() if "<!--bip:routine" in (r.get("input") or "")]


def boite(bridge):
    return bridge.get("/v1/outbox", headers=AUTH).json()["items"]


def test_a_routine_made_in_a_conversation_runs_with_the_conversation_prompt_and_reports_there(bridges, fakes):
    bridge = bridges.new()
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (TOKEN, "sandbox", "[]", NOW, NOW))])
    bridge.start()
    give_token(bridge)
    hermes = fakes.hermes
    hermes.rounds = 1
    hermes.add_session("api_c", [{"role": "user", "content": "Chaque matin, la météo"}], title="Météo")
    # the agent makes it in that conversation (the run the app follows there)
    gate = threading.Event()
    hermes.runs["run_c"] = [sse("tool.started", run_id="run_c", tool="mcp_bipagents_routine_schedule", preview="x"),
                            gate, sse("run.completed", run_id="run_c", output="C'est noté.")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_c", "session_id": "api_c"})
    assert wait_until(lambda: hermes.seen("/v1/runs/run_c/events"))
    time.sleep(0.3)
    made = call(bridge, "routine_schedule", {"action": "create", "name": "Météo du matin",
                                             "schedule": "every day at 7:30", "prompt": "Dis-moi la météo du jour."})
    assert not made["isError"], text_of(made)
    gate.set()
    [job] = bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"]
    assert (job["name"], job["schedule"]["expr"], job["state"]) == ("Météo du matin", "30 7 * * *", "scheduled")
    assert job["origin_session_id"] == "api_c" and job["next_run_at"]
    assert "Météo du matin" in text_of(call(bridge, "routine_schedule", {"action": "list"}))

    # run now: one turn in a hidden conversation of its own, the rules in its message, no run instructions
    hermes.run_output = "Soleil toute la journée, 18 °C."
    assert bridge.post(f"/v1/routines/{job['id']}/run", headers=AUTH).status_code == 200
    assert wait_until(lambda: turns(fakes), timeout=20)
    [turn] = turns(fakes)
    assert "Dis-moi la météo du jour.\n\n<!--bip:routine\nThis is your routine « Météo du matin »" in turn["input"]
    assert not turn.get("instructions") and turn["session_id"] != "api_c"
    created = [body for agent, body in hermes.created if body.get("title", "").startswith("Météo du matin · ")]
    assert created
    # its report in the Boîte of the conversation it was made in, pushed; the agent finds it there with the next message
    assert wait_until(lambda: [i for i in boite(bridge) if i["title"] == "Météo du matin"], timeout=20)
    item = [i for i in boite(bridge) if i["title"] == "Météo du matin"][0]
    assert item["text"] == "Soleil toute la journée, 18 °C." and item["session_id"] == "api_c"
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if p.get("m") == f"o_{item['id']}"], timeout=20)
    # its run's conversation is a hidden routine thread, not one of the person's
    listed = bridge.get("/v1/threads", headers=AUTH, params={"agent": "wellness"}).json()["threads"]
    assert not [t for t in listed if t["kind"] == "routine"]
    hidden = bridge.get("/v1/threads", headers=AUTH, params={"agent": "wellness", "include_hidden": 1}).json()["threads"]
    assert [t for t in hidden if t["kind"] == "routine" and t["origin"]["routine"] == job["id"]]
    ran = bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"][0]
    assert ran["last_status"] == "sent" and ran["last_run_at"]

    # muted: stored without a push; [SILENT]: nothing
    assert bridge.put(f"/v1/cron-jobs/wellness/{job['id']}", headers=AUTH, json={"notify": False}).status_code == 200
    hermes.run_output = "Pluie l'après-midi."
    pushes = len(fakes.apns.payloads())
    bridge.post(f"/v1/routines/{job['id']}/run", headers=AUTH)
    assert wait_until(lambda: [i for i in boite(bridge) if i["text"] == "Pluie l'après-midi."], timeout=20)
    time.sleep(1.5)
    assert len(fakes.apns.payloads()) == pushes
    hermes.run_output = "[SILENT]"
    bridge.post(f"/v1/routines/{job['id']}/run", headers=AUTH)
    assert wait_until(lambda: len(turns(fakes)) == 3, timeout=20)
    assert wait_until(lambda: bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"][0]["last_status"]
                      == "silent", timeout=20)
    assert len([i for i in boite(bridge) if i["title"] == "Météo du matin"]) == 2

    # pause, resume, change, delete (the app's routes)
    paused = bridge.post(f"/v1/routines/{job['id']}/pause", headers=AUTH).json()["job"]
    assert (paused["state"], paused["next_run_at"]) == ("paused", None)
    resumed = bridge.post(f"/v1/routines/{job['id']}/resume", headers=AUTH).json()["job"]
    assert resumed["state"] == "scheduled" and resumed["next_run_at"]
    changed = bridge.patch(f"/v1/routines/{job['id']}", headers=AUTH, json={"schedule": "every 2h"}).json()["job"]
    assert changed["schedule"] == {"kind": "interval", "minutes": 120, "display": "every 120m"}
    assert bridge.patch(f"/v1/routines/{job['id']}", headers=AUTH, json={"schedule": "un jour"}).status_code == 422
    assert bridge.delete(f"/v1/routines/{job['id']}", headers=AUTH).status_code == 200
    assert bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"] == []
    assert bridge.get("/v1/agents/wellness/routines").status_code == 401


def test_the_hermes_jobs_move_to_the_bridge_once(bridges, fakes):
    jobs = [{"id": "j1", "name": "Point du lundi", "prompt": "Fais le point", "enabled": True, "state": "scheduled",
             "schedule": {"kind": "cron", "expr": "0 8 * * 1"}, "origin_session_id": "api_c",
             "repeat": {"times": None, "completed": 4}, "skills": ["plan"]}]

    def hermes(seen):
        if seen.path == "/api/jobs" and seen.method == "GET":
            return JSONResponse({"jobs": jobs})
        if seen.path == "/api/jobs/j1" and seen.method == "DELETE":
            return JSONResponse({"ok": True})
        return None

    fakes.hermes.override = hermes
    bridge = bridges.new().start()
    assert wait_until(lambda: bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"], timeout=15)
    [moved] = bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"]
    assert (moved["id"], moved["schedule"]["expr"], moved["origin_session_id"], moved["skills"]) == \
        ("j1", "0 8 * * 1", "api_c", ["plan"])
    assert wait_until(lambda: fakes.hermes.seen("/api/jobs/j1", "DELETE"), timeout=10)
    time.sleep(1.0)
    assert len(fakes.hermes.seen("/api/jobs", "GET")) == 1   # once
    bridge.restart()
    time.sleep(2.0)
    assert len(fakes.hermes.seen("/api/jobs", "GET")) == 1 and \
        len(bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"]) == 1
