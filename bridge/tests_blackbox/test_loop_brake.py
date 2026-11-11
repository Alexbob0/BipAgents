"""The brake on a turn going round in circles (bridge-rs/src/loop_brake.rs, 2026-10-27): a run calling one tool too
many times, the same call again and again, or far too many tools is stopped (Hermes' `POST /v1/runs/{id}/stop`) and
the conversation gets a plain line (`bridge.notice`, code `loop_stopped`); the agents' write tools refuse the same
write twice in a turn, and too many writes. `[brake]` in bridge.toml."""
import json
import threading
import time

from bbkit.common import AUTH, app_turn, follow_run, read_sse, sse, in_session
from bbkit.mcp import call, give_token, media_of, text_of
from bbkit.servers import wait_until
from conftest import managed_install, profile


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
    assert [d.get("code") for n, d in events if n == "bridge.notice"].count("loop_stopped") == 1
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/run_loop/stop", "POST"))
    assert not [s for s in fakes.hermes.steers if "[bipagents:handoff]" in s["body"]["input"]], "not as long work"
    # told once before the stop (a steer: the 7th of 10 calls of one tool)
    warned = [s for s in fakes.hermes.steers if s["body"]["input"].startswith("[bipagents:brake]")]
    assert len(warned) == 1 and "calendar_add 7 times" in warned[0]["body"]["input"], warned
    assert bridge.logged("brake_warned: the agent told it is going round in circles")
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
    notices = [data for name, data in follow(bridge, "run_same") if name == "bridge.notice" and data.get("code") == "loop_stopped"]
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
    # no run followed here: a routine's calls, scheduled work held to the task's writes
    bridge.config["brake"] = {"task_writes": 2}
    bridge.start()
    give_token(bridge)
    page = {"title": "Plan", "html": "<h1>Plan</h1>"}
    first = call(bridge, "create_page", page)
    assert not first["isError"], text_of(first)
    # with no run followed, the same write again is not refused (the person may have asked again): only counted
    # (the same write twice within a run: the Rust unit tests, loop_brake.rs)
    assert not call(bridge, "create_page", page)["isError"]
    refused = call(bridge, "create_page", {**page, "title": "Plan 3"})
    assert refused["isError"] and "this run has made 2 changes" in text_of(refused)
    # the reason, for the person, in plain words (2026-10-31: « le bridge a refusé la suite »)
    assert "so that a mistake can never spread to everything" in text_of(refused)
    # reading is never capped
    assert not call(bridge, "list_pages", {})["isError"]


# -- 2026-10-27 evening: the token lab's bench (internal/research/token-diet-step3-2026-10-27.md) showed the brake
# never stopping a turn, and refusing a reminder at the first write: the lab started and followed its runs through the
# door's Hermes relay (gone since), invisible to the run hub. Each test drives a turn as the app does: its message
# (`POST /v1/threads/{id}/messages`), then the run's events.


def start(bridge, fakes, session="api_c"):
    return app_turn(bridge, fakes, "C'est quand l'anniversaire de Julie ?", session)


def bench_turn(run_id, tools):
    # each call's preview its own (the bench's events kept none): the limits on one tool, not « the same call »
    return ([sse("tool.started", run_id=run_id, tool=tool, preview=f"step {i}") for i, tool in enumerate(tools)]
            + [sse("run.completed", run_id=run_id, output="")])


