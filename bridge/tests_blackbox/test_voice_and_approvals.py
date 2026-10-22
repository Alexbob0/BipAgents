"""WS /v1/voice follow/cancel, the app's SSE through the bridge, push decisions for watched runs, /v1/approve
forwarding, and watched runs resumed after a restart (tests/test_voice_and_approvals.py, over HTTP)."""
import json
import threading
import time

from bbkit.common import AUTH, HERMES_KEY, pcm_for, read_sse, sse
from bbkit.servers import wait_until
from bbkit.ws import open_ws, receive_frames, recv_json

TOKEN = "e" * 64


def ws(bridge):
    return open_ws(bridge.ws_url("/v1/voice"), AUTH)


def test_voice_follow_happy_path(bridge, fakes):
    hermes = fakes.hermes
    hermes.runs["run_1"] = [
        ": keepalive\n\n",
        sse("message.delta", run_id="run_1", delta="Bonjour à tous. Il fait 3."),
        sse("message.delta", run_id="run_1", delta="5 degrés"),
        sse("tool.started", run_id="run_1", tool="weather", preview="{}"),
        sse("message.delta", run_id="run_1", delta=" dehors ! **Fin**"),
        sse("run.completed", run_id="run_1"),
    ]
    with ws(bridge) as conn:
        conn.send(json.dumps({"type": "follow", "agent": "wellness", "run_id": "run_1"}))
        frames = receive_frames(conn)
    following = frames[0][1]
    assert following == {"type": "following", "run_id": "run_1", "agent": "wellness", "format": "pcm16",
                         "sample_rate": 24000, "channels": 1}
    sentences = ["Bonjour à tous.", "Il fait 3.5 degrés.", "dehors !", "Fin."]
    spoken = ["Bonjour à tous.", "Il fait trois virgule cinq degrés.", "dehors !", "Fin."]   # numbers_fr, as spoken
    body = frames[1:-1]
    assert len(body) == 2 * len(sentences)
    for i, text in enumerate(sentences):
        kind, header = body[2 * i]
        assert kind == "json" and header["type"] == "sentence" and header["seq"] == i and header["text"] == text
        assert header["sample_rate"] == 24000 and header["channels"] == 1 and header["format"] == "pcm16"
        kind, audio = body[2 * i + 1]
        assert kind == "bin" and audio == pcm_for(spoken[i]) and header["bytes"] == len(audio)
    assert frames[-1][1] == {"type": "done", "run_id": "run_1", "reason": "completed"}
    # Hermes called with the agent key; Kyutai with the agent voice, in order.
    assert hermes.event_requests[0].header("authorization") == f"Bearer {HERMES_KEY}"
    assert hermes.event_requests[0].header("accept") == "text/event-stream"
    assert [b["input"] for b in fakes.kyutai.inputs] == spoken
    assert {b["voice"] for b in fakes.kyutai.inputs} == {"4193"}


def test_voice_cancel_stops_everything(bridge, fakes):
    gate = threading.Event()
    fakes.hermes.runs["run_2"] = [
        sse("message.delta", run_id="run_2", delta="Première phrase. "),
        sse("message.delta", run_id="run_2", delta="Deuxième"),
        gate,  # Hermes is still generating...
        sse("message.delta", run_id="run_2", delta=" phrase jamais dite."),
        sse("run.completed", run_id="run_2"),
    ]
    with ws(bridge) as conn:
        conn.send(json.dumps({"type": "follow", "agent": "Wellness", "run_id": "run_2", "voice": "5207"}))
        frames = receive_frames(conn, until_type="sentence")
        assert frames[-1][1]["text"] == "Première phrase."
        assert conn.recv(timeout=10) == pcm_for("Première phrase.")
        conn.send(json.dumps({"type": "cancel"}))
        assert recv_json(conn) == {"type": "done", "run_id": None, "reason": "cancelled"}
        gate.set()
        conn.send(json.dumps({"type": "ping"}))
        assert recv_json(conn) == {"type": "pong"}  # nothing else was queued
    assert [b["input"] for b in fakes.kyutai.inputs] == ["Première phrase."]
    assert fakes.kyutai.inputs[0]["voice"] == "5207"


