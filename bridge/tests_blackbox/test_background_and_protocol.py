"""What the bridge does on its own and how it speaks HTTP: the purge of old files and messages, the SSE keepalives,
slash redirects, a clean stop (purge_uploads and purge_audio of tests/test_http_routes.py and test_agent_tools.py, plus
behaviours no unit test pinned)."""
import json
import os
import time

import pytest

from bbkit.common import AUTH, sse
from bbkit.mcp import give_token, rpc
from bbkit.servers import wait_until

OLD = "2020-01-01T00:00:00.000Z"


def test_old_uploads_audio_and_messages_are_purged(bridges):
    bridge = bridges.new(change=lambda c: c["limits"].update(purge_interval_hours=0.0005, upload_retention_days=30))
    uploads = bridge.root / "uploads" / "wellness" / "2026-01"
    uploads.mkdir(parents=True)
    long_ago = time.time() - 40 * 86400
    old, new, foreign = uploads / "0123456789ab-old.pdf", uploads / "0123456789ac-new.pdf", uploads / "notes.txt"
    for path in (old, new, foreign):
        path.write_bytes(b"x")
    os.utime(old, (long_ago, long_ago))
    os.utime(foreign, (long_ago, long_ago))
    lonely = bridge.root / "uploads" / "wellness" / "2025-12"
    lonely.mkdir()
    (lonely / "0123456789ad-alone.pdf").write_bytes(b"x")
    os.utime(lonely / "0123456789ad-alone.pdf", (long_ago, long_ago))
    audio = bridge.data / "agent-audio" / "wellness"
    audio.mkdir(parents=True)
    (audio / "old.m4a").write_bytes(b"x")
    (audio / "new.m4a").write_bytes(b"y")
    os.utime(audio / "old.m4a", (1, 1))
    (bridge.data / "audio").mkdir()
    (bridge.data / "audio" / "o1.mp3").write_bytes(b"ID3")
    bridge.seed_db([("INSERT INTO outbox (id, agent, ntfy_id, title, text, created_at, audio_path) VALUES (?,?,?,?,?,?,?)",
                     ("o1", "wellness", "n1", None, "Vieux message", OLD, str(bridge.data / "audio" / "o1.mp3"))),
                    ("INSERT INTO outbox (id, agent, ntfy_id, title, text, created_at) VALUES (?,?,?,?,?,?)",
                     ("o2", "wellness", "n2", None, "Message récent", "2099-01-01T00:00:00.000Z"))])
    bridge.start()
    assert wait_until(lambda: bridge.logged("outbox purged"), timeout=15)
    assert not old.exists() and new.exists() and foreign.exists() and not lonely.exists()
    assert not (audio / "old.m4a").exists() and (audio / "new.m4a").exists()
    assert not (bridge.data / "audio" / "o1.mp3").exists()
    assert [i["id"] for i in bridge.get("/v1/outbox", headers=AUTH).json()["items"]] == ["o2"]
    assert bridge.logged("uploads purged")[0] == {**bridge.logged("uploads purged")[0], "agent": "wellness", "removed": 2}
    assert bridge.logged("agent audio purged")[0]["removed"] == 1 and bridge.logged("outbox purged")[0]["removed"] == 1


@pytest.mark.slow
def test_the_apps_event_stream_keeps_alive_every_ten_seconds(bridge, fakes):
    import threading
    gate = threading.Event()
    fakes.hermes.runs["run_k"] = [gate, sse("run.completed", output="Fini.")]
    started = time.monotonic()
    chunks = []
    with bridge.stream("GET", "/v1/runs/run_k/events?agent=wellness", headers=AUTH) as resp:
        for chunk in resp.iter_raw():
            chunks.append(chunk)
            if b": keepalive\n\n" in b"".join(chunks) and not gate.is_set():
                gate.set()
    text = b"".join(chunks).decode()
    assert text.startswith(": keepalive\n\n") and text.endswith("event: run.completed\n" + 'data: {"event": "run.completed", '
                                                                 '"output": "Fini."}\n\n')
    assert time.monotonic() - started >= 9.5


@pytest.mark.slow
def test_a_long_tool_call_keeps_its_stream_alive(bridge, fakes):
    give_token(bridge)
    fakes.kyutai.delay = 11
    streamed = rpc(bridge, "tools/call", {"name": "create_audio", "arguments": {"text": "Une phrase lente."}},
                   accept="application/json, text/event-stream")
    assert streamed.text.startswith(": reading aloud\n\n")
    last = [line for line in streamed.text.splitlines() if line.startswith("data: ")][-1]
    assert json.loads(last[len("data: "):]) == {"jsonrpc": "2.0", "id": 1,
                                                "result": json.loads(last[len("data: "):])["result"]}
    assert json.loads(last[len("data: "):])["result"]["isError"] is False


def test_a_trailing_slash_is_redirected(bridge):
    resp = bridge.get("/v1/agents/", headers=AUTH)
    assert resp.status_code == 307 and resp.headers["location"] == f"{bridge.url}/v1/agents"
    assert bridge.get("/v1/agents", headers=AUTH).status_code == 200


def test_the_bridge_stops_cleanly(bridge, fakes):
    """SIGTERM (systemd's stop): it ends within its 5 s of grace even with a run watched and ntfy subscribed, with
    nothing logged as an error. (uvicorn re-raises the signal once stopped: -15; exiting 0 is as good.)"""
    import threading
    fakes.hermes.runs["never"] = [threading.Event()]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "never"})
    started = time.monotonic()
    assert bridge.stop() in (0, -15)
    assert time.monotonic() - started < 10
    assert not [e for e in bridge.logs() if e.get("level") in ("error", "critical") or "exc" in e]


def test_android_devices_of_an_install_without_fcm_go_through_the_relay(bridges, fakes):
    member_key = "member-relay-key-0123456789abcdef012345"
    bridge = bridges.new(change=lambda c: c.update(apns={}, push={"relay_url": fakes.admin_relay.url,
                                                                    "relay_key": member_key}))
    bridge.start()
    android = "x:fcm-token-0123456789"
    assert bridge.post("/v1/devices", headers=AUTH, json={"token": android, "environment": "fcm"}).status_code == 200
    fakes.hermes.runs["run_a"] = [sse("run.completed", output="Fini.")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_a"})
    assert wait_until(lambda: fakes.admin_relay.seen("/v1/relay/push"))
    body = fakes.admin_relay.seen("/v1/relay/push")[0].json()
    assert body["token"] == android and body["environment"] == "fcm" and body["collapse_id"] == "run_a"
    assert body["payload"]["kind"] == "reply" and bridge.get("/health").json()["fcm"] is False


@pytest.mark.slow
def test_an_outbox_audio_still_being_made_says_so(bridges, fakes):
    """GET /v1/outbox/{id}/audio waits 20 s for the synthesis, then answers 202 {"status": "pending"}, Retry-After 3."""
    bridge = bridges.new()
    fakes.kyutai.delay = 24
    fakes.ntfy.lines["hermes-wellness-out"] = [json.dumps({"id": "slow", "event": "message", "message": "Un texte lent."})]
    bridge.start()
    assert wait_until(lambda: bridge.get("/v1/outbox", headers=AUTH).json()["items"])
    item = bridge.get("/v1/outbox", headers=AUTH).json()["items"][0]
    assert item["has_audio"] is False
    started = time.monotonic()
    pending = bridge.get(f"/v1/outbox/{item['id']}/audio", headers=AUTH)
    assert pending.status_code == 202 and pending.json() == {"status": "pending"}
    assert pending.headers["retry-after"] == "3" and 19 <= time.monotonic() - started < 23
