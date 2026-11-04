"""The app's SSE through the bridge, push decisions for watched runs, /v1/approve
forwarding, and watched runs resumed after a restart (tests/test_voice_and_approvals.py, over HTTP)."""
import threading
import time

from bbkit.common import answered, AUTH, HERMES_KEY, read_sse, sse, in_session, pushed_text
from bbkit.servers import wait_until

TOKEN = "e" * 64


def approval_event(run_id, request_id):
    return sse("approval.request", run_id=run_id, request_id=request_id, command="rm -rf build/",
               choices=["once", "session", "deny"])


def register(bridge, token=TOKEN):
    assert bridge.post("/v1/devices", headers=AUTH, json={"token": token, "environment": "sandbox",
                                                           "agent_ids": ["wellness"]}).status_code == 200


def test_watch_pushes_approval_when_nobody_follows(bridge, fakes):
    register(bridge)
    gate = threading.Event()
    fakes.hermes.runs["run_w"] = [
        sse("message.delta", run_id="run_w", delta="Je supprime le dossier."),
        approval_event("run_w", "req_1"),
        approval_event("run_w", "req_1"),  # duplicate must not push twice
        gate,
        sse("run.completed", run_id="run_w"),
    ]
    resp = bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_w"})
    assert resp.status_code == 200
    assert resp.json() == {"watching": True, "agent": "wellness", "run_id": "run_w", "followers": 0, "finished": False}
    apns = fakes.apns
    assert wait_until(lambda: len(apns.pushes()) >= 1)
    payload = apns.payloads()[0]
    assert payload["aps"]["category"] == "APPROVAL" and payload["aps"]["alert"]["body"] == "notif.approval"
    assert payload.pop("n").startswith("n_")   # the journal's row; no conversation without a session
    assert payload == {"aps": {"alert": {"loc-key": "notif.approval", "body": "notif.approval"},
                               "thread-id": "wellness", "mutable-content": 1, "category": "APPROVAL", "sound": "default",
                               "badge": 1},
                       "v": 2, "t": "approval", "agent": "wellness", "r": "req_1", "run_id": "run_w",
                       "request_id": "req_1", "choices": ["once", "session", "deny"]}
    first = apns.pushes()[0]
    assert first.header("apns-collapse-id") == "req_1" and first.header("apns-push-type") == "alert"
    assert first.header("apns-priority") == "10" and first.header("apns-topic") == "io.github.bipagents"
    assert apns.environment(first) == "sandbox"

    pending = bridge.get("/v1/approvals", headers=AUTH).json()["items"]
    assert len(pending) == 1 and pending[0]["command"] == "rm -rf build/" and pending[0]["request_id"] == "req_1"
    # and the bridge's plain words (approval_text.rs): « Wellness veut supprimer : rm -rf build/ »
    assert set(pending[0]) == {"agent", "run_id", "request_id", "command", "description", "choices", "created_at",
                               "session_id", "bip_summary", "bip_detail"}
    assert pending[0]["bip_summary"] == "Wellness wants to delete the folder “build” and everything in it"
    assert "rm -rf build/" in pending[0]["bip_detail"]
    assert pending[0]["created_at"].endswith("Z") and pending[0]["choices"] == ["once", "session", "deny"]
    assert bridge.get("/v1/approvals?agent=wellness", headers=AUTH).json()["items"] == pending
    assert bridge.get("/v1/approvals?agent=ghost", headers=AUTH).status_code == 404

    gate.set()
    # Run finished while watched and unfollowed -> silent push for resync; approval cleared.
    assert wait_until(lambda: len(apns.pushes()) >= 2)
    silent = apns.payloads()[1]
    assert silent == {"aps": {"content-available": 1}, "agent": "wellness", "reason": "run_finished",
                      "run_id": "run_w", "status": "run.completed"}
    assert apns.pushes()[1].header("apns-push-type") == "background"
    assert apns.pushes()[1].header("apns-priority") == "5"
    assert bridge.get("/v1/approvals", headers=AUTH).json()["items"] == []
    assert len([p for p in apns.payloads() if p["aps"].get("category") == "APPROVAL"]) == 1


