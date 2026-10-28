"""The brake on a turn going round in circles (bridge-rs/src/loop_brake.rs, 2026-10-27): a run calling one tool too
many times, the same call again and again, or far too many tools is stopped (Hermes' `POST /v1/runs/{id}/stop`) and
the conversation gets a plain line (`bridge.notice`, code `loop_stopped`); the agents' write tools refuse the same
write twice in a turn, and too many writes. `[brake]` in bridge.toml."""
import threading

from bbkit.common import AUTH, HERMES_KEY, read_sse, sse
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until


def follow(bridge, run_id):
    with bridge.stream("GET", f"/v1/runs/{run_id}/events?agent=wellness&session_id=api_c", headers=AUTH) as resp:
        assert resp.status_code == 200
        return read_sse(resp)


def looping(run_id, count, tool="mcp_bipagents_calendar_add"):
    # the token lab's turn: the calendar again and again, each call's preview its own
    return ([sse("tool.started", run_id=run_id, tool=tool, preview=f"Test {i}") for i in range(count)]
            + [sse("run.completed", run_id=run_id, output="J'arrête là les vérifications calendrier.")])


def test_a_turn_going_round_in_circles_is_stopped_and_said(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.runs["run_loop"] = looping("run_loop", 20)
    events = follow(bridge, "run_loop")
    names = [name for name, _ in events]
    assert names.index("bridge.notice") == 11, "right after the 11th call of the same tool"
    notice = events[11][1]
    assert notice["code"] == "loop_stopped" and notice["reason"] == "same tool too many times (calendar_add)"
    assert "agenda" in notice["text"] or "calendar" in notice["text"]
    assert names.count("bridge.notice") == 1
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/run_loop/stop", "POST"))
    assert not fakes.hermes.steers, "stopped, not steered as long work"
    assert bridge.logged("going round in circles: run stopped")
    # replayed to an app that comes back
    assert "bridge.notice" in [name for name, _ in follow(bridge, "run_loop")]


def test_the_same_call_three_times_and_the_limits_in_the_config(bridges, fakes):
    bridge = bridges.new()
    bridge.config["brake"] = {"same_tool": 50}
    bridge.config["jobs"] = {"handoff": False}   # no long-work steer here
    bridge.start()
    fakes.hermes.runs["run_same"] = (
        [sse("tool.started", run_id="run_same", tool="memory", preview="add: le code du portail") for _ in range(3)]
        + [sse("run.completed", run_id="run_same", output="ok")])
    notices = [data for name, data in follow(bridge, "run_same") if name == "bridge.notice"]
    assert [n["reason"] for n in notices] == ["same call repeated (memory)"]
    # 20 different calendar calls: under the configured 50
    fakes.hermes.runs["run_ok"] = looping("run_ok", 20)
    assert "bridge.notice" not in [name for name, _ in follow(bridge, "run_ok")]
    assert not fakes.hermes.seen("/v1/runs/run_ok/stop", "POST")


def test_the_brake_switched_off(bridges, fakes):
    bridge = bridges.new()
    bridge.config["brake"] = {"enabled": False}
    bridge.config["jobs"] = {"handoff": False}
    bridge.start()
    fakes.hermes.runs["run_off"] = looping("run_off", 15)
    assert "bridge.notice" not in [name for name, _ in follow(bridge, "run_off")]


def test_writes_in_a_turn_are_capped(bridges, fakes):
    bridge = bridges.new()
    bridge.config["brake"] = {"writes": 2}
    bridge.start()
    give_token(bridge)
    page = {"title": "Plan", "html": "<h1>Plan</h1>"}
    first = call(bridge, "create_page", page)
    assert not first["isError"], text_of(first)
    # with no run followed, the same write again is not refused (the person may have asked again): only counted
    # (the same write twice within a run: the Rust unit tests, loop_brake.rs)
    assert not call(bridge, "create_page", page)["isError"]
    refused = call(bridge, "create_page", {**page, "title": "Plan 3"})
    assert refused["isError"] and "you already made 2 changes" in text_of(refused)
    # reading is never capped
    assert not call(bridge, "list_pages", {})["isError"]


# -- 2026-10-27 evening: the token lab's bench (internal/research/token-diet-step3-2026-10-27.md) showed the brake
# never stopping a turn, and refusing a reminder at the first write: the lab started and followed its runs through the
# door's Hermes relay (/hermes/<agent>/v1/runs…), invisible to the run hub. Each test drives a turn as the app or the
# lab does: `POST /v1/runs` through the relay, then the run's events.

HERMES_AUTH = {"Authorization": f"Bearer {HERMES_KEY}"}


def start(bridge, session="api_c"):
    posted = bridge.post("/hermes/wellness/v1/runs", json={"input": "C'est quand l'anniversaire de Julie ?",
                                                           "session_id": session}, headers=HERMES_AUTH)
    assert posted.status_code == 202, posted.text
    return posted.json()["run_id"]


def follow_relay(bridge, run_id):
    with bridge.stream("GET", f"/hermes/wellness/v1/runs/{run_id}/events", headers=HERMES_AUTH) as resp:
        assert resp.status_code == 200
        return read_sse(resp)


def bench_turn(run_id, tools):
    # each call's preview its own (the bench's events kept none): the limits on one tool, not « the same call »
    return ([sse("tool.started", run_id=run_id, tool=tool, preview=f"step {i}") for i, tool in enumerate(tools)]
            + [sse("run.completed", run_id=run_id, output="")])


def test_the_benchs_birthday_loop_started_through_the_door_is_stopped(bridges, fakes):
    # glm-all-2, birthday_lookup: 20 notebook_note (the bench's real sequence), 11 min, no answer
    bridge = bridges.new().start()
    note = "mcp__bipagents__notebook_note"
    tools = (["mcp__bipagents__notebook_search", note, "tool_search", note, note, "mcp__bipagents__notebook_read",
              "terminal", note, "tool_call", "tool_search", note, "terminal"] + [note] * 4 + ["terminal"] + [note] * 3
             + ["terminal"] + [note] * 8 + ["terminal"])
    fakes.hermes.runs["r1"] = bench_turn("r1", tools)
    run_id = start(bridge)
    assert run_id == "r1"
    for events in (follow(bridge, run_id), follow_relay(bridge, run_id)):   # the app's way, the lab's way
        names = [name for name, _ in events]
        assert names.count("bridge.notice") == 1
        at = names.index("bridge.notice")
        assert [data.get("tool") for name, data in events[:at]].count(note) == 11, "right after the 11th note"
        notice = events[at][1]
        assert notice["code"] == "loop_stopped" and notice["reason"] == "same tool too many times (notebook_note)"
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/r1/stop", "POST"))
    assert len(fakes.hermes.event_requests) == 1, "one Hermes subscription for the hub, the app and the lab"


def test_the_benchs_draft_loop_followed_only_through_the_relay_is_stopped(bridges, fakes):
    # glm-concise-2, draft_reply: 25 notebook_search
    bridge = bridges.new().start()
    search = "mcp__bipagents__notebook_search"
    fakes.hermes.runs["r1"] = bench_turn("r1", ["mcp__bipagents__calendar_list", search, search, "tool_describe"]
                                         + [search] * 23)
    events = follow_relay(bridge, start(bridge))
    notices = [data for name, data in events if name == "bridge.notice"]
    assert [n["reason"] for n in notices] == ["same tool too many times (notebook_search)"]
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/r1/stop", "POST"))


def test_a_page_the_browser_cant_read_ends_on_a_plain_line_before_hermes_cap(bridges, fakes):
    # glm-concise-2, browser_public: 145 page reads, cut by Hermes' cap of 150 with no answer
    bridge = bridges.new()
    bridge.config["jobs"] = {"handoff": False}
    bridge.start()
    read = "mcp__stealth__get_page_content"
    fakes.hermes.runs["r1"] = bench_turn("r1", ["skill_view", "mcp__stealth__spawn_browser", "mcp__stealth__navigate",
                                                read, "terminal", "tool_describe"] + [read] * 144)
    events = follow_relay(bridge, start(bridge))
    names = [name for name, _ in events]
    at = names.index("bridge.notice")
    assert [data.get("tool") for name, data in events[:at]].count(read) == 16, "right after the 16th page read"
    notice = events[at][1]
    assert notice["reason"] == "the browser read the page too many times"
    assert "lire cette page" in notice["text"] or "read this page" in notice["text"]
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/r1/stop", "POST"))


def test_a_turn_nobody_follows_is_stopped_too(bridges, fakes):
    # a run started through the relay that nobody follows (the app polls its status, a peer's message): stopped, and
    # the plain line pushed to the phone
    bridge = bridges.new()
    now = "2026-10-28T08:00:00.000Z"
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     ("1" * 64, "production", "[]", now, now))])
    bridge.start()
    fakes.hermes.runs["r1"] = looping("r1", 20)
    start(bridge)
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/r1/stop", "POST"))
    assert bridge.logged("going round in circles: run stopped")
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if p.get("run_id") == "r1"])
    pushed = [p for p in fakes.apns.payloads() if p.get("run_id") == "r1"]
    assert len(pushed) == 1 and pushed[0]["session_id"] == "api_c"
    said = bridge.get(f"/v1/replies/{pushed[0]['reply_id']}", headers=AUTH).json()
    assert "circles" in str(said) or "rond" in str(said), said