def test_the_benchs_birthday_loop_started_through_the_door_is_stopped(bridges, fakes):
    # glm-all-2, birthday_lookup: 20 Carnet writes (the bench's real sequence), 11 min, no answer
    bridge = bridges.new().start()
    note = "mcp__bipagents__notebook_write"
    tools = (["mcp__bipagents__notebook_search", note, "tool_search", note, note, "mcp__bipagents__notebook_read",
              "terminal", note, "tool_call", "tool_search", note, "terminal"] + [note] * 4 + ["terminal"] + [note] * 3
             + ["terminal"] + [note] * 8 + ["terminal"])
    fakes.hermes.runs["r1"] = bench_turn("r1", tools)
    run_id = start(bridge, fakes)
    assert run_id == "r1"
    events = follow(bridge, run_id)
    names = [name for name, _ in events]
    assert [d.get("code") for n, d in events if n == "bridge.notice"].count("loop_stopped") == 1
    at = names.index("bridge.notice")
    assert [data.get("tool") for name, data in events[:at]].count(note) == 11, "right after the 11th note"
    notice = events[at][1]
    assert notice["code"] == "loop_stopped" and notice["reason"] == "same tool too many times (notebook_write)"
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/r1/stop", "POST"))
    turn = [s for s in fakes.hermes.event_requests if s.path == "/v1/runs/r1/events"]
    assert len(turn) == 1, "one Hermes subscription for the hub and the app (the rescue turn's apart)"


def test_the_benchs_draft_loop_is_stopped(bridges, fakes):
    # glm-concise-2, draft_reply: 25 notebook_search
    bridge = bridges.new().start()
    search = "mcp__bipagents__notebook_search"
    fakes.hermes.runs["r1"] = bench_turn("r1", ["mcp__bipagents__calendar_list", search, search, "tool_describe"]
                                         + [search] * 23)
    events = follow_run(bridge, start(bridge, fakes))
    notices = [data for name, data in events if name == "bridge.notice" and data.get("code") == "loop_stopped"]
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
    events = follow_run(bridge, start(bridge, fakes))
    names = [name for name, _ in events]
    at = names.index("bridge.notice")
    assert [data.get("tool") for name, data in events[:at]].count(read) == 16, "right after the 16th page read"
    notice = events[at][1]
    assert notice["reason"] == "the browser read the page too many times"
    assert "lire cette page" in notice["text"] or "read this page" in notice["text"]
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/r1/stop", "POST"))


def test_a_turn_nobody_follows_is_stopped_too(bridges, fakes):
    # a turn nobody follows (the app closed): stopped, and the plain line pushed to the phone
    bridge = bridges.new()
    now = "2026-10-28T08:00:00.000Z"
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     ("1" * 64, "production", "[]", now, now))])
    bridge.start()
    fakes.hermes.runs["r1"] = looping("r1", 20)
    start(bridge, fakes)
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/r1/stop", "POST"))
    assert bridge.logged("going round in circles: run stopped")
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if p.get("r") == "r1"])
    pushed = [p for p in fakes.apns.payloads() if p.get("r") == "r1"]
    assert len(pushed) == 1 and in_session(bridge, pushed[0], "api_c")
    said = bridge.get(f"/v1/notifications/{pushed[0]['n']}", headers=AUTH).json()["preview"]
    assert "circles" in str(said) or "rond" in str(said), said


def test_two_reminders_in_a_fresh_conversation_are_accepted(bridges, fakes):
    # the bench's false positives: earlier writes of the agent (a routine, other conversations) counted with this one's
    bridge = bridges.new().start()
    give_token(bridge)
    assert bridge.put("/v1/reminders/agents", json={"agents": ["Wellness"]}, headers=AUTH).status_code == 200
    assert bridge.put("/v1/reminders", json={"lists": [{"id": "L1", "title": "Maison"}], "items": []},
                      headers=AUTH).status_code == 200
    for i in range(40):   # a routine before (no run): its 40 pages (scheduled work, a task's writes), its cap reached
        assert not call(bridge, "create_page", {"title": f"Brief {i}", "html": f"<h1>{i}</h1>"})["isError"]
    assert call(bridge, "create_page", {"title": "Brief 41", "html": "<h1>41</h1>"})["isError"]
    # the person's new conversation: its turn still going on
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("tool.started", run_id="r1", tool="mcp__bipagents__reminders_add", preview="{}"),
                               gate, sse("run.completed", run_id="r1", output="C'est noté.")]
    start(bridge, fakes, session="api_new")
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


