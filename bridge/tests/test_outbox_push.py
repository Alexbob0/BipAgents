"""ntfy -> outbox ingestion, audio pre-synthesis, MESSAGE push, outbox routes, APNs client."""
import json
import time

import httpx
import jwt
import pytest

from bipbridge.apns import ApnsClient
from bipbridge.config import ApnsConfig
from conftest import AUTH, NTFY_TOKEN, FakeApns, wait_until

TOKEN = "b" * 64
DEAD = "d" * 64


def register(client, token=TOKEN, env="sandbox", agents=("wellness",)):
    resp = client.post("/v1/devices", headers=AUTH, json={"token": token, "environment": env,
                                                          "agent_ids": list(agents)})
    assert resp.status_code == 200


def ntfy_message(msg_id, text, title="Check-in", tags=None, t=1790000000):
    return json.dumps({"id": msg_id, "time": t, "event": "message", "topic": "hermes-wellness-out",
                       "message": text, "title": title, "tags": tags or []})


@pytest.fixture
def ntfy_client(config, backend, apns):
    """Client whose fake ntfy already holds two messages (one duplicate) for the topic."""
    from fastapi.testclient import TestClient

    from bipbridge.app import create_app

    backend.ntfy_lines["hermes-wellness-out"] = [
        json.dumps({"id": "k1", "event": "keepalive"}),
        ntfy_message("m1", "Bonjour **Sam**, as-tu bien dormi ?", tags=["session:sess_42"]),
        ntfy_message("m1", "Bonjour **Sam**, as-tu bien dormi ?"),  # duplicate delivery
        ntfy_message("m2", "Pense à boire de l'eau.", title=None),
    ]
    app = create_app(config, transport=backend.transport(), apns_transport=apns.transport())
    return TestClient(app), backend, apns


def test_ntfy_message_lands_in_outbox_with_audio_and_push(ntfy_client, config):
    client, backend, apns = ntfy_client
    # Device registered before the messages flow: create it straight in the DB at startup.
    from bipbridge.store import Store
    store = Store(config.outbox.db_path)
    import asyncio
    asyncio.run(store.upsert_device(TOKEN, "sandbox", ["wellness"]))
    asyncio.run(store.upsert_device(DEAD, "production", []))
    store.close()
    apns.responses[DEAD] = (410, "Unregistered")

    with client:
        # m1 -> TOKEN + DEAD (410, then deleted), m2 -> TOKEN (+ DEAD if it raced the deletion)
        assert wait_until(lambda: len([r for r in apns.requests if r.url.path.endswith(TOKEN)]) >= 2, timeout=10)
        req = backend.ntfy_requests[0]
        assert req.url.path == "/hermes-wellness-out/json"
        assert req.headers["authorization"] == f"Bearer {NTFY_TOKEN}"
        assert "since" not in req.url.params

        items = client.get("/v1/outbox", headers=AUTH).json()["items"]
        assert [i["text"] for i in items] == ["Bonjour **Sam**, as-tu bien dormi ?", "Pense à boire de l'eau."]
        first = items[0]
        assert first["agent"] == "wellness" and first["title"] == "Check-in"
        assert first["session_id"] == "sess_42" and first["has_audio"] is True
        assert first["created_at"].endswith("Z") and first["sent_at"].startswith("2026-")

        # Audio was pre-synthesized as mp3 from the normalized text.
        mp3_calls = [b for b in backend.kyutai_inputs if b["response_format"] == "mp3"]
        assert mp3_calls[0]["input"] == "Bonjour Sam, as-tu bien dormi ?" and mp3_calls[0]["voice"] == "4193"
        audio = client.get(f"/v1/outbox/{first['id']}/audio", headers=AUTH)
        assert audio.status_code == 200 and audio.headers["content-type"] == "audio/mpeg"
        assert audio.content.startswith(b"ID3")

        # Push: MESSAGE payload, sandbox host for the sandbox device, generic body (no preview).
        sandbox = [r for r in apns.requests if r.url.host == "api.sandbox.push.apple.com"]
        payload = json.loads(sandbox[0].content)
        assert payload == {"aps": {"alert": {"title": "Wellness", "body": "Nouveau message"},
                                   "thread-id": "wellness", "mutable-content": 1, "category": "MESSAGE",
                                   "sound": "default"},
                           "outbox_id": first["id"], "agent": "wellness", "session_id": "sess_42"}
        assert sandbox[0].headers["apns-topic"] == "io.github.bipagents"
        assert sandbox[0].headers["apns-push-type"] == "alert"
        assert sandbox[0].url.path == f"/3/device/{TOKEN}"
        # The 410 token was deleted after its first failure.
        dead_calls = [r for r in apns.requests if r.url.path.endswith(DEAD)]
        assert 1 <= len(dead_calls) <= 2
        import sqlite3
        with sqlite3.connect(config.outbox.db_path) as db:
            tokens = [row[0] for row in db.execute("SELECT token FROM devices")]
        assert tokens == [TOKEN]

        # Single item + since filtering + agent filter
        one = client.get(f"/v1/outbox/{first['id']}", headers=AUTH).json()
        assert one["text"] == first["text"]
        later = client.get("/v1/outbox", headers=AUTH, params={"since": first["created_at"]}).json()["items"]
        assert [i["text"] for i in later] == ["Pense à boire de l'eau."]
        assert client.get("/v1/outbox", headers=AUTH, params={"agent": "ghost"}).status_code == 404
        assert client.get("/v1/outbox", headers=AUTH, params={"since": "hier"}).status_code == 400
        assert client.get("/v1/outbox/nope", headers=AUTH).status_code == 404

    # Cursor persisted: a restart resumes with since=<last id>.
    from fastapi.testclient import TestClient
    from bipbridge.app import create_app
    backend.ntfy_requests.clear()
    with TestClient(create_app(config, transport=backend.transport(), apns_transport=apns.transport())):
        assert wait_until(lambda: backend.ntfy_requests, timeout=5)
        assert backend.ntfy_requests[0].url.params["since"] == "m2"
    backend.release.set()