def test_watch_validates(bridge):
    assert bridge.post("/v1/watch", headers=AUTH, json={"agent": "ghost", "run_id": "r"}).status_code == 404
    bad = bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "../x"})
    assert bad.status_code == 400 and bad.json() == {"detail": "invalid run_id"}
    assert bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness"}).status_code == 422


def waiting(prompt_id, generation=5):
    """A run's status as Hermes keeps it while an approval prompt waits."""
    return {"status": "waiting_for_approval", "admission_id": "adm_9", "execution_generation": generation,
            "pending_controls": [{"kind": "approval", "prompt_id": prompt_id, "execution_generation": generation,
                                  "command": "ls", "choices": ["once", "deny"]}]}


def test_approve_forwards_to_hermes(bridge, fakes):
    """An approval goes as exactly the prompt its run's status shows waiting: {request_id, execution_generation,
    choice}."""
    hermes = fakes.hermes
    hermes.run_info["run_9"] = waiting("req_9")
    resp = bridge.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": "run_9",
                                                           "request_id": "req_9", "choice": "Once"})
    assert resp.status_code == 200 and resp.json() == {"object": "hermes.run.approval_response", "resolved": 1}
    assert [(a["run_id"], a["body"]) for a in hermes.approvals] == [
        ("run_9", {"request_id": "req_9", "execution_generation": 5, "choice": "once"})]
    assert hermes.seen("/v1/runs/run_9/approval")[0].header("authorization") == f"Bearer {HERMES_KEY}"
    hermes.approval_response = (409, {"error": {"code": "stale_generation"}})
    resp = bridge.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": "run_9", "choice": "deny"})
    assert resp.status_code == 409 and resp.json() == {"error": {"code": "stale_generation"}}
    assert hermes.approvals[-1]["body"] == {"request_id": "req_9", "execution_generation": 5, "choice": "deny"}
    # no such run: Hermes' 404, nothing sent
    assert bridge.post("/v1/approve", headers=AUTH,
                       json={"agent": "wellness", "run_id": "run_gone", "choice": "once"}).status_code == 404
    assert len(hermes.approvals) == 2
    assert bridge.post("/v1/approve", headers=AUTH,
                       json={"agent": "wellness", "run_id": "../x", "choice": "once"}).status_code == 400
    assert bridge.post("/v1/approve", headers=AUTH,
                       json={"agent": "ghost", "run_id": "r", "choice": "once"}).status_code == 404
    assert bridge.post("/v1/approve", headers=AUTH,
                       json={"agent": "wellness", "run_id": "r", "choice": "x" * 33}).status_code == 422


def test_a_runs_prompts_reach_the_app_with_their_request_id(bridge, fakes):
    """Hermes' prompts (`prompt_id`, `execution_generation`) reach the app with the `request_id` it reads, and
    `approval.settled` ends the approval waiting in the bridge."""
    fakes.hermes.runs["run_c"] = [
        sse("approval.request", run_id="run_c", kind="approval", prompt_id="p3", execution_generation=2,
            command="rm -rf build/", choices=["once", "deny"]),
        sse("approval.settled", run_id="run_c", prompt_id="p3", execution_generation=2),
        sse("clarify.request", run_id="run_c", kind="clarify", prompt_id="c4", execution_generation=2,
            question="Quel train ?", choices=["9h", "14h"], multi_select=False),
        sse("clarify.settled", run_id="run_c", prompt_id="c4", execution_generation=2),
        sse("run.completed", run_id="run_c", output="Fini"),
    ]
    with bridge.stream("GET", "/v1/runs/run_c/events?agent=wellness", headers=AUTH) as resp:
        events = read_sse(resp)
    assert [name for name, _ in events] == ["approval.request", "approval.settled", "clarify.request",
                                            "clarify.settled", "run.completed"]
    assert [data.get("request_id") for _, data in events[:4]] == ["p3", "p3", "c4", "c4"]
    assert events[0][1]["execution_generation"] == 2 and events[2][1]["question"] == "Quel train ?"
    assert bridge.get("/v1/approvals", headers=AUTH).json()["items"] == []