def test_a_stopped_turn_shows_only_the_brakes_line_in_the_timeline(bridges, fakes):
    """Alex 4/11 (release blocker): the rescue turn's input, stored with the time line in front of its mark, showed
    as his own bubble with the agent's English instruction. The timeline serves the brake's plain line only."""
    bridge = bridges.new().start()
    fakes.hermes.runs["run_vie"] = diagnosis("run_vie", 5)
    reply = "Je n'ai pas trouvé l'erreur. Veux-tu que je regarde les journaux du serveur ?"
    fakes.hermes.runs["r1"] = [sse("message.delta", run_id="r1", delta=reply),
                               sse("run.completed", run_id="r1", output=reply)]
    events = follow(bridge, "run_vie")
    line = notices(events)[0]["text"]
    sent = fakes.hermes.started["r1"]["input"]
    stored = sent if sent.startswith("<!--bip:now") else f"<!--bip:now mardi 4 novembre, 10 h 12-->\n{sent}"
    fakes.hermes.add_session("api_c", [
        {"role": "user", "content": "Pourquoi le job de nuit échoue ?"},
        {"role": "assistant", "content": "This turn did not complete. Some actions may already have run; verify "
                                         "their effects before resending."},
        {"role": "user", "content": stored},
        {"role": "user", "content": "[OUT-OF-BAND USER MESSAGE from the user]\n[bipagents:brake] A note from the "
                                    "app, not from your person: change approach\n[/OUT-OF-BAND USER MESSAGE]"},
        {"role": "assistant", "content": reply}], title="Job de nuit")
    thread = bridge.get("/v1/threads/resolve", headers=AUTH, params={"agent": "wellness", "session_id": "api_c"}).json()
    page = bridge.get(f"/v1/threads/{thread['thread']['id']}/timeline", headers=AUTH, params={"agent": "wellness"})
    assert page.status_code == 200, page.text
    shown = [(i["kind"], i.get("role"), i.get("text") or i.get("notice", {}).get("text")) for i in page.json()["items"]]
    # the brake's line in the timeline: the head of the stream's (`notice_head`), in the agent's language
    head = shown[1][2] if len(shown) == 3 else None
    assert head and line.startswith(head), (shown, line)
    assert shown == [("message", "person", "Pourquoi le job de nuit échoue ?"), ("notice", None, head),
                     ("message", "agent", reply)], shown
    assert page.json()["items"][1]["notice"]["code"] == "loop_rescue"
    assert "bipagents:" not in page.text and "A note from the app" not in page.text and "<!--" not in page.text


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


def test_the_rescue_turn_of_a_page_it_couldnt_read_gets_no_page_from_the_browser(bridges, fakes):
    """Token lab of 2026-11-07 (GLM-5.3 Flash, the French Wikipedia page of the Eiffel tower, too large to come back
    whole): stopped after 16 reads, its rescue turn read the page again and waited 120 s. While the rescue turn goes on,
    the agent's browser folder holds its deadline (`rescue`, read by bip-browser-mcp: page reads refused at once);
    gone at its end."""
    bridge = bridges.new()
    root = bridge.data / "browsers"
    (root / "wellness").mkdir(parents=True)
    bridge.config.setdefault("browser", {})["root"] = str(root)
    bridge.start()
    fakes.hermes.runs["run_page"] = (
        [sse("tool.started", run_id="run_page", tool="mcp_stealth_get_page_content", preview="") for _ in range(16)]
        + [sse("run.cancelled", run_id="run_page")])
    gate = threading.Event()
    reply = "Je n'ai pas réussi à lire la page : elle est trop grande pour mon navigateur. Veux-tu que je cherche ailleurs ?"
    fakes.hermes.runs["r1"] = [gate, sse("message.delta", run_id="r1", delta=reply),
                               sse("run.completed", run_id="r1", output=reply)]
    got = []
    follower = threading.Thread(target=lambda: got.append(follow(bridge, "run_page")), daemon=True)
    follower.start()
    rescue = root / "wellness" / "rescue"
    try:
        assert wait_until(lambda: rescue.exists(), timeout=30)
        deadline = float(rescue.read_text())
        assert time.time() + 120 < deadline < time.time() + 200, "the rescue's 180 s and a margin"
    finally:
        gate.set()
    follower.join(60)
    said = notices(got[0])
    assert [n["code"] for n in said] == ["loop_stopped", "loop_rescue_started"]
    assert said[0]["reason"] == "the browser read the page too many times"
    assert "Don't use the browser again in this turn." in fakes.hermes.started["r1"]["input"]
    assert got[0][-1][0] == "run.completed"
    assert wait_until(lambda: not rescue.exists())
    assert bridge.logged("rescue turn: its browser reads no page")