def test_voice_errors(bridge, fakes):
    fakes.hermes.run_status["missing"] = 404
    with ws(bridge) as conn:
        conn.send(json.dumps({"type": "follow", "agent": "ghost", "run_id": "r"}))
        assert recv_json(conn) == {"type": "error", "code": "unknown_agent", "message": "agent inconnu"}
        conn.send("not json")
        assert recv_json(conn) == {"type": "error", "code": "invalid_json", "message": "JSON attendu"}
        conn.send(json.dumps(["a list"]))
        assert recv_json(conn)["code"] == "invalid_json"
        conn.send(b"\x00binary")
        assert recv_json(conn) == {"type": "error", "code": "binary_not_supported", "message": "JSON attendu"}
        conn.send(json.dumps({"type": "dance"}))
        assert recv_json(conn) == {"type": "error", "code": "unknown_message", "message": "dance"}
        conn.send(json.dumps({"type": "follow", "agent": "wellness", "run_id": "a/b"}))
        assert recv_json(conn) == {"type": "error", "code": "invalid_run_id", "message": "run_id invalide"}
        conn.send(json.dumps({"type": "follow", "agent": "wellness", "run_id": "r", "format": "mp3"}))
        assert recv_json(conn) == {"type": "error", "code": "invalid_format", "message": "mp3"}
        conn.send(json.dumps({"type": "cancel"}))                     # nothing to cancel: no answer
        conn.send(json.dumps({"type": "ping"}))
        assert recv_json(conn) == {"type": "pong"}
        conn.send(json.dumps({"type": "follow", "agent": "wellness", "run_id": "missing"}))
        frames = receive_frames(conn)
        assert frames[0][1]["type"] == "following"
        assert frames[1][1] == {"type": "error", "code": "run_not_found", "message": "flux Hermes indisponible"}
        assert frames[2][1] == {"type": "done", "run_id": "missing", "reason": "error"}


def test_voice_formats_and_refused_runs(bridge, fakes):
    fakes.hermes.run_status["secret"] = 403
    fakes.hermes.runs["run_o"] = [sse("message.delta", delta="Bonjour."), sse("run.failed", error="x")]
    with ws(bridge) as conn:
        conn.send(json.dumps({"type": "follow", "agent": "wellness", "run_id": "run_o", "format": "opus"}))
        frames = receive_frames(conn)
        assert frames[0][1]["format"] == "opus"
        assert frames[1][1]["type"] == "sentence" and frames[1][1]["format"] == "opus"
        assert frames[2] == ("bin", b"ID3" + "Bonjour.".encode())  # the fake's non-wav answer, as it is
        assert frames[-1][1] == {"type": "done", "run_id": "run_o", "reason": "failed"}
        conn.send(json.dumps({"type": "follow", "agent": "wellness", "run_id": "secret"}))
        frames = receive_frames(conn)
        assert frames[1][1]["code"] == "hermes_refused"
    assert fakes.kyutai.inputs[0]["response_format"] == "opus"


def test_voice_resume_from_seq_and_final_text_fallback(bridge, fakes):
    fakes.hermes.runs["run_3"] = [sse("run.completed", run_id="run_3", output="Un. Deux. Trois.")]
    with ws(bridge) as conn:
        conn.send(json.dumps({"type": "follow", "agent": "wellness", "run_id": "run_3", "from_seq": 1}))
        frames = receive_frames(conn)
    texts = [f[1]["text"] for f in frames if f[0] == "json" and f[1]["type"] == "sentence"]
    seqs = [f[1]["seq"] for f in frames if f[0] == "json" and f[1]["type"] == "sentence"]
    assert texts == ["Deux.", "Trois."] and seqs == [1, 2]


def test_voice_tts_failure_is_reported_per_sentence(bridge, fakes):
    fakes.kyutai.status = 500
    fakes.hermes.runs["run_t"] = [sse("message.delta", delta="Une phrase."), sse("run.completed")]
    with ws(bridge) as conn:
        conn.send(json.dumps({"type": "follow", "agent": "wellness", "run_id": "run_t"}))
        frames = receive_frames(conn)
    assert frames[1][1]["type"] == "error" and frames[1][1]["code"] == "tts_failed" and frames[1][1]["seq"] == 0
    assert frames[-1][1]["reason"] == "completed"


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
    assert payload["aps"]["category"] == "APPROVAL" and payload["aps"]["alert"]["body"] == "Approbation requise"
    assert payload == {"aps": {"alert": {"title": "Wellness", "body": "Approbation requise",
                                         "loc-key": "Approbation requise"},
                               "thread-id": "wellness", "mutable-content": 1, "category": "APPROVAL", "sound": "default"},
                       "agent": "wellness", "run_id": "run_w", "request_id": "req_1",
                       "choices": ["once", "session", "deny"]}
    first = apns.pushes()[0]
    assert first.header("apns-collapse-id") == "req_1" and first.header("apns-push-type") == "alert"
    assert first.header("apns-priority") == "10" and first.header("apns-topic") == "io.github.bipagents"
    assert apns.environment(first) == "sandbox"

    pending = bridge.get("/v1/approvals", headers=AUTH).json()["items"]
    assert len(pending) == 1 and pending[0]["command"] == "rm -rf build/" and pending[0]["request_id"] == "req_1"
    assert set(pending[0]) == {"agent", "run_id", "request_id", "command", "description", "choices", "created_at",
                               "session_id"}
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