def test_answered_approvals_stay_in_the_conversation(bridge, fakes):
    """Hermes' history has no approval rows (owner 2026-10-22): the bridge keeps each decision it relays (every answer
    goes through it, `POST /v1/approve`), for the timeline to show it answered; no phone reports or keeps any."""
    fakes.hermes.run_info["run_9"] = waiting("req_9")
    assert bridge.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": "run_9", "request_id": "req_9",
                                                          "choice": "Once"}).status_code == 200
    assert [(r["run_id"], r["choice"], r["via"]) for r in answered(bridge, "wellness")] == [("run_9", "once", "notification")]
    assert bridge.post("/v1/approvals/answered", headers=AUTH, json={"agent": "wellness", "run_id": "r", "choice": "once"}
                       ).status_code in (404, 405)


def test_approve_with_hermes_down(bridges, fakes):
    bridge = bridges.new(change=lambda c: c["agents"]["Wellness"].update(hermes_url=fakes.down.url)).start()
    resp = bridge.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": "r", "choice": "once"})
    assert resp.status_code == 502 and resp.json() == {"detail": "your agent isn't answering right now: try again in a moment"}


def test_app_follows_run_through_bridge_and_replays_on_reattach(bridge, fakes):
    fakes.hermes.runs["run_a"] = [
        sse("tool.started", run_id="run_a", tool="web_search"),
        sse("message.delta", run_id="run_a", delta="Bonjour "),
        sse("message.delta", run_id="run_a", delta="Alex."),
        sse("run.completed", run_id="run_a", output="Bonjour Alex."),
    ]
    url = "/v1/runs/run_a/events?agent=wellness&session_id=api_123"
    with bridge.stream("GET", url, headers=AUTH) as resp:
        assert resp.status_code == 200 and resp.headers["content-type"].startswith("text/event-stream")
        assert resp.headers["cache-control"] == "no-store" and resp.headers["x-accel-buffering"] == "no"
        first = read_sse(resp)
    assert [name for name, _ in first] == ["tool.started", "message.delta", "message.delta", "run.completed"]
    assert first[-1][1]["output"] == "Bonjour Alex."
    assert first[1][1] == {"event": "message.delta", "run_id": "run_a", "delta": "Bonjour "}   # Hermes' data as it was
    # Re-attaching (app reopened, connection lost) replays the whole run from the bridge's backlog,
    # consecutive deltas merged.
    with bridge.stream("GET", url, headers=AUTH) as resp:
        replay = read_sse(resp)
    assert [name for name, _ in replay] == ["tool.started", "message.delta", "run.completed"]
    assert replay[1][1]["delta"] == "Bonjour Alex."
    assert len(fakes.hermes.event_requests) == 1  # one Hermes subscription for both


def test_sse_frames_are_written_as_the_bridge_writes_them(bridge, fakes):
    """`event: <type>\\ndata: <Hermes' data, its newlines continued>\\n\\n`; a bare JSON line and a typed payload
    without an SSE event name are tolerated (hermes.SSEParser)."""
    fakes.hermes.runs["run_raw"] = [
        'event: message.delta\ndata: {"delta": "a",\ndata:  "run_id": "run_raw"}\n\n',
        '{"type": "tool.started", "tool": "x"}\n',
        'data: {"type": "run.completed", "output": "fin"}\n\n',
    ]
    with bridge.stream("GET", "/v1/runs/run_raw/events?agent=wellness", headers=AUTH) as resp:
        raw = b"".join(resp.iter_raw()).decode()
    assert raw == ('event: message.delta\ndata: {"delta": "a",\ndata:  "run_id": "run_raw"}\n\n'
                   'event: tool.started\ndata: {"type": "tool.started", "tool": "x"}\n\n'
                   'event: run.completed\ndata: {"type": "run.completed", "output": "fin"}\n\n')


