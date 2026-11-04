"""Routines on the bridge's own clock (bridge-rs/src/routines.rs, 2026-11-03): Hermes' cron runs none. Made by the
agent (`routine_schedule`) in a conversation or by the app (`/v1/agents/{agent}/routines`), kept by the bridge, run as
one background turn with the conversations' prompt (its rules in a hidden note of its message, never as run
instructions), in a hidden conversation of its own; its report a Boîte message where it was made (muted: stored
without a push), `[SILENT]` nothing; each agent's Hermes jobs read once and moved."""
import threading
import time

from starlette.responses import JSONResponse

from bbkit.common import AUTH, sse, boite_items
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until

TOKEN = "c" * 64
NOW = "2026-11-03T08:00:00.000Z"


def turns(fakes):
    return [r for r in fakes.hermes.started.values() if "<!--bip:routine" in (r.get("input") or "")]


def boite(bridge):
    return boite_items(bridge)


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


def run_rows(fakes, turn, reply):
    """What Hermes keeps of a routine's turn in the session it ran in."""
    fakes.hermes.messages[("", turn["session_id"])] = [{"role": "user", "content": turn["input"]},
                                                      {"role": "assistant", "content": reply}]


def timeline(bridge, thread):
    return bridge.get(f"/v1/threads/{thread}/timeline", headers=AUTH, params={"agent": "wellness"}).json()


def test_a_routine_answering_the_code_word_translated_tells_nothing(bridges, fakes):
    """Alex 4/11: a French routine answered « [PAS UN MOT] » (the code word translated) and it was delivered as a
    report, pushed, in the Boîte. Any short bracketed marker alone, in any language, is silent."""
    bridge = bridges.new()
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (TOKEN, "sandbox", "[]", NOW, NOW))])
    bridge.start()
    hermes = fakes.hermes
    hermes.rounds = 1
    hermes.add_session("api_main", [{"role": "user", "content": "Bonjour"}], title="Bot Chat")
    made = bridge.post("/v1/agents/wellness/routines", headers=AUTH,
                       json={"name": "Courriels importants", "schedule": "every 2h",
                             "prompt": "Regarde mes e-mails. S'il n'y a rien d'important, ne dis rien."})
    assert made.status_code in (200, 201), made.text
    job = made.json()["job"]
    # the note says the code word stays as written, the same words every run (one prompt prefix)
    for n, reply in enumerate(["[PAS UN MOT]", "[RIEN]", "SILENT", "", "[SILENT]"], start=1):
        hermes.run_output = reply
        assert bridge.post(f"/v1/routines/{job['id']}/run", headers=AUTH).status_code == 200
        assert wait_until(lambda: len(turns(fakes)) == n, timeout=20)
        assert wait_until(lambda: bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"][0]
                          ["last_status"] in ("silent", "ended completed") and
                          bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"][0]["last_run_at"],
                          timeout=20)
        time.sleep(0.5)
    notes = {t["input"].split("<!--bip:routine", 1)[1] for t in turns(fakes)}
    assert len(notes) == 1 and "never translated" in notes.pop()
    time.sleep(1.5)
    assert [i for i in boite(bridge) if i["title"] == "Courriels importants"] == []
    assert fakes.apns.payloads() == []
    # the run's hidden conversation shows no reply for the code word; the main one shows nothing of the runs
    turn = turns(fakes)[0]
    run_rows(fakes, turn, "[PAS UN MOT]")
    hidden = bridge.get("/v1/threads", headers=AUTH, params={"agent": "wellness", "include_hidden": 1}).json()["threads"]
    runs = [t for t in hidden if t["kind"] == "routine" and t["origin"]["routine"] == job["id"]]
    assert len(runs) == 5
    shown = [i for t in runs for i in timeline(bridge, t["id"])["items"]]
    assert not [i for i in shown if i.get("role") == "agent"]
    assert not [i for i in timeline(bridge, "main")["items"] if "Regarde mes e-mails" in (i.get("text") or "")]

    # a real report that starts with a tag is one; made in no conversation, it lands in the main one (its push opens
    # the person's conversation, not the run's hidden thread)
    hermes.run_output = "[Banque] Un virement de 120 € est arrivé."
    bridge.post(f"/v1/routines/{job['id']}/run", headers=AUTH)
    assert wait_until(lambda: [i for i in boite(bridge) if i["title"] == "Courriels importants"], timeout=20)
    [item] = [i for i in boite(bridge) if i["title"] == "Courriels importants"]
    assert item["session_id"] == "api_main"
    assert [i for i in timeline(bridge, "main")["items"] if i["kind"] == "boite"]
    # the run's hidden thread is named after the run, not left blank
    hidden = bridge.get("/v1/threads", headers=AUTH, params={"agent": "wellness", "include_hidden": 1}).json()["threads"]
    assert all((t["title"] or "").startswith("Courriels importants · ") for t in hidden if t["kind"] == "routine")