def test_no_approval_push_while_voice_client_follows(bridge, fakes):
    register(bridge)
    gate = threading.Event()
    fakes.hermes.runs["run_f"] = [
        sse("message.delta", run_id="run_f", delta="Je dois demander. "),
        gate,
        approval_event("run_f", "req_2"),
        sse("run.completed", run_id="run_f"),
    ]
    with ws(bridge) as conn:
        conn.send(json.dumps({"type": "follow", "agent": "wellness", "run_id": "run_f"}))
        receive_frames(conn, until_type="sentence")
        conn.recv(timeout=10)  # audio
        watched = bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_f"}).json()
        assert watched["followers"] == 1
        gate.set()
        frames = receive_frames(conn)
        assert frames[-1][1]["reason"] == "completed"
    time.sleep(0.5)
    assert fakes.apns.pushes() == []  # follower attached: the app shows the approval itself


def test_unwatched_run_never_pushes(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(push={"watch_followed_runs": False})).start()
    register(bridge)
    fakes.hermes.runs["run_u"] = [approval_event("run_u", "req_3"), sse("run.completed", run_id="run_u")]
    with ws(bridge) as conn:
        conn.send(json.dumps({"type": "follow", "agent": "wellness", "run_id": "run_u"}))
        receive_frames(conn)
    time.sleep(0.5)
    assert fakes.apns.pushes() == []


def test_approve_forwards_to_hermes(bridge, fakes):
    hermes = fakes.hermes
    resp = bridge.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": "run_9",
                                                           "request_id": "req_9", "choice": "Once"})
    assert resp.status_code == 200 and resp.json() == {"object": "hermes.run.approval_response", "resolved": 1}
    assert [(a["run_id"], a["body"]) for a in hermes.approvals] == [("run_9", {"choice": "once", "request_id": "req_9"})]
    assert hermes.seen("/v1/runs/run_9/approval")[0].header("authorization") == f"Bearer {HERMES_KEY}"
    hermes.approval_response = (409, {"error": {"code": "approval_not_pending"}})
    resp = bridge.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": "run_9", "choice": "deny"})
    assert resp.status_code == 409 and resp.json() == {"error": {"code": "approval_not_pending"}}
    assert hermes.approvals[-1]["body"] == {"choice": "deny"}
    assert bridge.post("/v1/approve", headers=AUTH,
                       json={"agent": "wellness", "run_id": "../x", "choice": "once"}).status_code == 400
    assert bridge.post("/v1/approve", headers=AUTH,
                       json={"agent": "ghost", "run_id": "r", "choice": "once"}).status_code == 404
    assert bridge.post("/v1/approve", headers=AUTH,
                       json={"agent": "wellness", "run_id": "r", "choice": "x" * 33}).status_code == 422


def test_answered_approvals_stay_in_the_conversation(bridge, fakes):
    """Hermes' history has no approval rows (owner 2026-10-22): the bridge keeps each decision, the one it relayed from
    a notification and the ones an app reports, per conversation, for the apps to put back in the thread."""
    assert bridge.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": "run_9", "request_id": "req_9",
                                                          "choice": "Once"}).status_code == 200
    reported = bridge.post("/v1/approvals/answered", headers=AUTH, json={
        "agent": "wellness", "session_id": "s1", "run_id": "run_10", "request_id": "q1", "command": "curl -X POST x",
        "description": "network", "topic": "sendOut", "choice": "deny", "at": 1_800_000_000.0})
    assert reported.status_code == 200 and reported.json()["kept"]["choice"] == "deny"
    # answered twice (a notification, then the app): one row, the last word
    bridge.post("/v1/approvals/answered", headers=AUTH, json={"agent": "wellness", "session_id": "s1", "run_id": "run_10",
                                                              "request_id": "q1", "choice": "once", "at": 1_800_000_100.0})
    rows = bridge.get("/v1/approvals/answered", params={"agent": "wellness", "session_id": "s1"}, headers=AUTH).json()["items"]
    assert [(r["run_id"], r["choice"], r["topic"], r["via"]) for r in rows] == [("run_10", "once", "sendOut", None)]
    everything = bridge.get("/v1/approvals/answered", params={"agent": "wellness"}, headers=AUTH).json()["items"]
    assert {(r["run_id"], r["choice"], r["via"]) for r in everything} == {("run_9", "once", "notification"),
                                                                           ("run_10", "once", None)}
    assert bridge.post("/v1/approvals/answered", headers=AUTH,
                       json={"agent": "wellness", "run_id": "r", "choice": "maybe"}).status_code == 422
    assert bridge.post("/v1/approvals/answered", headers=AUTH,
                       json={"agent": "ghost", "run_id": "r", "choice": "once"}).status_code == 404
    assert bridge.get("/v1/approvals/answered", params={"agent": "wellness"}).status_code == 401