def test_two_reminders_in_a_fresh_conversation_are_accepted(bridges, fakes):
    # the bench's false positives: earlier writes of the agent (a routine, other conversations) counted with this one's
    bridge = bridges.new().start()
    give_token(bridge)
    assert bridge.put("/v1/reminders/agents", json={"agents": ["Wellness"]}, headers=AUTH).status_code == 200
    assert bridge.put("/v1/reminders", json={"lists": [{"id": "L1", "title": "Maison"}], "items": []},
                      headers=AUTH).status_code == 200
    for i in range(10):   # a routine before (no run): its 10 pages, its cap reached
        assert not call(bridge, "create_page", {"title": f"Brief {i}", "html": f"<h1>{i}</h1>"})["isError"]
    assert call(bridge, "create_page", {"title": "Brief 11", "html": "<h1>11</h1>"})["isError"]
    # the person's new conversation: its turn started through the door, still going on
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("tool.started", run_id="r1", tool="mcp__bipagents__reminders_add", preview="{}"),
                               gate, sse("run.completed", run_id="r1", output="C'est noté.")]
    start(bridge, session="api_new")
    assert wait_until(lambda: fakes.hermes.event_requests)
    for title in ("Pharmacie", "Banque"):
        added = call(bridge, "reminders_add", {"title": title, "due": "2026-10-28T09:00", "list": "Maison"})
        assert not added["isError"], text_of(added)
    # the same reminder again in this turn: « already done », not a second request
    again = call(bridge, "reminders_add", {"title": "Banque", "due": "2026-10-28T09:00", "list": "Maison"})
    assert not again["isError"] and "Already" in text_of(again)
    gate.set()