def test_a_rescue_turn_leaves_the_browser_to_another_run_of_the_agent(bridges, fakes):
    # a background task of the same agent browsing meanwhile: its page reads are not refused
    bridge = bridges.new()
    root = bridge.data / "browsers"
    (root / "wellness").mkdir(parents=True)
    bridge.config.setdefault("browser", {})["root"] = str(root)
    bridge.start()
    task = threading.Event()
    fakes.hermes.runs["run_task"] = [sse("tool.started", run_id="run_task", tool="mcp_stealth_navigate",
                                         preview="https://fr.wikipedia.org/"), task,
                                     sse("run.completed", run_id="run_task", output="ok")]
    task_events = []
    tasks = threading.Thread(target=lambda: task_events.append(follow(bridge, "run_task")), daemon=True)
    tasks.start()
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/run_task/events", "GET"))
    fakes.hermes.runs["run_vie"] = diagnosis("run_vie", 5)
    reply = "Je n'ai pas trouvé l'erreur."
    fakes.hermes.runs["r1"] = [sse("message.delta", run_id="r1", delta=reply),
                               sse("run.completed", run_id="r1", output=reply)]
    try:
        events = follow(bridge, "run_vie")
        assert not (root / "wellness" / "rescue").exists()
    finally:
        task.set()
        tasks.join(30)
    assert [n["code"] for n in notices(events)] == ["loop_stopped", "loop_rescue_started"]
    assert task_events and task_events[0][-1][0] == "run.completed", "the other run goes on to its own end"
    assert not bridge.logged("rescue turn: its browser reads no page")


def test_a_turn_nobody_follows_gets_no_rescue_turn(bridges, fakes):
    # a run nobody follows (another agent's message, relayed by peers.rs): stopped as before, no rescue turn
    bridge = bridges.new()
    hub = managed_install(bridge, fakes, agents=("budget", "maison"))
    for name in ("budget", "maison"):
        profile(hub, name)
    bridge.start()
    links = [{"from": "budget", "to": "maison"}]
    assert bridge.put("/v1/agent-links", headers=AUTH, json={"links": links}).status_code == 200
    peer = {"Authorization": f"Bearer {(hub / '.env').read_text().split('HERMES_PEER_BIP_KEY=')[1].strip()}"}
    opened = bridge.post("/peer/p/maison/api/sessions", headers=peer, json={"title": "Bot Chat", "source": "bot_peer_dm"})
    assert opened.status_code == 201, opened.text
    fakes.hermes.runs["r1"] = diagnosis("r1", 5)
    message = {"input": "Message from Budget (@budget): tu as vu l'erreur de cette nuit ?",
               "session_id": opened.json()["session"]["id"]}
    sent = bridge.post("/peer/p/maison/v1/runs", headers=peer, json=message)
    assert sent.status_code == 202, sent.text
    assert wait_until(lambda: fakes.hermes.seen("/p/maison/v1/runs/r1/stop", "POST"))
    assert wait_until(lambda: bridge.logged("going round in circles: run stopped"))
    assert list(fakes.hermes.started) == ["r1"], "no rescue turn started"