def test_approve_with_hermes_down(bridges, fakes):
    bridge = bridges.new(change=lambda c: c["agents"]["Wellness"].update(hermes_url=fakes.down.url)).start()
    resp = bridge.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": "r", "choice": "once"})
    assert resp.status_code == 502 and resp.json() == {"detail": "Hermes unreachable"}


def test_voice_reports_unreachable_hermes(bridges, fakes):
    bridge = bridges.new(change=lambda c: c["agents"].update(
        Panne={"hermes_url": fakes.down.url, "hermes_key": "p" * 32})).start()
    with ws(bridge) as conn:
        conn.send(json.dumps({"type": "follow", "agent": "panne", "run_id": "down_1"}))
        frames = receive_frames(conn)
    assert frames[1][1]["code"] == "hermes_unreachable"
    assert frames[-1][1] == {"type": "done", "run_id": "down_1", "reason": "error"}
    time.sleep(1.0)
    assert fakes.down.connections == 3   # three failed connections, then the (unwatched) follow gives up


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
    assert payload["kind"] == "reply" and payload["session_id"] == "api_9" and payload["run_id"] == "run_r"
    assert payload["aps"]["category"] == "MESSAGE" and payload["aps"]["alert"]["body"] == "Ta réponse est prête."
    assert apns.pushes()[0].header("apns-collapse-id") == "run_r"
    # The text stays off Apple: the extension fetches it from the bridge.
    assert "19 €" not in apns.pushes()[0].body.decode()
    reply = bridge.get(f"/v1/replies/{payload['reply_id']}", headers=AUTH)
    assert reply.status_code == 200 and reply.json() == {"id": payload["reply_id"], "text": "Le train de 9h est à 19 €."}
    assert bridge.get("/v1/replies/nope", headers=AUTH).status_code == 404
    assert bridge.get(f"/v1/replies/{payload['reply_id']}").status_code == 401


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
    assert payload["kind"] == "question" and payload["request_id"] == "clr_1" and payload["session_id"] == "api_7"
    assert payload["aps"]["alert"]["body"] == "Une question pour toi" and payload["aps"]["mutable-content"] == 1
    assert bridge.get(f"/v1/replies/{payload['reply_id']}", headers=AUTH).json()["text"] == "Quel train ?"


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
    with ws(bridge) as conn:
        conn.send(json.dumps({"type": "follow", "agent": "wellness", "run_id": "run_l"}))
        receive_frames(conn, until_type="sentence")
        bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_l"})
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
        "run_wait": {"status": "waiting_for_approval", "session_id": "api_bot",
                     "approval": {"request_id": "req_x", "command": "python3 -c 'tts health'",
                                  "choices": ["once", "session", "deny"]}},
        "run_done": {"status": "completed"},
    }
    bridge.start()
    assert wait_until(lambda: bridge.get("/v1/approvals", headers=AUTH).json()["items"])
    pending = bridge.get("/v1/approvals", headers=AUTH).json()["items"]
    assert [(p["run_id"], p["request_id"], p["session_id"]) for p in pending] == [("run_wait", "req_x", "api_bot")]
    assert wait_until(lambda: len(fakes.apns.pushes()) >= 1)
    payload = fakes.apns.payloads()[0]
    assert payload["aps"]["category"] == "APPROVAL" and payload["session_id"] == "api_bot"
    assert wait_until(lambda: [r[1] for r in rows()] == ["run_wait"])   # finished and unknown ones forgotten
    assert bridge.logged("run watch resumed")[0]["status"] == "waiting_for_approval"


def test_a_new_follow_replaces_the_current_one_silently(bridge, fakes):
    """`follow` while another run is followed: the first one stops without its own « done »."""
    fakes.hermes.runs["run_x"] = [sse("message.delta", delta="Première course. "), threading.Event()]
    fakes.hermes.runs["run_y"] = [sse("message.delta", delta="Seconde course."), sse("run.completed")]
    with ws(bridge) as conn:
        conn.send(json.dumps({"type": "follow", "agent": "wellness", "run_id": "run_x"}))
        first = receive_frames(conn, until_type="sentence")
        assert first[0][1]["run_id"] == "run_x" and first[-1][1]["text"] == "Première course."
        assert conn.recv(timeout=10) == pcm_for("Première course.")
        conn.send(json.dumps({"type": "follow", "agent": "wellness", "run_id": "run_y"}))
        frames = receive_frames(conn)
    kinds = [(k, d.get("type"), d.get("run_id")) for k, d in frames if k == "json"]
    assert kinds == [("json", "following", "run_y"), ("json", "sentence", None), ("json", "done", "run_y")]
    assert frames[-1][1] == {"type": "done", "run_id": "run_y", "reason": "completed"}