def test_app_route_validates_agent_and_ids(bridge):
    assert bridge.get("/v1/runs/run_a/events?agent=nope", headers=AUTH).status_code == 404
    bad = bridge.get("/v1/runs/run_a/events?agent=wellness&session_id=bad%20id", headers=AUTH)
    assert bad.status_code == 400 and bad.json() == {"detail": "invalid session_id"}
    assert bridge.get("/v1/runs/run_a/events", headers=AUTH).status_code == 422          # agent is required
    assert bridge.get("/v1/runs/a%20b/events?agent=wellness", headers=AUTH).status_code == 400


def test_a_missing_run_ends_the_apps_stream_with_the_bridges_error(bridge, fakes):
    with bridge.stream("GET", "/v1/runs/gone_1/events?agent=wellness", headers=AUTH) as resp:
        events = read_sse(resp)
    assert events == [("bridge.error", {"type": "bridge.error", "code": "run_not_found", "status": 404})]


def follow_then_leave(bridge, run_id, session_id=None):
    """What the app does: follow the run over SSE, then leave the conversation (the connection closes). Waits until
    the bridge saw it go (`POST /v1/watch` tells its followers)."""
    query = f"agent=wellness" + (f"&session_id={session_id}" if session_id else "")
    with bridge.stream("GET", f"/v1/runs/{run_id}/events?{query}", headers=AUTH) as resp:
        assert resp.status_code == 200
        watch = {"agent": "wellness", "run_id": run_id}
        assert wait_until(lambda: bridge.post("/v1/watch", headers=AUTH, json=watch).json()["followers"] == 1)
    assert wait_until(lambda: bridge.post("/v1/watch", headers=AUTH, json=watch).json()["followers"] == 0, timeout=20)


def test_reply_ready_pushed_when_the_app_left(bridge, fakes):
    register(bridge)
    gate = threading.Event()
    fakes.hermes.runs["run_r"] = [sse("message.delta", run_id="run_r", delta="Je cherche."), gate,
                                  sse("run.completed", run_id="run_r", output="Le train de 9h est à 19 €.")]
    follow_then_leave(bridge, "run_r", "api_9")
    gate.set()
    apns = fakes.apns
    assert wait_until(lambda: len(apns.pushes()) >= 1)
    payload = apns.payloads()[0]
    assert payload["t"] == "reply" and in_session(bridge, payload, "api_9") and payload["r"] == "run_r"
    assert payload["aps"]["category"] == "MESSAGE" and payload["aps"]["alert"]["body"] == "notif.reply"
    assert apns.pushes()[0].header("apns-collapse-id") == "run_r"
    # The text stays off Apple: the extension fetches it from the bridge.
    assert "19 €" not in apns.pushes()[0].body.decode()
    reply = bridge.get(f"/v1/notifications/{payload['n']}", headers=AUTH)
    assert reply.status_code == 200 and reply.json()["preview"] == "Le train de 9h est à 19 €."
    assert bridge.get("/v1/notifications/nope", headers=AUTH).status_code == 404
    assert bridge.get(f"/v1/notifications/{payload['n']}").status_code == 401


