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