def test_the_agent_is_told_before_the_stop_and_a_turn_that_changes_approach_goes_on(bridges, fakes):
    # the same search 3 times (stopped at 5): a steer tells the agent, which then tries other ways and answers
    bridge = bridges.new()
    bridge.config["jobs"] = {"handoff": False}   # no long-work steer here
    bridge.start()
    script = [sse("tool.started", run_id="run_vie", tool=SEARCH, preview=NIGHT) for _ in range(3)]
    script += [sse("tool.started", run_id="run_vie", tool=t, preview=f"piste {i}")
               for i, t in enumerate(["read_file", "search_files", "web_search", SEARCH, "terminal"] * 3)]
    script += [sse("run.completed", run_id="run_vie", output="Le job échoue : le mot de passe IMAP a changé.")]
    fakes.hermes.runs["run_vie"] = script
    events = follow(bridge, "run_vie")
    assert notices(events) == [], "never stopped"
    assert events[-1][1]["output"].startswith("Le job échoue")
    assert not fakes.hermes.seen("/v1/runs/run_vie/stop", "POST")
    assert wait_until(lambda: fakes.hermes.steers)
    warned = [s for s in fakes.hermes.steers if s["run_id"] == "run_vie"]
    assert len(warned) == 1
    text = warned[0]["body"]["input"]
    assert text.startswith("[bipagents:brake] ") and "same session_search call 3 times" in text and "at 5" in text
    assert bridge.logged("brake_warned: the agent told it is going round in circles")


# -- 2026-10-28, aibox: a routine made today's podcast and its reply carried yesterday's path, written from memory.
# The bridge hands over the files it made in a turn whatever the model wrote (bridge-rs/src/made_files.rs).

def test_the_audio_made_in_a_turn_is_handed_over_by_its_reply(bridges, fakes):
    bridge = bridges.new().start()
    give_token(bridge)
    gate = threading.Event()
    stale = "/bipagents/audio/wellness/20261027-095308-podcast-du-mardi.m4a"
    reply = f"🎙️ Ton podcast du jour est prêt.\nMEDIA:{stale}"
    fakes.hermes.runs["r1"] = [sse("tool.started", run_id="r1", tool="mcp_bipagents_create_audio", preview="{}"),
                               gate, sse("message.delta", run_id="r1", delta=reply),
                               sse("run.completed", run_id="r1", output=reply)]
    start(bridge, fakes)
    assert wait_until(lambda: fakes.hermes.event_requests)
    made = call(bridge, "create_audio", {"text": "Bonjour, voici le podcast du jour.", "title": "Podcast du jour"})
    assert not made["isError"], text_of(made)
    path = media_of(made)
    assert text_of(made).startswith(f"MEDIA:{path}\n"), "the MEDIA line first"
    gate.set()
    events = follow_run(bridge, "r1")
    end = events[-1][1]
    assert events[-1][0] == "run.completed"
    # today's file the end's attachment, the stale line marked, the model's words untouched (2026-11-03) but its MEDIA
    # lines, never passed to the apps (4/11)
    words = "🎙️ Ton podcast du jour est prêt."
    assert end["output"] == words and [a["path"] for a in end["attachments"]] == [path] and end["stale"] == [stale]
    said = "".join(d.get("delta", "") for n, d in events if n == "message.delta")
    assert said == words + "\n"
    assert bridge.logged("files of the turn handed over as attachments")
    # Hermes keeps the reply as the model wrote it: the timeline still shows today's file and not yesterday's, the
    # stored message's attachments (structured, 2026-11-03), at every read
    fakes.hermes.add_session("api_c", [{"role": "user", "content": "C'est quand l'anniversaire de Julie ?"},
                                       {"role": "assistant", "content": reply}], title="Julie")
    thread = bridge.get("/v1/threads/resolve", headers=AUTH, params={"agent": "wellness", "session_id": "api_c"}).json()

    def files():
        page = bridge.get(f"/v1/threads/{thread['thread']['id']}/timeline", headers=AUTH,
                          params={"agent": "wellness"}).json()
        said = [i for i in page["items"] if i["kind"] == "message" and i["role"] == "agent"]
        return [a["path"] for a in said[-1].get("attachments", [])] if said else []

    assert wait_until(lambda: files() == [path], timeout=20), files()


