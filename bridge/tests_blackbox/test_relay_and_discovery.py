"""The relay for other installs (`/v1/relay/push`), an install sending through another's relay, devices registered
with the publisher's relay, agents' discovery and the install's QR code, `/v1/status`
(tests/test_relay_and_discovery.py, over HTTP and the CLI)."""
import json

from bbkit.common import AUTH, BRIDGE_KEY, HERMES_KEY
from bbkit.servers import free_port, wait_until

MEMBER_KEY = "member-relay-key-0123456789abcdef012345"
TOKEN = "ab" * 32


def ntfy_message(fakes, msg_id="m1", text="Le texte du message"):
    fakes.ntfy.lines["hermes-wellness-out"] = [json.dumps({"id": msg_id, "event": "message", "message": text})]


def test_relay_forwards_to_apns_with_the_admin_key(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(relay={"clients": {"chi": MEMBER_KEY}, "per_hour": 3})).start()
    push = {"token": TOKEN, "environment": "sandbox", "payload": {"aps": {"alert": {"title": "Vie", "body": "Prêt"}}}}
    refused = bridge.post("/v1/relay/push", json=push)
    assert refused.status_code == 401 and refused.headers["www-authenticate"] == "Bearer"
    assert bridge.post("/v1/relay/push", json=push, headers=AUTH).status_code == 401  # the bridge key is not a relay key
    member = {"Authorization": f"Bearer {MEMBER_KEY}"}
    resp = bridge.post("/v1/relay/push", json={**push, "token": TOKEN.upper(), "collapse_id": "c1"}, headers=member)
    assert resp.status_code == 200 and resp.json() == {"status": 200, "reason": None, "apns_id": "abc"}
    sent = fakes.apns.pushes()[-1]
    assert sent.json()["aps"]["alert"]["body"] == "Prêt"
    assert sent.path == f"/sandbox/3/device/{TOKEN}"                       # lower-cased
    assert sent.header("apns-collapse-id") == "c1" and sent.header("authorization").startswith("bearer ")
    bad = dict(push, token="not-hex")
    assert bridge.post("/v1/relay/push", json=bad, headers=member).status_code == 400
    for wrong in ({"environment": "dev"}, {"push_type": "voip"}, {"priority": 7}):
        resp = bridge.post("/v1/relay/push", json={**push, **wrong}, headers=member)
        assert resp.status_code == 400 and resp.json() == {"detail": "invalid push"}, wrong
    big = dict(push, payload={"aps": {}, "blob": "x" * 5000})
    too_big = bridge.post("/v1/relay/push", json=big, headers=member)
    assert too_big.status_code == 413 and too_big.json() == {"detail": "payload too large"}
    assert bridge.post("/v1/relay/push", json={"token": TOKEN}, headers=member).status_code == 422
    fakes.apns.responses[TOKEN] = (410, "Unregistered")
    gone = bridge.post("/v1/relay/push", json=push, headers=member)
    assert gone.json() == {"status": 410, "reason": "Unregistered", "apns_id": None}     # the 2nd of 3 this hour
    bridge.post("/v1/relay/push", json=push, headers=member)
    quota = bridge.post("/v1/relay/push", json=push, headers=member)
    assert quota.status_code == 429 and quota.json() == {"detail": "quota exceeded"}  # 3 per hour here
    assert bridge.logged("relayed push")[0]["client"] == "chi"


def test_relay_clients_from_a_file(bridges, fakes):
    bridge = bridges.new()
    clients = bridge.root / "relay-clients"
    clients.write_text(f"# one install per line\nchi {MEMBER_KEY}\n")
    bridge.config["relay"] = {"clients_file": str(clients)}
    bridge.start()
    push = {"token": TOKEN, "environment": "production", "payload": {"aps": {}}}
    assert bridge.post("/v1/relay/push", json=push, headers={"Authorization": f"Bearer {MEMBER_KEY}"}).status_code == 200
    assert fakes.apns.environment(fakes.apns.pushes()[-1]) == "production"


def test_relay_without_the_apple_key_says_so(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(relay={"clients": {"chi": MEMBER_KEY}}, apns={})).start()
    push = {"token": TOKEN, "environment": "sandbox", "payload": {"aps": {}}}
    resp = bridge.post("/v1/relay/push", json=push, headers={"Authorization": f"Bearer {MEMBER_KEY}"})
    assert resp.status_code == 503 and resp.json() == {"detail": "APNs not configured on the relay"}
    android = bridge.post("/v1/relay/push", json={**push, "environment": "fcm", "token": "x:fcm-token-0123456789"},
                          headers={"Authorization": f"Bearer {MEMBER_KEY}"})
    assert android.status_code == 503 and android.json() == {"detail": "FCM not configured on the relay"}