def test_a_routine_going_round_in_circles_is_told_to_stop(bridges, fakes):
    # the morning brief (a routine: Hermes' cron, no run to stop): 35 Carnet searches in the bench
    bridge = bridges.new().start()
    give_token(bridge)
    results = [call(bridge, "list_pages", {}) for _ in range(12)]
    assert not any(r["isError"] for r in results[:10])
    assert results[10]["isError"] and "answer the person with what you have" in text_of(results[10])


# -- 2026-10-28 09:24, Alex's home server: Vie diagnosing a failing nightly job was stopped after 5 calls, 3 of them the
# same session_search, and the iPhone showed Hermes' English « This turn did not complete… » instead of the brake's
# line. Now: lookups take 5 identical calls, and a stopped conversation turn is followed by one short rescue turn
# whose reply comes in the same stream, under the brake's short line.

SEARCH = "session_search"
NIGHT = "scan-emails-fond-nuit"


def diagnosis(run_id, searches):
    return ([sse("message.delta", run_id=run_id, delta=f"Je cherche « {NIGHT} » dans nos anciennes discussions…")]
            + [sse("tool.started", run_id=run_id, tool=SEARCH, preview=NIGHT) for _ in range(searches)]
            + [sse("run.cancelled", run_id=run_id)])


def notices(events):
    return [data for name, data in events if name == "bridge.notice"]