def test_push_previews_and_silent_payloads(config):
    from bipbridge.push import approval_payload, message_payload, silent_payload
    assert message_payload("vie", "Vie", "o1", None, "Texte")["aps"]["alert"]["body"] == "Texte"
    assert "session_id" not in message_payload("vie", "Vie", "o1")
    ap = approval_payload("vie", "Vie", "run_1", "req_9", ["once", "deny"])
    assert ap["aps"]["category"] == "APPROVAL" and ap["run_id"] == "run_1" and ap["request_id"] == "req_9"
    assert ap["choices"] == ["once", "deny"]
    assert silent_payload("vie", "run_finished", run_id="r") == {"aps": {"content-available": 1}, "agent": "vie",
                                                                 "reason": "run_finished", "run_id": "r"}


async def test_apns_jwt_refresh_and_retry(p8_path):
    fake = FakeApns()
    now = [1_800_000_000.0]
    cfg = ApnsConfig(team_id="TEAM123456", key_id="TESTKEY123", p8_path=p8_path, environment="production")
    client = ApnsClient(cfg, httpx.AsyncClient(transport=fake.transport()), clock=lambda: now[0])

    res = await client.send("c" * 64, {"aps": {"content-available": 1}}, environment="production",
                            push_type="background", priority=5)
    assert res.ok
    req = fake.requests[0]
    assert req.url.host == "api.push.apple.com"
    assert req.headers["apns-push-type"] == "background" and req.headers["apns-priority"] == "5"
    token = req.headers["authorization"].split(" ", 1)[1]
    header = jwt.get_unverified_header(token)
    assert header["alg"] == "ES256" and header["kid"] == "TESTKEY123"
    claims = jwt.decode(token, options={"verify_signature": False})
    assert claims == {"iss": "TEAM123456", "iat": 1_800_000_000}

    first_token = client.provider_token()
    now[0] += 49 * 60
    assert client.provider_token() == first_token
    now[0] += 2 * 60
    assert client.provider_token() != first_token  # refreshed before Apple's 60 min limit

    # 403 ExpiredProviderToken -> one retry with a fresh token
    calls = []

    def handler(request):
        calls.append(request.headers["authorization"])
        if len(calls) == 1:
            return httpx.Response(403, json={"reason": "ExpiredProviderToken"})
        return httpx.Response(200)

    client.client = httpx.AsyncClient(transport=httpx.MockTransport(handler))
    now[0] += 1
    res = await client.send("c" * 64, {"aps": {}}, environment="sandbox")
    assert res.ok and len(calls) == 2

    bad = ApnsClient(cfg, httpx.AsyncClient(transport=httpx.MockTransport(
        lambda r: httpx.Response(400, json={"reason": "BadDeviceToken"}))))
    result = await bad.send("c" * 64, {"aps": {}}, environment="sandbox")
    assert result.token_is_dead


def test_unwrap_cron_keeps_the_job_name_and_output():
    from bipbridge.ntfy import unwrap_cron
    wrapped = ("Cronjob Response: test-push-bipagents (job_id: 0681ac73805e)\n-------------\n\n"
               "Bonjour Alex, ceci est un test.\n\nNote: The agent cannot see this message, and therefore cannot respond to it.")
    assert unwrap_cron(wrapped) == ("test-push-bipagents", "Bonjour Alex, ceci est un test.")
    assert unwrap_cron("Cronjob Response: Morning feeds\n-------------\n\nLe point du matin.") == ("Morning feeds", "Le point du matin.")
    assert unwrap_cron("  Un message normal.  ") == (None, "Un message normal.")