def test_a_routine_moved_from_a_hermes_job_of_the_main_conversation_runs_apart(bridges, fakes):
    """A routine made before 3/11 in the main conversation (« Bot Chat ») as a Hermes cron job, moved by the bridge:
    its runs go to hidden routine threads, never to the main conversation's timeline; the main one gets only the
    Boîte message of a report (none for a silent run). Both shapes of Hermes jobs: with `origin_session_id` (patch
    0019) and the older one with only `origin` (an api_server chat)."""
    hermes = fakes.hermes
    hermes.add_session("api_main", [{"role": "user", "content": "Préviens-moi des e-mails importants"},
                                    {"role": "assistant", "content": "C'est noté."}], title="Bot Chat")
    jobs = [{"id": "j_new", "name": "Courriels", "prompt": "Regarde mes e-mails importants.", "enabled": True,
             "state": "scheduled", "schedule": {"kind": "interval", "minutes": 30}, "deliver": "local",
             "origin_session_id": "api_main", "repeat": {"times": None, "completed": 9}},
            {"id": "j_old", "name": "Agenda", "prompt": "Regarde mon agenda de demain.", "enabled": True,
             "state": "scheduled", "schedule": {"kind": "cron", "expr": "0 18 * * *"}, "deliver": "origin",
             "origin": {"platform": "api_server", "chat_id": "api_main"}, "repeat": {"times": None, "completed": 2}}]

    def jobs_api(seen):
        if seen.path == "/api/jobs" and seen.method == "GET":
            return JSONResponse({"jobs": jobs})
        if seen.path.startswith("/api/jobs/") and seen.method == "DELETE":
            return JSONResponse({"ok": True})
        return None

    hermes.override = jobs_api
    bridge = bridges.new()
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (TOKEN, "sandbox", "[]", NOW, NOW))])
    bridge.start()
    assert wait_until(lambda: len(bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"]) == 2,
                      timeout=15)
    main = timeline(bridge, "main")
    assert main["thread"]["tip"] == "api_main"
    hermes.rounds = 4
    for n, (job, reply) in enumerate([("j_new", "Un e-mail de l'école : réunion jeudi."), ("j_old", "[PAS UN MOT]")],
                                     start=1):
        hermes.run_output = reply
        assert bridge.post(f"/v1/routines/{job}/run", headers=AUTH).status_code == 200
        assert wait_until(lambda: len(turns(fakes)) == n, timeout=20)
        turn = turns(fakes)[-1]
        # its own conversation, a hidden routine thread; nothing running in the main one meanwhile
        assert turn["session_id"] != "api_main"
        assert timeline(bridge, "main")["pending"]["run"] is None
        run_rows(fakes, turn, reply)
        assert wait_until(lambda: bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"]
                          and [j for j in bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"]
                               if j["id"] == job][0]["last_status"] in ("sent", "silent"), timeout=20)
    hidden = bridge.get("/v1/threads", headers=AUTH, params={"agent": "wellness", "include_hidden": 1}).json()["threads"]
    for job in ("j_new", "j_old"):
        [run] = [t for t in hidden if t["kind"] == "routine" and t["origin"]["routine"] == job]
        assert run["hidden"]
    items = timeline(bridge, "main")["items"]
    said = " ".join(i.get("text") or "" for i in items)
    assert "Regarde mes e-mails" not in said and "Regarde mon agenda" not in said and "PAS UN MOT" not in said
    assert [i for i in items if i["kind"] == "message"] and \
        [i["role"] for i in items if i["kind"] == "message"] == ["person", "agent"]
    # the report: one Boîte message in the main conversation; the silent run: none, no push for it
    [report] = [i for i in items if i["kind"] == "boite"]
    assert (report["title"], report["text"]) == ("Courriels", "Un e-mail de l'école : réunion jeudi.")
    assert [i for i in boite(bridge)] and not [i for i in boite(bridge) if "PAS UN MOT" in i["text"]]
    time.sleep(1.5)
    assert len([p for p in fakes.apns.payloads() if p.get("m")]) == 1
