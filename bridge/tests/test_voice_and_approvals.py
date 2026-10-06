"""WS /v1/voice follow/cancel, background approval push decision, /v1/approve forwarding."""
import json
import threading

from bipbridge.numbers_fr import prepare_for_synthesis
from conftest import AUTH, HERMES_KEY, pcm_for, sse, wait_until

TOKEN = "e" * 64


def receive_frames(ws, until_type="done", limit=50):
    frames = []
    for _ in range(limit):
        msg = ws.receive()
        if msg.get("bytes") is not None:
            frames.append(("bin", msg["bytes"]))
            continue
        data = json.loads(msg["text"])
        frames.append(("json", data))
        if data.get("type") == until_type:
            break
    return frames


def test_voice_follow_happy_path(client):
    backend = client.backend
    backend.runs["run_1"] = [
        ": keepalive\n\n",
        sse("message.delta", run_id="run_1", delta="Bonjour à tous. Il fait 3."),
        sse("message.delta", run_id="run_1", delta="5 degrés"),
        sse("tool.started", run_id="run_1", tool="weather", preview="{}"),
        sse("message.delta", run_id="run_1", delta=" dehors ! **Fin**"),
        sse("run.completed", run_id="run_1"),
    ]
    with client.websocket_connect("/v1/voice", headers=AUTH) as ws:
        ws.send_text(json.dumps({"type": "follow", "agent": "wellness", "run_id": "run_1"}))
        frames = receive_frames(ws)
    following = frames[0][1]
    assert following == {"type": "following", "run_id": "run_1", "agent": "wellness", "format": "pcm16",
                         "sample_rate": 24000, "channels": 1}
    sentences = ["Bonjour à tous.", "Il fait 3.5 degrés.", "dehors !", "Fin."]
    body = frames[1:-1]
    assert len(body) == 2 * len(sentences)
    for i, text in enumerate(sentences):
        kind, header = body[2 * i]
        assert kind == "json" and header["type"] == "sentence" and header["seq"] == i and header["text"] == text
        assert header["sample_rate"] == 24000 and header["channels"] == 1
        kind, audio = body[2 * i + 1]
        assert kind == "bin" and audio == pcm_for(prepare_for_synthesis(text)) and header["bytes"] == len(audio)
    assert frames[-1][1] == {"type": "done", "run_id": "run_1", "reason": "completed"}
    # Hermes called with the agent key; Kyutai with the agent voice, in order.
    assert backend.run_requests[0].headers["authorization"] == f"Bearer {HERMES_KEY}"
    assert [b["input"] for b in backend.kyutai_inputs] == [prepare_for_synthesis(s) for s in sentences]
    assert {b["voice"] for b in backend.kyutai_inputs} == {"4193"}


def test_voice_cancel_stops_everything(client):
    backend = client.backend
    gate = threading.Event()
    backend.runs["run_2"] = [
        sse("message.delta", run_id="run_2", delta="Première phrase. "),
        sse("message.delta", run_id="run_2", delta="Deuxième"),
        gate,  # Hermes is still generating...
        sse("message.delta", run_id="run_2", delta=" phrase jamais dite."),
        sse("run.completed", run_id="run_2"),
    ]
    with client.websocket_connect("/v1/voice", headers=AUTH) as ws:
        ws.send_text(json.dumps({"type": "follow", "agent": "Wellness", "run_id": "run_2", "voice": "5207"}))
        frames = receive_frames(ws, until_type="sentence")
        assert frames[-1][1]["text"] == "Première phrase."
        assert ws.receive()["bytes"] == pcm_for("Première phrase.")
        ws.send_text(json.dumps({"type": "cancel"}))
        done = json.loads(ws.receive()["text"])
        assert done == {"type": "done", "run_id": None, "reason": "cancelled"}
        gate.set()
        ws.send_text(json.dumps({"type": "ping"}))
        assert json.loads(ws.receive()["text"]) == {"type": "pong"}  # nothing else was queued
    assert [b["input"] for b in backend.kyutai_inputs] == ["Première phrase."]
    assert backend.kyutai_inputs[0]["voice"] == "5207"


