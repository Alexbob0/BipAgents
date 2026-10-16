"""Android notifications: FCM HTTP v1 with the Firebase service account, a data message the app turns into a
notification, dead tokens dropped, and Android devices registered next to iPhones."""
import asyncio
import json

import httpx
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import rsa

from bipbridge.fcm import FcmClient, data_message
from bipbridge.push import approval_payload, reply_payload
from conftest import AUTH


def account(tmp_path):
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    pem = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                            serialization.NoEncryption()).decode()
    path = tmp_path / "fcm.json"
    path.write_text(json.dumps({"project_id": "bipagents-test", "client_email": "push@bipagents-test.iam.gserviceaccount.com",
                                "private_key": pem, "token_uri": "https://oauth2.googleapis.com/token"}))
    return str(path)


def test_an_apns_payload_becomes_fcm_data():
    data = data_message(approval_payload("wellness", "Wellness", "run_1", "req_1", ["once", "deny"], session_id="s1"))
    assert data["category"] == "APPROVAL" and data["agent"] == "wellness" and data["run_id"] == "run_1"
    assert data["choices"] == '["once", "deny"]' and data["session_id"] == "s1" and data["title"] == "Wellness"
    assert all(isinstance(v, str) for v in data.values())
    reply = data_message(reply_payload("vie", "Vie", "run_2", reply_id="abc"))
    assert reply["kind"] == "reply" and reply["reply_id"] == "abc"


def test_sending_exchanges_the_key_for_a_token_and_drops_dead_tokens(tmp_path):
    seen = []

    def handler(request: httpx.Request) -> httpx.Response:
        seen.append(request)
        if request.url.host == "oauth2.googleapis.com":
            assert b"jwt-bearer" in request.content
            return httpx.Response(200, json={"access_token": "ya29.test", "expires_in": 3600})
        assert request.headers["authorization"] == "Bearer ya29.test"
        message = json.loads(request.content)["message"]
        if message["token"] == "dead:token-0123456789":
            return httpx.Response(404, json={"error": {"status": "NOT_FOUND", "details": [{"errorCode": "UNREGISTERED"}]}})
        assert message["android"]["priority"] == "HIGH" and message["data"]["category"] == "MESSAGE"
        return httpx.Response(200, json={"name": "projects/bipagents-test/messages/1"})

    async def run():
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as http:
            fcm = FcmClient(account(tmp_path), http)
            payload = reply_payload("vie", "Vie", "run_2")
            ok = await fcm.send("live:token-0123456789", payload)
            dead = await fcm.send("dead:token-0123456789", payload)
            return ok, dead

    ok, dead = asyncio.run(run())
    assert ok.ok and dead.token_is_dead
    assert sum(1 for r in seen if r.url.host == "oauth2.googleapis.com") == 1   # the access token is reused
    assert seen[1].url.path == "/v1/projects/bipagents-test/messages:send"


def test_an_android_device_registers_with_its_token_as_is(client):
    token = "eXaMpLe:APA91bH_" + "x" * 140
    resp = client.post("/v1/devices", headers=AUTH, json={"token": token, "environment": "fcm", "agent_ids": ["wellness"]})
    assert resp.status_code == 200 and resp.json()["token"] == token   # FCM tokens are case-sensitive
    assert client.post("/v1/devices", headers=AUTH, json={"token": "bad token!", "environment": "fcm"}).status_code == 400
    assert client.post("/v1/devices", headers=AUTH, json={"token": "ab" * 32, "environment": "other"}).status_code == 400


def test_each_device_gets_its_own_service():
    from bipbridge.apns import ApnsResult
    from bipbridge.push import PushService

    class Sender:
        def __init__(self):
            self.sent = []

        async def send(self, token, payload, **kw):
            self.sent.append((token, kw["environment"]))
            return ApnsResult(200)

    class Store:
        async def devices_for_agent(self, agent):
            return [{"token": "ab" * 32, "environment": "production"}, {"token": "x:fcm-token-0123456789", "environment": "fcm"}]

    class Config:
        push_previews = False

        def agent(self, name):
            return None

    apple, android = Sender(), Sender()
    sent = asyncio.run(PushService(Config(), Store(), apple, android).notify_silent("wellness", "test"))
    assert sent == 2 and apple.sent == [("ab" * 32, "production")] and android.sent == [("x:fcm-token-0123456789", "fcm")]
    # No Android sender: iPhones still get theirs.
    apple2 = Sender()
    assert asyncio.run(PushService(Config(), Store(), apple2, None).notify_silent("wellness", "test")) == 1