def test_an_agent_told_not_to_notify_stays_quiet_until_turned_back_on(bridge, fakes):
    """« Me prévenir quand il a fini ou qu'il t'attend » off on its page: its reply waits in the app, silent."""
    register(bridge)
    assert bridge.put("/v1/agents/wellness/notify", headers=AUTH, json={"on": False}).json() == {"id": "wellness", "on": False}
    assert bridge.put("/v1/agents/nope/notify", headers=AUTH, json={"on": False}).status_code == 404
    for run_id, session in (("run_quiet", "api_q1"), ("run_loud", "api_q2")):
        gate = threading.Event()
        fakes.hermes.runs[run_id] = [gate, sse("run.completed", run_id=run_id, output="Fini.")]
        follow_then_leave(bridge, run_id, session)
        gate.set()
        if run_id == "run_quiet":
            # its end is seen by the bridge (the run's replay is over), then the switch goes back on
            with bridge.stream("GET", f"/v1/runs/{run_id}/events?agent=wellness", headers=AUTH) as resp:
                assert read_sse(resp)[-1][0] == "run.completed"
            time.sleep(0.5)
            assert bridge.put("/v1/agents/wellness/notify", headers=AUTH, json={"on": True}).json()["on"] is True
    apns = fakes.apns
    assert wait_until(lambda: len(apns.pushes()) >= 1)
    time.sleep(0.5)
    assert [p["r"] for p in apns.payloads()] == ["run_loud"]


def test_an_approval_is_pushed_even_when_the_agent_is_told_not_to_notify(bridge, fakes):
    """The switch never hides what the agent is stuck on: an approval always notifies (Alex, 2026-11-02)."""
    register(bridge)
    assert bridge.put("/v1/agents/wellness/notify", headers=AUTH, json={"on": False}).json()["on"] is False
    gate = threading.Event()
    fakes.hermes.runs["run_ap"] = [gate, approval_event("run_ap", "req_ap"), threading.Event()]
    follow_then_leave(bridge, "run_ap", "api_ap")
    gate.set()
    assert wait_until(lambda: any(p["aps"].get("category") == "APPROVAL" for p in fakes.apns.payloads()))
    assert [p["request_id"] for p in fakes.apns.payloads()] == ["req_ap"]


def test_replay_backlog_merges_deltas_so_long_replies_keep_their_start(bridge, fakes):
    words = [f"mot{i} " for i in range(5000 + 500)]   # runs.BACKLOG_EVENTS + 500
    fakes.hermes.runs["run_long"] = [sse("message.delta", run_id="run_long", delta="Début : ")] + [
        sse("message.delta", run_id="run_long", delta=w) for w in words] + [
        sse("tool.started", run_id="run_long", tool="web_search"),
        sse("message.delta", run_id="run_long", delta="Fin."),
        sse("run.completed", run_id="run_long", output="…")]
    with bridge.stream("GET", "/v1/runs/run_long/events?agent=wellness", headers=AUTH) as resp:
        live = read_sse(resp)  # first follower: every delta, live
    assert len(live) == len(words) + 4
    with bridge.stream("GET", "/v1/runs/run_long/events?agent=wellness", headers=AUTH) as resp:
        replay = read_sse(resp)  # late follower: the backlog
    assert [name for name, _ in replay] == ["message.delta", "tool.started", "message.delta", "run.completed"]
    assert replay[0][1]["delta"] == "Début : " + "".join(words)


def test_question_pushed_when_the_app_left(bridge, fakes):
    register(bridge)
    gate = threading.Event()
    fakes.hermes.runs["run_q"] = [
        gate,
        sse("clarify.request", run_id="run_q", request_id="clr_1",
            questions=[{"id": "q1", "question": "Quel train ?", "choices": ["9h", "14h"]}]),
        threading.Event(),  # the run waits for the answer
    ]
    follow_then_leave(bridge, "run_q", "api_7")
    gate.set()
    apns = fakes.apns
    assert wait_until(lambda: len(apns.pushes()) >= 1)
    payload = apns.payloads()[0]
    assert payload["t"] == "question" and payload["request_id"] == "clr_1" and in_session(bridge, payload, "api_7")
    assert payload["aps"]["alert"]["body"] == "notif.question" and payload["aps"]["mutable-content"] == 1
    assert pushed_text(bridge, payload) == "Quel train ?"