def test_voice_errors(client):
    client.backend.run_status["missing"] = 404
    with client.websocket_connect("/v1/voice", headers=AUTH) as ws:
        ws.send_text(json.dumps({"type": "follow", "agent": "ghost", "run_id": "r"}))
        assert json.loads(ws.receive()["text"])["code"] == "unknown_agent"
        ws.send_text("not json")
        assert json.loads(ws.receive()["text"])["code"] == "invalid_json"
        ws.send_text(json.dumps({"type": "follow", "agent": "wellness", "run_id": "missing"}))
        frames = receive_frames(ws)
        assert frames[0][1]["type"] == "following"
        assert frames[1][1] == {"type": "error", "code": "run_not_found", "message": "flux Hermes indisponible"}
        assert frames[2][1]["reason"] == "error"


def test_voice_resume_from_seq_and_final_text_fallback(client):
    client.backend.runs["run_3"] = [sse("run.completed", run_id="run_3", output="Un. Deux. Trois.")]
    with client.websocket_connect("/v1/voice", headers=AUTH) as ws:
        ws.send_text(json.dumps({"type": "follow", "agent": "wellness", "run_id": "run_3", "from_seq": 1}))
        frames = receive_frames(ws)
    texts = [f[1]["text"] for f in frames if f[0] == "json" and f[1]["type"] == "sentence"]
    seqs = [f[1]["seq"] for f in frames if f[0] == "json" and f[1]["type"] == "sentence"]
    assert texts == ["Deux.", "Trois."] and seqs == [1, 2]


def approval_event(run_id, request_id):
    return sse("approval.request", run_id=run_id, request_id=request_id, command="rm -rf build/",
               choices=["once", "session", "deny"])


def register(client):
    assert client.post("/v1/devices", headers=AUTH, json={"token": TOKEN, "environment": "sandbox",
                                                          "agent_ids": ["wellness"]}).status_code == 200


def test_watch_pushes_approval_when_nobody_follows(client):
    register(client)
    gate = threading.Event()
    client.backend.runs["run_w"] = [
        sse("message.delta", run_id="run_w", delta="Je supprime le dossier."),
        approval_event("run_w", "req_1"),
        approval_event("run_w", "req_1"),  # duplicate must not push twice
        gate,
        sse("run.completed", run_id="run_w"),
    ]
    resp = client.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_w"})
    assert resp.status_code == 200 and resp.json()["watching"] is True
    apns = client.apns
    assert wait_until(lambda: len(apns.requests) >= 1)
    payload = apns.payloads()[0]
    assert payload["aps"]["category"] == "APPROVAL" and payload["aps"]["alert"]["body"] == "Approbation requise"
    assert payload["aps"]["thread-id"] == "wellness" and payload["aps"]["mutable-content"] == 1
    assert (payload["run_id"], payload["request_id"], payload["agent"]) == ("run_w", "req_1", "wellness")
    assert payload["choices"] == ["once", "session", "deny"]
    assert apns.requests[0].headers["apns-collapse-id"] == "req_1"

    pending = client.get("/v1/approvals", headers=AUTH).json()["items"]
    assert len(pending) == 1 and pending[0]["command"] == "rm -rf build/" and pending[0]["request_id"] == "req_1"

    gate.set()
    # Run finished while watched and unfollowed -> silent push for resync; approval cleared.
    assert wait_until(lambda: len(apns.requests) >= 2)
    silent = apns.payloads()[1]
    assert silent == {"aps": {"content-available": 1}, "agent": "wellness", "reason": "run_finished",
                      "run_id": "run_w", "status": "run.completed"}
    assert apns.requests[1].headers["apns-push-type"] == "background"
    assert client.get("/v1/approvals", headers=AUTH).json()["items"] == []
    assert len([p for p in apns.payloads() if p["aps"].get("category") == "APPROVAL"]) == 1


