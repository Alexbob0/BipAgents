"""The cron watcher end to end (cronwatch.py): finished scheduled-task runs read from Hermes' sessions into the outbox
and pushed (a muted job stored silently), Bot Chat turns taken alone pushed as « reply ready », and the conversation
state route that tells the bridge what is on screen. The watcher starts 10 s after the bridge."""
import time

import pytest

from bbkit.common import AUTH
from bbkit.servers import wait_until

TOKEN = "c" * 64
NOW = "2026-10-21T08:00:00.000Z"


def stamp(seconds_ago: float) -> str:
    return time.strftime("%Y%m%d_%H%M%S", time.gmtime(time.time() - seconds_ago))   # the bridge runs with TZ=UTC


def watching(bridges):
    bridge = bridges.new(change=lambda c: c.update(cron={"watch": True, "interval_seconds": 0.3, "window_hours": 12}))
    bridge.seed_db([
        ("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
         (TOKEN, "sandbox", '["wellness"]', NOW, NOW)),
        ("INSERT INTO cron_jobs (agent, job, name, notify, last_seen) VALUES (?,?,?,?,?)",
         ("wellness", "0681ac73805e", "Tri des e-mails", 0, NOW)),
    ])
    return bridge


@pytest.mark.slow
def test_finished_cron_replies_go_to_the_outbox_once_and_muted_ones_silently(bridges, fakes):
    bridge = watching(bridges)
    hermes = fakes.hermes
    podcast, muted = f"cron_fc344ed09026_{stamp(3600)}", f"cron_0681ac73805e_{stamp(1800)}"
    hermes.add_session(podcast, [{"role": "user", "content": "Produis le point"},
                                 {"role": "assistant", "content": [{"type": "text", "text": "Bonjour Alex !"}]}],
                       title="Podcast du matin · Oct 21 07:00")
    hermes.add_session(muted, [{"role": "user", "content": "Trie"}, {"role": "assistant", "content": "Tri des e-mails fait."}],
                       title=None)
    hermes.add_session(f"cron_aaaa_{stamp(600)}", [{"role": "assistant", "content": "[SILENT]"}])
    hermes.add_session(f"cron_bbbb_{stamp(3 * 86400)}", [{"role": "assistant", "content": "Vieux message"}])
    hermes.add_session(f"cron_cccc_{stamp(60)}", [{"role": "user", "content": "Produis le point"}])   # still running
    hermes.add_session(f"cron_dddd_{stamp(60)}", [{"role": "assistant", "content": "je passe par terminal…",
                                                   "tool_calls": [{"id": "t1"}]}])
    bridge.start()
    assert wait_until(lambda: len(bridge.get("/v1/outbox", headers=AUTH).json()["items"]) == 2, timeout=25)
    time.sleep(1.0)   # more polls: nothing else, nothing twice
    items = bridge.get("/v1/outbox", headers=AUTH).json()["items"]
    by_text = {i["text"]: i for i in items}
    assert set(by_text) == {"Bonjour Alex !", "Tri des e-mails fait."}
    assert by_text["Bonjour Alex !"]["title"] == "Podcast du matin · Oct 21 07:00"
    assert by_text["Bonjour Alex !"]["session_id"] == podcast
    assert by_text["Tri des e-mails fait."]["title"] == "Tri des e-mails"     # the job's name kept by the bridge
    with bridge.db() as db:
        ids = sorted(row[0] for row in db.execute("SELECT ntfy_id FROM outbox"))
        jobs = dict(db.execute("SELECT job, name FROM cron_jobs").fetchall())
    assert ids == sorted([f"hermes-session:{podcast}", f"hermes-session:{muted}"])
    assert jobs == {"fc344ed09026": "Podcast du matin", "0681ac73805e": "Tri des e-mails"}
    assert wait_until(lambda: len(fakes.apns.pushes()) >= 1)
    time.sleep(2.5)   # the muted job's message waited for its audio as long as the pushed one did
    pushed = [p for p in fakes.apns.payloads() if p["aps"].get("category") == "MESSAGE"]
    assert [p["outbox_id"] for p in pushed] == [by_text["Bonjour Alex !"]["id"]]
    listed = bridge.get("/v1/cron-jobs", headers=AUTH).json()["jobs"]
    assert {(j["job"], j["notify"]) for j in listed} == {("fc344ed09026", True), ("0681ac73805e", False)}