def test_relay_off_by_default(bridge):
    push = {"token": TOKEN, "environment": "sandbox", "payload": {"aps": {}}}
    resp = bridge.post("/v1/relay/push", json=push, headers={"Authorization": f"Bearer {MEMBER_KEY}"})
    assert resp.status_code == 404 and resp.json() == {"detail": "not found"}


def test_member_install_sends_through_the_relay(bridges, fakes):
    """No Apple key here: pushes go to the administrator's bridge (`[push] relay_url`), with this install's key."""
    bridge = bridges.new(change=lambda c: c.update(
        apns={}, push={"relay_url": fakes.admin_relay.url + "/", "relay_key": MEMBER_KEY}))
    ntfy_message(fakes)
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (TOKEN, "production", "[]", "2026-10-21T08:00:00.000Z", "2026-10-21T08:00:00.000Z"))])
    bridge.start()
    health = bridge.get("/health").json()
    assert health["push"] == "relay" and health["apns"] is False
    assert wait_until(lambda: fakes.admin_relay.seen("/v1/relay/push"), timeout=15)
    sent = fakes.admin_relay.seen("/v1/relay/push")[0]
    assert sent.header("authorization") == f"Bearer {MEMBER_KEY}"
    body = sent.json()
    assert body["token"] == TOKEN and body["environment"] == "production" and body["push_type"] == "alert"
    assert body["priority"] == 10 and body["collapse_id"] is None and body["payload"]["aps"]["category"] == "MESSAGE"
    assert set(body) == {"token", "environment", "payload", "push_type", "priority", "collapse_id"}


def test_relay_config_checks(bridges):
    short = bridges.new("short", change=lambda c: c.update(relay={"clients": {"chi": "short"}}))
    short.write_config()
    checked = short.cli("check")
    assert checked.returncode == 2 and "32 characters" in checked.stderr and checked.stderr.startswith("config error: ")
    keyless = bridges.new("keyless", change=lambda c: c.update(push={"relay_url": "https://admin:8643"}))
    keyless.write_config()
    checked = keyless.cli("check")
    assert checked.returncode == 2 and "relay_key" in checked.stderr


def test_agents_discovery_and_install_qr(bridges, fakes):
    def change(c):
        c["public_url"] = "https://box.example.ts.net:8643"
        c["agents"]["Wellness"]["public_url"] = "https://box.example.ts.net:8642"
    bridge = bridges.new(change=change).start()
    assert bridge.get("/v1/agents").status_code == 401
    listed = bridge.get("/v1/agents", headers=AUTH).json()
    assert listed["bridge"] == {"url": "https://box.example.ts.net:8643", "lan": None}
    wellness = next(a for a in listed["agents"] if a["id"] == "wellness")
    assert wellness["name"] == "Wellness" and wellness["url"] == "https://box.example.ts.net:8642"
    assert wellness["key"] == HERMES_KEY
    install = bridge.cli("qr", "--json")
    assert install.returncode == 0, install.stderr
    assert json.loads(install.stdout) == {"v": 2, "bridgeURL": "https://box.example.ts.net:8643", "bridgeKey": BRIDGE_KEY}
    one = bridge.cli("qr", "--json", "--agent", "Wellness")
    assert json.loads(one.stdout) == {"name": "Wellness", "agent": "wellness", "baseURL": "https://box.example.ts.net:8642",
                                      "apiKey": HERMES_KEY, "bridgeURL": "https://box.example.ts.net:8643",
                                      "bridgeKey": BRIDGE_KEY, "voice": "4193"}
    unknown = bridge.cli("qr", "--json", "--agent", "nobody")
    assert unknown.returncode == 2 and unknown.stderr.strip() == "unknown agent: nobody"


def test_status_tells_server_from_model(bridges, fakes):
    closed = f"http://127.0.0.1:{free_port()}"

    def change(c):
        c["model"] = {"health_url": f"{closed}/v1/models"}
        c["agents"]["Vie"] = {"hermes_url": fakes.hermes.url + "/p/vie", "hermes_key": "v" * 32}
        c["agents"]["Panne"] = {"hermes_url": closed, "hermes_key": "p" * 32}
    bridge = bridges.new(change=change).start()
    fakes.hermes.capabilities["vie"] = 502
    status = bridge.get("/v1/status", headers=AUTH).json()
    assert status["agents"]["wellness"] == {"ok": True}
    assert status["agents"]["vie"] == {"ok": False, "error": "HermesHTTPError"}
    assert status["agents"]["panne"] == {"ok": False, "error": "ConnectError"}
    assert status["model"] == {"ok": False, "error": "ConnectError"}
    assert status["protected"] is False and list(status) == ["agents", "model", "protected"]