def test_no_approval_push_while_voice_client_follows(client):
    register(client)
    gate = threading.Event()
    client.backend.runs["run_f"] = [
        sse("message.delta", run_id="run_f", delta="Je dois demander. "),
        gate,
        approval_event("run_f", "req_2"),
        sse("run.completed", run_id="run_f"),
    ]
    with client.websocket_connect("/v1/voice", headers=AUTH) as ws:
        ws.send_text(json.dumps({"type": "follow", "agent": "wellness", "run_id": "run_f"}))
        receive_frames(ws, until_type="sentence")
        ws.receive()  # audio
        client.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_f"})
        gate.set()
        frames = receive_frames(ws)
        assert frames[-1][1]["reason"] == "completed"
    assert client.apns.requests == []  # follower attached: the app shows the approval itself


def test_unwatched_run_never_pushes(client, config):
    config.watch_followed_runs = False
    register(client)
    client.backend.runs["run_u"] = [approval_event("run_u", "req_3"), sse("run.completed", run_id="run_u")]
    with client.websocket_connect("/v1/voice", headers=AUTH) as ws:
        ws.send_text(json.dumps({"type": "follow", "agent": "wellness", "run_id": "run_u"}))
        receive_frames(ws)
    assert client.apns.requests == []


def test_approve_forwards_to_hermes(client):
    backend = client.backend
    resp = client.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": "run_9",
                                                          "request_id": "req_9", "choice": "Once"})
    assert resp.status_code == 200 and resp.json()["resolved"] == 1
    assert backend.approvals == [{"run_id": "run_9", "body": {"choice": "once", "request_id": "req_9"}}]
    backend.approval_response = (409, {"error": {"code": "approval_not_pending"}})
    resp = client.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": "run_9", "choice": "deny"})
    assert resp.status_code == 409 and backend.approvals[-1]["body"] == {"choice": "deny"}
    assert client.post("/v1/approve", headers=AUTH,
                       json={"agent": "wellness", "run_id": "../x", "choice": "once"}).status_code == 400
    assert client.post("/v1/approve", headers=AUTH,
                       json={"agent": "ghost", "run_id": "r", "choice": "once"}).status_code == 404


def test_voice_reports_unreachable_hermes(client):
    with client.websocket_connect("/v1/voice", headers=AUTH) as ws:
        ws.send_text(json.dumps({"type": "follow", "agent": "wellness", "run_id": "down_1"}))
        frames = receive_frames(ws)
    assert frames[1][1]["code"] == "hermes_unreachable"
    assert frames[-1][1] == {"type": "done", "run_id": "down_1", "reason": "error"}
    assert len(client.backend.run_requests) == 3


def read_sse(resp):
    """(event, data) pairs of an SSE response until it ends; keepalives skipped."""
    events, name = [], None
    for line in resp.iter_lines():
        if line.startswith("event: "):
            name = line[len("event: "):]
        elif line.startswith("data: "):
            events.append((name, json.loads(line[len("data: "):])))
    return events


def test_app_follows_run_through_bridge_and_replays_on_reattach(client):
    client.backend.runs["run_a"] = [
        sse("tool.started", run_id="run_a", tool="web_search"),
        sse("message.delta", run_id="run_a", delta="Bonjour "),
        sse("message.delta", run_id="run_a", delta="Alex."),
        sse("run.completed", run_id="run_a", output="Bonjour Alex."),
    ]
    url = "/v1/runs/run_a/events?agent=wellness&session_id=api_123"
    with client.stream("GET", url, headers=AUTH) as resp:
        assert resp.status_code == 200 and resp.headers["content-type"].startswith("text/event-stream")
        first = read_sse(resp)
    assert [name for name, _ in first] == ["tool.started", "message.delta", "message.delta", "run.completed"]
    assert first[-1][1]["output"] == "Bonjour Alex."
    # Re-attaching (app reopened, connection lost) replays the whole run from the bridge's backlog,
    # consecutive deltas merged.
    with client.stream("GET", url, headers=AUTH) as resp:
        replay = read_sse(resp)
    assert [name for name, _ in replay] == ["tool.started", "message.delta", "run.completed"]
    assert replay[1][1]["delta"] == "Bonjour Alex."
    assert len(client.backend.run_requests) == 1  # one Hermes subscription for both


