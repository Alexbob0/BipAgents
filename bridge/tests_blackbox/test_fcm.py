"""Android notifications: FCM HTTP v1 with the Firebase service account (its key exchanged for an access token once),
a data message the app turns into a notification, dead tokens dropped, Android devices next to iPhones
(tests/test_fcm.py, over HTTP; `[fcm] base_url` / `token_url` point at the fake)."""
import json
import urllib.parse

import jwt
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import rsa

from bbkit.common import AUTH
from bbkit.servers import wait_until

LIVE = "eXaMpLe:APA91bH_" + "x" * 140
DEAD = "dead:token-0123456789"
APPLE = "ab" * 32


def with_fcm(bridges, fakes):
    bridge = bridges.new()
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    pem = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                            serialization.NoEncryption()).decode()
    account = bridge.root / "fcm.json"
    account.write_text(json.dumps({"project_id": "bipagents-test", "client_email": "push@bipagents-test.iam.gserviceaccount.com",
                                   "private_key": pem, "token_uri": fakes.fcm.url + "/token"}))
    bridge.config["fcm"] = {"service_account": str(account), "base_url": fakes.fcm.url, "token_url": fakes.fcm.url + "/token"}
    bridge.public_key = key.public_key()
    return bridge


def test_an_android_device_registers_with_its_token_as_is(bridge):
    resp = bridge.post("/v1/devices", headers=AUTH, json={"token": LIVE, "environment": "fcm", "agent_ids": ["wellness"]})
    assert resp.status_code == 200 and resp.json()["token"] == LIVE   # FCM tokens are case-sensitive
    bad = bridge.post("/v1/devices", headers=AUTH, json={"token": "bad token!", "environment": "fcm"})
    assert bad.status_code == 400 and bad.json() == {"detail": "invalid FCM token"}
    assert bridge.post("/v1/devices", headers=AUTH, json={"token": "ab" * 32, "environment": "other"}).status_code == 400


def test_sending_exchanges_the_key_for_a_token_and_drops_dead_tokens(bridges, fakes):
    bridge = with_fcm(bridges, fakes)
    fakes.fcm.dead.add(DEAD)
    bridge.start()
    assert bridge.get("/health").json()["fcm"] is True
    for token, env in ((LIVE, "fcm"), (DEAD, "fcm"), (APPLE, "production")):
        assert bridge.post("/v1/devices", headers=AUTH, json={"token": token, "environment": env}).status_code == 200
    bridge.stop()
    fakes.ntfy.lines["hermes-wellness-out"] = [json.dumps({"id": "a1", "event": "message", "message": "Coucou",
                                                           "tags": ["session:api_5"]})]
    bridge.start()
    assert wait_until(lambda: len(fakes.fcm.sends()) == 2 and fakes.apns.pushes(), timeout=15)
    tokens = [r for r in fakes.fcm.requests if r.path == "/token"]
    assert 1 <= len(tokens) <= 2                     # the two devices are sent to at once: each may ask for it
    form = urllib.parse.parse_qs(tokens[0].body.decode())
    assert form["grant_type"] == ["urn:ietf:params:oauth:grant-type:jwt-bearer"]
    claims = jwt.decode(form["assertion"][0], bridge.public_key, algorithms=["RS256"], audience=fakes.fcm.url + "/token")
    assert claims["iss"] == "push@bipagents-test.iam.gserviceaccount.com"
    assert claims["scope"] == "https://www.googleapis.com/auth/firebase.messaging" and claims["exp"] == claims["iat"] + 3600
    sends = fakes.fcm.sends()
    assert all(r.path == "/v1/projects/bipagents-test/messages:send" for r in sends)
    live = next(r.json()["message"] for r in sends if r.json()["message"]["token"] == LIVE)
    outbox_id = bridge.get("/v1/outbox", headers=AUTH).json()["items"][0]["id"]
    assert live == {"token": LIVE, "android": {"priority": "HIGH"},
                    "data": {"title": "Wellness", "body": "Nouveau message", "category": "MESSAGE",
                             "outbox_id": outbox_id, "agent": "wellness", "session_id": "api_5"}}
    assert fakes.apns.token(fakes.apns.pushes()[0]) == APPLE              # the iPhone gets its own
    assert wait_until(lambda: sorted(r[0] for r in bridge.db().execute("SELECT token FROM devices")) == sorted([LIVE, APPLE]))
    # The next message: the access token is reused.
    asked = len([r for r in fakes.fcm.requests if r.path == "/token"])
    from bbkit.common import sse
    fakes.hermes.runs["run_2"] = [sse("run.completed", output="Fini.")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_2"})
    assert wait_until(lambda: len(fakes.fcm.sends()) >= 3, timeout=15)
    assert fakes.fcm.sends()[-1].json()["message"]["data"]["kind"] == "reply"
    assert len([r for r in fakes.fcm.requests if r.path == "/token"]) == asked


def test_approval_data_messages_carry_the_choices_as_json(bridges, fakes):
    import threading
    from bbkit.common import sse
    bridge = with_fcm(bridges, fakes).start()
    bridge.post("/v1/devices", headers=AUTH, json={"token": LIVE, "environment": "fcm", "agent_ids": ["wellness"]})
    fakes.hermes.runs["run_1"] = [sse("approval.request", run_id="run_1", request_id="req_1", command="ls",
                                      choices=["once", "deny"]), threading.Event()]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_1", "session_id": "s1"})
    assert wait_until(lambda: fakes.fcm.sends(), timeout=10)
    message = fakes.fcm.sends()[0].json()["message"]
    assert message["android"] == {"priority": "HIGH", "collapse_key": "req_1"}
    assert message["data"] == {"title": "Wellness", "body": "Approbation requise", "category": "APPROVAL",
                               "agent": "wellness", "run_id": "run_1", "request_id": "req_1",
                               "choices": '["once", "deny"]', "session_id": "s1"}
    assert all(isinstance(v, str) for v in message["data"].values())
