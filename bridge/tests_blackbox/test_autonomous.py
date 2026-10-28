"""« Ce que tes agents font tout seuls » (`GET /v1/autonomous`, bridge-rs/src/autonomous.rs): every agent's routines
(its Hermes' `/api/jobs`), background tasks and mailbox scans (jobs.rs) and connectors on a mechanical schedule
(connectors.rs) in one list, grouped (schedule / on_change / now), with a plain state, the last and next run, the cost
fields and the actions; an agent whose Hermes doesn't answer is named, with its routines as last seen."""
import json
import time

from starlette.responses import JSONResponse

from bbkit.common import AUTH, HERMES_KEY

ROUTINES = [
    {"id": "j1", "name": "Point du lundi", "prompt": "Fais le point sur ma semaine", "state": "scheduled",
     "enabled": True, "schedule": {"kind": "cron", "expr": "0 8 * * 1"}, "schedule_display": "0 8 * * 1",
     "next_run_at": "2026-11-02T08:00:00+00:00", "last_run_at": "2026-10-26T08:00:00+00:00", "last_status": "ok"},
    {"id": "j2", "name": "Eau", "prompt": "Rappelle-moi de boire", "state": "paused", "enabled": False,
     "schedule": {"kind": "interval", "minutes": 120}},
]


def seeded(bridges, fakes, coach_down=True):
    bridge = bridges.new()
    bridge.config["agents"]["coach"] = {"display_name": "Coach", "hermes_url": f"{fakes.hermes.url}/p/coach",
                                        "hermes_key": HERMES_KEY}
    now = time.time()
    jobs = bridge.state_dir / "jobs"
    jobs.mkdir(parents=True, exist_ok=True)
    tasks = {
        "t0000000000a1": {"state": "proposed", "title": "Écoles du quartier", "created": now - 60,
                          "max_tokens": 300_000, "proposed_by": "agent"},
        "t0000000000a2": {"state": "done", "title": "Vieux rapport", "created": now - 9 * 86400,
                          "finished": now - 8 * 86400},
        "t0000000000a3": {"state": "done", "title": "Comparatif vélos", "created": now - 7200, "finished": now - 3600,
                          "tokens": {"input": 120_000, "output": 4_000, "cached": 0}, "max_tokens": 1_000_000},
    }
    for job_id, fields in tasks.items():
        job = {"id": job_id, "kind": "task", "agent": "wellness", "prompt": "…", "tokens": {"input": 0, "output": 0},
               **fields}
        (jobs / f"{job_id}.json").write_text(json.dumps(job))
    folder = bridge.state_dir / "connectors"
    folder.mkdir(parents=True, exist_ok=True)
    garmin = {"id": "c1", "name": "Garmin", "slug": "garmin", "status": "approved", "enabled": True,
              "last_scheduled": now - 600, "schedule": {"when": "daily 07:00", "mechanical": True, "tool": "sync"},
              "last_run": {"at": "2026-10-27T07:00:00+00:00", "ok": False, "message": "Le service n'a pas répondu à temps.",
                           "via": "schedule"}}
    manual = {"id": "c2", "name": "Strava", "slug": "strava", "status": "approved", "enabled": True}
    (folder / "connectors.json").write_text(json.dumps({"agents": {"coach": [garmin, manual]}}))

    def hermes(seen):
        if seen.path == "/api/jobs" and seen.method == "GET":
            return JSONResponse({"jobs": ROUTINES})
        if seen.path == "/p/coach/api/jobs" and coach_down:
            return JSONResponse({"error": "down"}, status_code=503)
        return None

    fakes.hermes.override = hermes
    return bridge.start()