def test_app_route_validates_agent_and_ids(client):
    assert client.get("/v1/runs/run_a/events?agent=nope", headers=AUTH).status_code == 404
    assert client.get("/v1/runs/run_a/events?agent=wellness&session_id=bad%20id", headers=AUTH).status_code == 400


def test_reply_ready_pushed_when_the_app_left(client):
    register(client)
    gate = threading.Event()
    client.backend.runs["run_r"] = [sse("message.delta", run_id="run_r", delta="Je cherche."), gate,
                                    sse("run.completed", run_id="run_r", output="Le train de 9h est à 19 €.")]
    services = client.app.state.services

    def follow_then_leave():  # what the SSE route does, then the app leaves the conversation
        listener = services.hub.subscribe(services.config.agent("wellness"), "run_r", kind="app", watch=True)
        listener.sub.session_id = "api_9"
        assert listener.sub.followers == 1
        listener.close()
        return listener.sub

    sub = client.portal.call(follow_then_leave)
    assert sub.followers == 0
    gate.set()
    apns = client.apns
    assert wait_until(lambda: len(apns.requests) >= 1)
    payload = apns.payloads()[0]
    assert payload["kind"] == "reply" and payload["session_id"] == "api_9" and payload["run_id"] == "run_r"
    assert payload["aps"]["category"] == "MESSAGE" and payload["aps"]["alert"]["body"] == "Ta réponse est prête."
    # The text stays off Apple: the extension fetches it from the bridge.
    reply = client.get(f"/v1/replies/{payload['reply_id']}", headers=AUTH)
    assert reply.status_code == 200 and reply.json()["text"] == "Le train de 9h est à 19 €."
    assert client.get("/v1/replies/nope", headers=AUTH).status_code == 404


def test_replay_backlog_merges_deltas_so_long_replies_keep_their_start(client):
    import bipbridge.runs as runs_module
    words = [f"mot{i} " for i in range(runs_module.BACKLOG_EVENTS + 500)]
    client.backend.runs["run_long"] = [sse("message.delta", run_id="run_long", delta="Début : ")] + [
        sse("message.delta", run_id="run_long", delta=w) for w in words] + [
        sse("tool.started", run_id="run_long", tool="web_search"),
        sse("message.delta", run_id="run_long", delta="Fin."),
        sse("run.completed", run_id="run_long", output="…")]
    with client.stream("GET", "/v1/runs/run_long/events?agent=wellness", headers=AUTH) as resp:
        read_sse(resp)  # first follower: every delta, live
    with client.stream("GET", "/v1/runs/run_long/events?agent=wellness", headers=AUTH) as resp:
        replay = read_sse(resp)  # late follower: the backlog
    assert [name for name, _ in replay] == ["message.delta", "tool.started", "message.delta", "run.completed"]
    assert replay[0][1]["delta"] == "Début : " + "".join(words)