@pytest.mark.slow
def test_a_bot_chat_turn_taken_alone_is_pushed_once_unless_on_screen(bridges, fakes):
    bridge = watching(bridges)
    hermes = fakes.hermes
    hermes.add_session("api_bot", [{"role": "user", "content": "Bonjour"}, {"role": "assistant", "content": "Ancienne réponse"}],
                       title="Bot Chat")
    bridge.start()
    fetched = lambda: len(hermes.seen("/api/sessions/api_bot/messages"))   # noqa: E731
    assert wait_until(lambda: fetched() >= 1, timeout=20)                  # baseline: nothing old is pushed
    hermes.messages[("", "api_bot")] += [{"role": "user", "content": "[Routine] Point de midi"},
                                         {"role": "assistant", "content": "Wellness dit que ta nuit était bonne."}]
    hermes.sessions[""]["api_bot"]["message_count"] = 4
    assert wait_until(lambda: len(fakes.apns.pushes()) >= 1, timeout=10)
    payload = fakes.apns.payloads()[0]
    assert payload["kind"] == "reply" and payload["session_id"] == "api_bot" and payload["run_id"] == "session:api_bot"
    assert bridge.get(f"/v1/replies/{payload['reply_id']}", headers=AUTH).json()["text"] == \
        "Wellness dit que ta nuit était bonne."
    # The conversation open on the phone (it polls its state): the next turn is not pushed.
    state = bridge.get("/v1/sessions/api_bot/state?agent=wellness", headers=AUTH)
    assert state.status_code == 200
    assert state.json() == {"session_id": "api_bot", "message_count": 4, "updated_at": 0, "outbox": "0@"}  # Hermes' last_active
    hermes.messages[("", "api_bot")] += [{"role": "user", "content": "Et demain ?"},
                                         {"role": "assistant", "content": "Grand soleil."}]
    hermes.sessions[""]["api_bot"]["message_count"] = 6
    seen = fetched()
    assert wait_until(lambda: fetched() >= seen + 2, timeout=10)
    time.sleep(0.5)
    assert len(fakes.apns.pushes()) == 1


def test_session_state_route(bridges, fakes):
    bridge = bridges.new(change=lambda c: c["agents"].update(Panne={"hermes_url": fakes.down.url, "hermes_key": "p" * 32}))
    bridge.start()
    resp = bridge.get("/v1/sessions/api_bot/state?agent=wellness", headers=AUTH)
    assert resp.status_code == 200 and resp.json() == {"session_id": "api_bot", "message_count": 7, "updated_at": None,
                                                       "outbox": "0@"}  # the agent's Boîte: none yet
    fakes.hermes.add_session("api_9", [{"role": "user", "content": "x"}], updated_at="2026-10-21T07:00:00Z")
    assert bridge.get("/v1/sessions/api_9/state?agent=wellness", headers=AUTH).json() == {
        "session_id": "api_9", "message_count": 1, "updated_at": "2026-10-21T07:00:00Z", "outbox": "0@"}
    bad = bridge.get("/v1/sessions/bad%20id/state?agent=wellness", headers=AUTH)
    assert bad.status_code == 400 and bad.json() == {"detail": "invalid session_id"}
    assert bridge.get("/v1/sessions/api_bot/state?agent=ghost", headers=AUTH).status_code == 404
    assert bridge.get("/v1/sessions/api_bot/state", headers=AUTH).status_code == 422
    down = bridge.get("/v1/sessions/api_bot/state?agent=panne", headers=AUTH)
    assert down.status_code == 502 and down.json() == {"detail": "your agent isn't answering right now: try again in a moment"}
    fakes.hermes.keys[""] = "another-key"
    refused = bridge.get("/v1/sessions/api_bot/state?agent=wellness", headers=AUTH)
    assert refused.status_code == 401 and refused.json() == {"detail": "Hermes refused"}

    def broken(seen):
        from starlette.responses import JSONResponse
        return JSONResponse({"error": "boom"}, status_code=500) if seen.path.startswith("/api/sessions/") else None

    fakes.hermes.keys[""] = "hermes-wellness-key-xyz"
    fakes.hermes.override = broken
    failing = bridge.get("/v1/sessions/api_bot/state?agent=wellness", headers=AUTH)
    assert failing.status_code == 502 and failing.json() == {"detail": "Hermes refused"}