def test_the_files_handed_over_are_on_the_timeline_when_the_end_arrives(bridges, fakes):
    """4/11 release check: the apps read the timeline as soon as a run ends, and a catch-up (`after=`) never gives an
    item again. The files a turn made, kept on its stored reply, are there before the end is passed on: a reply read
    then would stay without them on that phone."""
    bridge = bridges.new().start()
    give_token(bridge)
    gate = threading.Event()
    reply = "Voilà ta page."   # the model names no file: only the bridge's attachments show it
    fakes.hermes.runs["r1"] = [sse("tool.started", run_id="r1", tool="mcp_bipagents_create_page", preview="{}"),
                               gate, sse("message.delta", run_id="r1", delta=reply),
                               sse("run.completed", run_id="r1", output=reply)]
    fakes.hermes.add_session("api_c", [{"role": "user", "content": "C'est quand l'anniversaire de Julie ?"},
                                       {"role": "assistant", "content": reply}], title="Julie")
    start(bridge, fakes)
    assert wait_until(lambda: fakes.hermes.event_requests)
    made = call(bridge, "create_page", {"title": "Julie", "html": "<h1>Julie</h1>"})
    assert not made["isError"], text_of(made)
    path = media_of(made)
    thread = bridge.get("/v1/threads/resolve", headers=AUTH, params={"agent": "wellness", "session_id": "api_c"}).json()

    def files():
        page = bridge.get(f"/v1/threads/{thread['thread']['id']}/timeline", headers=AUTH,
                          params={"agent": "wellness"}).json()
        said = [i for i in page["items"] if i["kind"] == "message" and i["role"] == "agent"]
        return [a["path"] for a in said[-1].get("attachments", [])] if said else []

    gate.set()
    seen = None
    with bridge.stream("GET", "/v1/runs/r1/events", params={"agent": "wellness"}, headers=AUTH) as resp:
        for line in resp.iter_lines():
            if line == "event: run.completed":
                seen = files()   # read at once, as the apps do
                break
    assert seen == [path], seen


def test_a_reply_that_already_hands_the_file_over_is_left_alone(bridges, fakes):
    bridge = bridges.new().start()
    give_token(bridge)
    gate, said = threading.Event(), {}
    fakes.hermes.runs["r1"] = [sse("tool.started", run_id="r1", tool="mcp_bipagents_create_audio", preview="{}"), gate]
    start(bridge, fakes)
    assert wait_until(lambda: fakes.hermes.event_requests)
    path = media_of(call(bridge, "create_audio", {"text": "Bonjour.", "title": "Bonjour"}))
    said["reply"] = f"Voilà.\nMEDIA:{path}"
    fakes.hermes.runs["r1"] += [sse("message.delta", run_id="r1", delta=said["reply"]),
                                sse("run.completed", run_id="r1", output=said["reply"])]
    gate.set()
    events = follow_run(bridge, "r1")
    # its MEDIA line as the end's attachment (4/11: never as words), the file once
    assert events[-1][1]["output"] == "Voilà." and [a["path"] for a in events[-1][1]["attachments"]] == [path]
    assert all("MEDIA:" not in json.dumps(data) for _, data in events)
    # and a turn that made nothing: an older audio the person asked for again stays
    old = "/bipagents/audio/wellness/20261020-080000-ancien.m4a"
    fakes.hermes.runs["r2"] = [sse("message.delta", run_id="r2", delta=f"Le revoici.\nMEDIA:{old}"),
                               sse("run.completed", run_id="r2", output=f"Le revoici.\nMEDIA:{old}")]
    assert start(bridge, fakes) == "r2"
    events = follow_run(bridge, "r2")
    assert events[-1][1]["output"] == "Le revoici." and [a["path"] for a in events[-1][1]["attachments"]] == [old]
    assert "stale" not in events[-1][1]