def test_status_with_a_model_that_answers(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(model={"health_url": fakes.model.url + "/v1/models",
                                                          "health_key": "sk-model-key"})).start()
    assert bridge.get("/v1/status", headers=AUTH).json()["model"] == {"ok": True, "status": 200}
    assert fakes.model.requests[-1].header("authorization") == "Bearer sk-model-key"
    fakes.model.status = 401
    assert bridge.get("/v1/status", headers=AUTH).json()["model"] == {"ok": False, "status": 401}
    plain = bridges.new("plain").start()
    assert plain.get("/v1/status", headers=AUTH).json()["model"] is None


def test_devices_registered_with_the_publishers_relay_go_through_it(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(push={"service_url": fakes.push_service.url + "/"}))
    live, dead = "r2." + "Ab_-" * 15, "r2." + "x" * 56 + "dead"
    bridge.start()
    assert bridge.get("/health").json()["push_relay"] is True
    for token in (live, dead):
        device = bridge.post("/v1/devices", json={"token": token, "environment": "relay", "agent_ids": ["wellness"]},
                             headers=AUTH).json()
        assert device["token"] == token and device["environment"] == "relay"   # kept as is: case-sensitive
    bad = bridge.post("/v1/devices", json={"token": TOKEN, "environment": "relay"}, headers=AUTH)
    assert bad.status_code == 400 and bad.json() == {"detail": "invalid relay token"}
    old = bridge.post("/v1/devices", json={"token": "r1." + "A" * 60, "environment": "relay"}, headers=AUTH)
    assert old.status_code == 400   # the relay's old unexpiring tokens are refused (audit 2026-10-27, 20)
    # A message for the agent: both devices are sent it through the publisher's relay.
    bridge.stop()
    ntfy_message(fakes, "out-1")
    bridge.start()
    assert wait_until(lambda: len(fakes.push_service.seen("/v1/relay/push")) == 2, timeout=15)
    sent = fakes.push_service.seen("/v1/relay/push")
    assert {r.json()["token"] for r in sent} == {live, dead}
    assert all(r.header("authorization") is None for r in sent)
    assert all(r.json()["environment"] == "relay" for r in sent)
    # The relay said the second device is gone: forgotten here.
    with bridge.db() as db:
        assert wait_until(lambda: [row[0] for row in bridge.db().execute("SELECT token FROM devices")] == [live])
    # The app takes it back: its relay token, as it is.
    assert bridge.delete(f"/v1/devices/{live}", headers=AUTH).status_code == 204
    with bridge.db() as db:
        assert db.execute("SELECT count(*) FROM devices").fetchone()[0] == 0


def test_a_bridge_without_the_relay_refuses_relay_tokens(bridge):
    assert bridge.get("/health").json()["push_relay"] is False
    response = bridge.post("/v1/devices", json={"token": "r2." + "A" * 60, "environment": "relay"}, headers=AUTH)
    assert response.status_code == 409 and response.json() == {"detail": "this bridge has no push relay set up"}


def test_the_check_command(bridges, fakes):
    bridge = bridges.new()
    bridge.write_config()
    out = bridge.cli("check")
    assert out.returncode == 0, out.stderr
    lines = out.stdout.splitlines()
    assert lines[0] == f"config OK: {bridge.config_path}"
    assert f"  listen      127.0.0.1:{bridge.port}" in lines
    assert f"  kyutai      {fakes.kyutai.url}" in lines and "  voice       5476 (default)" in lines
    assert "  apns        enabled (sandbox)" in lines and "  lan         off" in lines
    assert any(line.startswith("  agent wellness: hermes=") for line in lines)
    assert len(bridge.cli("genkey").stdout.strip()) == 43


def test_the_relays_answer_counts_and_a_relay_down_is_a_soft_failure(bridges, fakes):
    dead, blocked = "d" * 60 + "dead", "e" * 57 + "blocked"
    bridge = bridges.new(change=lambda c: c.update(apns={}, push={"relay_url": fakes.admin_relay.url, "relay_key": MEMBER_KEY}))
    now = "2026-10-21T08:00:00.000Z"
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (token, "production", "[]", now, now)) for token in (TOKEN, dead, blocked)])
    ntfy_message(fakes)
    bridge.start()
    assert wait_until(lambda: len(fakes.admin_relay.seen("/v1/relay/push")) == 3, timeout=15)
    # The relay said the dead one is gone (APNs' 410 in its answer): forgotten; refused or fine: kept.
    assert wait_until(lambda: sorted(r[0] for r in bridge.db().execute("SELECT token FROM devices")) == sorted([TOKEN, blocked]))
    assert bridge.logged("relay refused this device")[0]["detail"] == "this device was blocked for abuse"
    down = bridges.new("down", change=lambda c: c.update(apns={}, push={"relay_url": fakes.down.url, "relay_key": MEMBER_KEY}))
    down.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                   (TOKEN, "production", "[]", now, now))])
    ntfy_message(fakes, "m2")
    down.start()
    assert wait_until(lambda: down.logged("relay unreachable"), timeout=15)
    assert [r[0] for r in down.db().execute("SELECT token FROM devices")] == [TOKEN]   # the device stays registered