def test_one_list_of_what_the_agents_do_on_their_own(bridges, fakes):
    bridge = seeded(bridges, fakes)
    answer = bridge.get("/v1/autonomous", headers=AUTH)
    assert answer.status_code == 200, answer.text
    body = answer.json()
    assert body["groups"] == ["schedule", "on_change", "now"]
    assert body["unreachable"] == ["coach"]
    assert body["budgets"]["medium"] == 1_000_000
    items = {i["id"]: i for i in body["items"]}
    assert set(items) == {"routine:wellness:j1", "routine:wellness:j2", "task:t0000000000a1", "task:t0000000000a3",
                          "connector:coach:c1"}
    monday = items["routine:wellness:j1"]
    assert (monday["kind"], monday["group"], monday["state"], monday["main_action"]) == \
        ("routine", "schedule", "active", "pause")
    assert monday["when"] == {"kind": "weekly", "weekday": 1, "hour": 8, "minute": 0, "text": "Chaque lundi à 8:00"}
    assert monday["last_run"]["ok"] is True and monday["next_run"] > 0 and monday["cost"] is None
    assert monday["actions"] == ["pause", "run", "delete", "open"] and monday["ref"] == "j1"
    water = items["routine:wellness:j2"]
    assert (water["state"], water["main_action"], water["when"]["text"]) == ("paused", "resume", "Toutes les 2 h")
    proposal = items["task:t0000000000a1"]
    assert (proposal["group"], proposal["state"], proposal["main_action"]) == ("now", "waiting", "start")
    assert proposal["cost"]["max_tokens"] == 300_000 and proposal["ref"] == "t0000000000a1"
    done = items["task:t0000000000a3"]
    assert done["state"] == "done" and done["last_run"]["ok"] is True and done["cost"]["tokens"]["input"] == 120_000
    garmin = items["connector:coach:c1"]
    assert (garmin["state"], garmin["when"]["text"], garmin["cost"]) == ("active", "Chaque jour à 7:00", {"ai": False})
    assert garmin["last_run"]["ok"] is False and "pas répondu" in garmin["last_run"]["error"]
    assert garmin["next_run"] > time.time() - 600
    # groups in order: the schedules first, then what runs now
    groups = [i["group"] for i in body["items"]]
    assert groups == sorted(groups, key=["schedule", "on_change", "now"].index)
    # the Hermes call: with the paused routines, with the agent's key
    call = [r for r in fakes.hermes.seen("/api/jobs", "GET")][0]
    assert "include_disabled=true" in call.query and call.header("authorization") == f"Bearer {HERMES_KEY}"


def test_one_agent_and_the_routines_last_seen(bridges, fakes):
    bridge = seeded(bridges, fakes, coach_down=False)
    coach_jobs = [{"id": "k1", "name": "Séance du jour", "enabled": True, "schedule": "0 7 * * *"}]

    def hermes(seen):
        if seen.path == "/p/coach/api/jobs":
            return JSONResponse({"jobs": coach_jobs})
        return None

    fakes.hermes.override = hermes
    first = bridge.get("/v1/autonomous", headers=AUTH, params={"agent": "coach"}).json()
    assert {i["id"] for i in first["items"]} == {"routine:coach:k1", "connector:coach:c1"} and first["unreachable"] == []
    assert first["items"][0]["when"]["text"] in ("Chaque jour à 7:00",)

    def down(seen):
        if seen.path == "/p/coach/api/jobs":
            return JSONResponse({"error": "asleep"}, status_code=502)
        return None

    fakes.hermes.override = down
    again = bridge.get("/v1/autonomous", headers=AUTH, params={"agent": "coach"}).json()
    assert again["unreachable"] == ["coach"]
    assert "routine:coach:k1" in {i["id"] for i in again["items"]}   # as last seen
    assert bridge.get("/v1/autonomous", headers=AUTH, params={"agent": "nobody"}).status_code == 404
    assert bridge.get("/v1/autonomous").status_code == 401


def test_a_connector_run_now_needs_its_schedule(bridges, fakes):
    bridge = seeded(bridges, fakes)
    no_schedule = bridge.post("/v1/agents/coach/connectors/c2/run", headers=AUTH)
    assert no_schedule.status_code == 409 and no_schedule.json()["detail"]["code"] == "no_schedule"
    assert bridge.post("/v1/agents/coach/connectors/zz/run", headers=AUTH).status_code == 404
    off = bridge.patch("/v1/agents/coach/connectors/c1", headers=AUTH, json={"enabled": False})
    assert off.status_code == 200
    paused = bridge.post("/v1/agents/coach/connectors/c1/run", headers=AUTH)
    assert paused.status_code == 409 and paused.json()["detail"]["code"] == "not_running"
    row = next(i for i in bridge.get("/v1/autonomous", headers=AUTH).json()["items"] if i["id"] == "connector:coach:c1")
    assert (row["state"], row["main_action"], row["next_run"]) == ("paused", "resume", None)