@pytest.mark.slow
def test_a_subtask_report_left_without_a_reply_is_pushed_once(bridges, fakes):
    """A background delegate_task's report, stored by Hermes as the conversation's last (user) turn without waking the
    agent: pushed once it is still last a poll later, with the file it names as its line (`[push] previews`), when the
    agent does not write about it itself (`[cron] subtask_handback = false`)."""
    bridge = bridges.new(change=lambda c: c.update(cron={"watch": True, "interval_seconds": 0.3, "window_hours": 12,
                                                         "subtask_handback": False},
                                                   push={"previews": True}))
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (TOKEN, "sandbox", '["wellness"]', NOW, NOW))])
    hermes = fakes.hermes
    hermes.add_session("api_7", [{"role": "user", "content": "Fais le podcast"}, {"role": "assistant", "content": "Je lance."}],
                       title="Podcast")
    bridge.start()
    assert wait_until(lambda: hermes.seen("/api/sessions", "GET"), timeout=20)    # first sight: what is there is old
    time.sleep(0.5)
    hermes.messages[("", "api_7")].append({"role": "user", "content": "[ASYNC DELEGATION RESULT] Done: "
                                           "/home/hermes/.hermes/media/podcast_jeudi.mp3 is ready."})
    hermes.sessions[""]["api_7"]["message_count"] = 3
    assert wait_until(lambda: fakes.apns.pushes(), timeout=10)
    time.sleep(1.0)
    assert len(fakes.apns.pushes()) == 1
    payload = fakes.apns.payloads()[0]
    assert payload["aps"]["alert"] == {"title": "Wellness", "body": "🎧 Podcast jeudi"}
    assert payload["run_id"] == "session:api_7" and payload["session_id"] == "api_7" and payload["kind"] == "reply"
    assert fakes.apns.pushes()[0].header("apns-collapse-id") == "subtask:api_7"
    assert bridge.get(f"/v1/replies/{payload['reply_id']}", headers=AUTH).json()["text"] == "🎧 Podcast jeudi"


@pytest.mark.slow
def test_a_subtask_report_left_without_a_reply_is_told_by_the_agent(bridges, fakes):
    """2026-10-25: the same report (still last a poll later, nobody woke the agent): the agent takes one short turn in
    that conversation, the note hidden from the person, and writes the message itself (the file the sub-agent made
    linked); it is in the Boîte, tagged with the conversation, and pushed once."""
    bridge = bridges.new(change=lambda c: c.update(cron={"watch": True, "interval_seconds": 0.3, "window_hours": 12}))
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (TOKEN, "sandbox", '["wellness"]', NOW, NOW))])
    hermes = fakes.hermes
    hermes.rounds = 1
    hermes.run_output = "Ton podcast de jeudi est prêt :\n- 12 minutes\n- deux voix\nMEDIA:/home/hermes/.hermes/media/podcast_jeudi.mp3"
    hermes.add_session("api_7", [{"role": "user", "content": "Fais le podcast"}, {"role": "assistant", "content": "Je lance."}],
                       title="Podcast")
    bridge.start()
    assert wait_until(lambda: hermes.seen("/api/sessions", "GET"), timeout=20)
    time.sleep(0.5)
    hermes.messages[("", "api_7")].append({"role": "user", "content": "[ASYNC DELEGATION RESULT] Done: "
                                           "/home/hermes/.hermes/media/podcast_jeudi.mp3 is ready."})
    hermes.sessions[""]["api_7"]["message_count"] = 3
    assert wait_until(lambda: "r1" in hermes.started, timeout=10)
    turn = hermes.started["r1"]
    assert turn["session_id"] == "api_7" and turn["input"].startswith("<!--bip:subtask\n")
    assert turn["input"].endswith("MEDIA:/home/hermes/.hermes/media/podcast_jeudi.mp3\n-->")

    def items():
        return bridge.get("/v1/outbox", headers=AUTH).json()["items"]

    assert wait_until(items, timeout=15)
    item = items()[0]
    assert item["session_id"] == "api_7" and item["text"] == "Ton podcast de jeudi est prêt :\n- 12 minutes\n- deux voix"
    assert wait_until(lambda: fakes.apns.pushes())
    time.sleep(1.5)
    payloads = fakes.apns.payloads()
    assert len(payloads) == 1 and payloads[0]["outbox_id"] == item["id"] and payloads[0]["session_id"] == "api_7"
    assert len(hermes.started) == 1   # once