def test_the_mornings_diagnosis_is_not_stopped_any_more(bridges, fakes):
    bridge = bridges.new().start()
    tools = [(SEARCH, NIGHT), ("skill_view", "cronjob"), (SEARCH, NIGHT), ("read_file", "jobs.json"), (SEARCH, NIGHT)]
    fakes.hermes.runs["run_vie"] = ([sse("tool.started", run_id="run_vie", tool=t, preview=p) for t, p in tools]
                                    + [sse("run.completed", run_id="run_vie", output="Le job échoue sur IMAP.")])
    events = follow(bridge, "run_vie")
    assert notices(events) == []
    assert not fakes.hermes.seen("/v1/runs/run_vie/stop", "POST")


def test_a_stopped_turn_ends_on_the_agents_own_reply_after_the_brakes_line(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.runs["run_vie"] = diagnosis("run_vie", 5)
    # the rescue turn the bridge starts (the fake's first POST /v1/runs)
    reply = ("Je voulais retrouver pourquoi le job de nuit échoue. J'ai trouvé trois sessions sans erreur claire. "
             "Veux-tu que je regarde les journaux du serveur, ou que je relance le job maintenant ?")
    fakes.hermes.runs["r1"] = [sse("message.delta", run_id="r1", delta=reply),
                               sse("run.completed", run_id="r1", output=reply)]
    events = follow(bridge, "run_vie")
    said = notices(events)
    assert [n["code"] for n in said] == ["loop_stopped", "loop_rescue_started"]
    stopped = said[0]
    assert stopped["reason"] == "same call repeated (session_search)" and stopped["rescue"] is True
    assert stopped["text"] in ("Je me suis arrêté : je relançais la même recherche dans nos anciennes discussions.",
                               "I stopped: I kept repeating the same search in our past conversations.")
    assert SEARCH not in stopped["text"], "never a tool's name"
    assert said[1]["run_id"] == "r1"
    names = [name for name, _ in events]
    # the stream ends on the rescue turn's reply, not on the stopped run's cancellation
    assert names[-1] == "run.completed" and events[-1][1]["output"] == reply
    assert "run.cancelled" not in names
    assert any(name == "message.delta" and data.get("delta") == reply for name, data in events)
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/run_vie/stop", "POST"))
    # one rescue turn, in the same conversation, its instruction out of the person's sight
    assert list(fakes.hermes.started) == ["r1"]
    rescue = fakes.hermes.started["r1"]
    assert rescue["session_id"] == "api_c"
    assert rescue["input"].startswith("[bipagents:loop_rescue] ")
    assert "Don't call session_search again in this turn." in rescue["input"]
    assert bridge.logged("going round in circles: the rescue turn answered")
    # an app coming back replays the same
    again = follow(bridge, "run_vie")
    assert [n["code"] for n in notices(again)] == ["loop_stopped", "loop_rescue_started"]
    assert again[-1][0] == "run.completed"


def test_a_rescue_turn_that_loops_is_not_rescued_again(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.runs["run_vie"] = diagnosis("run_vie", 5)
    # the rescue turn calls the stopped tool again: stopped at its second call, and no second rescue
    fakes.hermes.runs["r1"] = ([sse("tool.started", run_id="r1", tool=SEARCH, preview=f"{NIGHT} {i}") for i in range(4)]
                               + [sse("run.cancelled", run_id="r1")])
    events = follow(bridge, "run_vie")
    said = notices(events)
    assert [n["code"] for n in said] == ["loop_stopped", "loop_rescue_started", "loop_rescue_failed"]
    assert said[2]["text"] in ("Dis-moi si tu veux que je réessaie autrement.", "Tell me if you want me to try another way.")
    assert events[-1][0] == "run.cancelled" and events[-1][1]["run_id"] == "run_vie"
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/r1/stop", "POST"))
    assert list(fakes.hermes.started) == ["r1"], "never a rescue of the rescue"
    assert bridge.logged("going round in circles: the rescue turn did not answer")


def test_a_turn_nobody_follows_gets_no_rescue_turn(bridges, fakes):
    # a run nobody follows (a peer's message, the lab's runs polled): stopped as before, no rescue turn
    bridge = bridges.new().start()
    fakes.hermes.runs["r1"] = diagnosis("r1", 5)
    start(bridge)
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/r1/stop", "POST"))
    assert wait_until(lambda: bridge.logged("going round in circles: run stopped"))
    assert list(fakes.hermes.started) == ["r1"], "no rescue turn started"