def test_upstream_question_contract_is_pushed_too(bridge, fakes):
    """Hermes' upstream clarify (one question per `clarify_id`) is pushed like ours."""
    register(bridge)
    gate = threading.Event()
    fakes.hermes.runs["run_u"] = [
        gate,
        sse("clarify.request", run_id="run_u", clarify_id="cq_9", question="Quel train ?", choices=["9h", "14h"],
            multi_select=False),
        threading.Event(),
    ]
    follow_then_leave(bridge, "run_u")
    gate.set()
    assert wait_until(lambda: len(fakes.apns.pushes()) >= 1)
    assert fakes.apns.payloads()[0]["request_id"] == "cq_9"


def test_approval_is_pushed_when_the_app_stops_following(bridge, fakes):
    register(bridge)
    asked, finish = threading.Event(), threading.Event()
    fakes.hermes.runs["run_l"] = [
        sse("message.delta", run_id="run_l", delta="Je génère le podcast. "),
        asked,
        approval_event("run_l", "req_l"),
        finish,
        sse("run.completed", run_id="run_l"),
    ]
    with bridge.stream("GET", "/v1/runs/run_l/events?agent=wellness", headers=AUTH) as resp:
        assert resp.status_code == 200
        watch = {"agent": "wellness", "run_id": "run_l"}
        assert wait_until(lambda: bridge.post("/v1/watch", headers=AUTH, json=watch).json()["followers"] == 1)
        asked.set()
        assert wait_until(lambda: bridge.get("/v1/approvals", headers=AUTH).json()["items"])
        assert fakes.apns.pushes() == []  # on screen: the app shows it
    # Left the conversation with the approval unanswered: it is pushed now, once.
    assert wait_until(lambda: any(p["aps"].get("category") == "APPROVAL" for p in fakes.apns.payloads()))
    approvals = [p for p in fakes.apns.payloads() if p["aps"].get("category") == "APPROVAL"]
    assert len(approvals) == 1 and approvals[0]["request_id"] == "req_l"
    finish.set()


def test_watched_runs_resume_after_a_restart_with_their_pending_approval(bridge, fakes):
    """runs.RunHub.resume, seen from outside: the watched runs are kept in the database (`watched_runs`); after a
    restart the bridge asks Hermes for each run's status, rebuilds the approval it waits for and pushes it."""
    register(bridge)
    for run_id in ("run_wait", "run_done", "run_gone"):
        fakes.hermes.runs[run_id] = [threading.Event()]   # nothing happens while the first bridge watches
    assert bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_wait",
                                                         "session_id": "api_bot"}).status_code == 200
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_done"})
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_gone"})

    def rows():
        with bridge.db() as db:
            return sorted(db.execute("SELECT agent, run_id, session_id FROM watched_runs").fetchall())

    assert wait_until(lambda: len(rows()) == 3)
    assert rows()[2] == ("wellness", "run_wait", "api_bot")
    bridge.stop()
    fakes.hermes.run_info = {
        "run_wait": {"status": "waiting_for_approval", "session_id": "api_bot", "pending_controls": [
            {"kind": "approval", "prompt_id": "req_x", "execution_generation": 1, "command": "python3 -c 'tts health'",
             "choices": ["once", "session", "deny"]}]},
        "run_done": {"status": "completed"},
    }
    bridge.start()
    assert wait_until(lambda: bridge.get("/v1/approvals", headers=AUTH).json()["items"])
    pending = bridge.get("/v1/approvals", headers=AUTH).json()["items"]
    assert [(p["run_id"], p["request_id"], p["session_id"]) for p in pending] == [("run_wait", "req_x", "api_bot")]
    assert wait_until(lambda: len(fakes.apns.pushes()) >= 1)
    payload = fakes.apns.payloads()[0]
    assert payload["aps"]["category"] == "APPROVAL" and in_session(bridge, payload, "api_bot")
    assert wait_until(lambda: [r[1] for r in rows()] == ["run_wait"])   # finished and unknown ones forgotten
    assert bridge.logged("run watch resumed")[0]["status"] == "waiting_for_approval"