def test_question_pushed_when_the_app_left(client):
    register(client)
    gate = threading.Event()
    client.backend.runs["run_q"] = [
        gate,
        sse("clarify.request", run_id="run_q", request_id="clr_1",
            questions=[{"id": "q1", "question": "Quel train ?", "choices": ["9h", "14h"]}]),
        threading.Event(),  # the run waits for the answer
    ]
    services = client.app.state.services

    def follow_then_leave():
        listener = services.hub.subscribe(services.config.agent("wellness"), "run_q", kind="app", watch=True)
        listener.sub.session_id = "api_7"
        listener.close()

    client.portal.call(follow_then_leave)
    gate.set()
    apns = client.apns
    assert wait_until(lambda: len(apns.requests) >= 1)
    payload = apns.payloads()[0]
    assert payload["kind"] == "question" and payload["request_id"] == "clr_1" and payload["session_id"] == "api_7"
    assert payload["aps"]["alert"]["body"] == "Une question pour toi" and payload["aps"]["mutable-content"] == 1


def test_approval_is_pushed_when_the_app_stops_following(client):
    register(client)
    asked, finish = threading.Event(), threading.Event()
    client.backend.runs["run_l"] = [
        sse("message.delta", run_id="run_l", delta="Je génère le podcast. "),
        asked,
        approval_event("run_l", "req_l"),
        finish,
        sse("run.completed", run_id="run_l"),
    ]
    with client.websocket_connect("/v1/voice", headers=AUTH) as ws:
        ws.send_text(json.dumps({"type": "follow", "agent": "wellness", "run_id": "run_l"}))
        receive_frames(ws, until_type="sentence")
        client.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_l"})
        asked.set()
        assert wait_until(lambda: client.get("/v1/approvals", headers=AUTH).json()["items"])
        assert client.apns.requests == []  # on screen: the app shows it
    # Left the conversation with the approval unanswered: it is pushed now, once.
    assert wait_until(lambda: any(p["aps"].get("category") == "APPROVAL" for p in client.apns.payloads()))
    approvals = [p for p in client.apns.payloads() if p["aps"].get("category") == "APPROVAL"]
    assert len(approvals) == 1 and approvals[0]["request_id"] == "req_l"
    finish.set()


def test_watched_runs_resume_after_a_restart_with_their_pending_approval(tmp_path):
    import asyncio
    from bipbridge.config import AgentConfig
    from bipbridge.hermes import HermesHTTPError
    from bipbridge.runs import RunHub
    from bipbridge.store import Store

    class Hermes:
        def __init__(self, runs):
            self.runs = runs

        async def run(self, agent, run_id):
            if run_id not in self.runs:
                raise HermesHTTPError(404, "gone")
            return self.runs[run_id]

        async def run_events(self, agent, run_id):
            await asyncio.sleep(3600)
            yield  # pragma: no cover

    class Push:
        def __init__(self):
            self.approvals = []

        async def notify_approval(self, *args):
            self.approvals.append(args)

    agent = AgentConfig(name="vie", display_name="Vie", hermes_url="http://h", hermes_key="k" * 40)

    async def go():
        store = Store(str(tmp_path / "b.db"))
        before = RunHub(Hermes({}), None, store=store)
        before.watch(agent, "run_wait", "api_bot")
        before.watch(agent, "run_done")
        before.watch(agent, "run_gone")
        await asyncio.sleep(0.05)
        await before.close()
        hermes = Hermes({
            "run_wait": {"status": "waiting_for_approval", "session_id": "api_bot",
                         "approval": {"request_id": "req_x", "command": "python3 -c 'tts health'",
                                      "choices": ["once", "session", "deny"]}},
            "run_done": {"status": "completed"},
        })
        push = Push()
        after = RunHub(hermes, push, store=store)
        assert await after.resume({"vie": agent}) == 1
        pending = after.pending_approvals("vie")
        assert [(p["run_id"], p["request_id"], p["session_id"]) for p in pending] == [("run_wait", "req_x", "api_bot")]
        await asyncio.sleep(0.05)
        assert len(push.approvals) == 1 and push.approvals[0][5] == "api_bot"  # nobody follows: pushed, with its session
        assert [r["run_id"] for r in await store.watched_runs(3600)] == ["run_wait"]  # finished and unknown ones forgotten
        await after.close()

    asyncio.run(go())
