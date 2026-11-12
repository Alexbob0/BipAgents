"""Devices registered with the publisher's relay, agents' discovery and the install's QR code, `/v1/status`
(tests/test_relay_and_discovery.py, over HTTP and the CLI)."""
import json

from bbkit.common import AUTH, BRIDGE_KEY, boite_message
from bbkit.servers import free_port, wait_until

TOKEN = "ab" * 32


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
    assert "key" not in wellness
    install = bridge.cli("qr", "--json")
    assert install.returncode == 0, install.stderr
    assert json.loads(install.stdout) == {"v": 2, "bridgeURL": "https://box.example.ts.net:8643", "bridgeKey": BRIDGE_KEY}
    one = bridge.cli("qr", "--json", "--agent", "Wellness")
    assert json.loads(one.stdout) == {"name": "Wellness", "agent": "wellness", "bridgeURL": "https://box.example.ts.net:8643",
                                      "bridgeKey": BRIDGE_KEY, "voice": "pocket:colibri"}
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
    # one agent: only its Hermes is asked
    asked = len(fakes.hermes.seen("/p/vie/v1/capabilities"))
    one = bridge.get("/v1/status", headers=AUTH, params={"agent": "Panne"}).json()
    assert list(one["agents"]) == ["panne"] and len(fakes.hermes.seen("/p/vie/v1/capabilities")) == asked
    assert bridge.get("/v1/status", headers=AUTH, params={"agent": "nobody"}).status_code == 404


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
    # An app from before 2026-10-28 (the relay's first format): taken until 2026-12-15, and asked for a new token.
    old = bridge.post("/v1/devices", json={"token": "r1." + "A" * 60, "environment": "relay"}, headers=AUTH)
    assert old.status_code == 200 and old.json()["renew"] is True
    assert bridge.delete("/v1/devices/r1." + "A" * 60, headers=AUTH).status_code == 204
    # A message for the agent: both devices are sent it through the publisher's relay.
    boite_message(bridge, fakes, "Le texte du message")
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


FCM_TOKEN = "eXaMpLe:APA91bH_" + "x" * 140


def test_a_place_without_the_relay_reaches_no_phone_until_it_gets_it(bridges, fakes):
    """2026-11-12 (the offer's places, the installs from before 2026-11-04): no `[push] service_url`, so `push_relay`
    false; the Android app gave its Firebase token, which a bridge without a Firebase account can't use: no push, ever.
    With the line the installer's migration 2026-11-12-push-service-everywhere writes, the app's next launch registers
    a relay token and the push goes through the relay."""
    bridge = bridges.new("place", change=lambda c: c.pop("apns")).start()   # a place: no Apple key of its own
    health = bridge.get("/health").json()
    assert health["push_relay"] is False and health["fcm"] is False and health["push"] is None
    # what the app does with such a health: its own token, as it is
    device = bridge.post("/v1/devices", json={"token": FCM_TOKEN, "environment": "fcm", "agent_ids": ["wellness"]},
                         headers=AUTH)
    assert device.status_code == 200
    assert bridge.logged("device registered, but this bridge has no way to send to it (no key of its own, "
                         "no [push] service_url)")
    boite_message(bridge, fakes, "Avant")
    assert not wait_until(lambda: fakes.push_service.seen("/v1/relay/push"), timeout=3)
    # the migration's line, the bridge restarted; the app's next launch reads health again
    bridge.config["push"] = {**bridge.config.get("push", {}), "service_url": fakes.push_service.url}
    bridge.restart()
    assert bridge.get("/health").json()["push_relay"] is True
    relayed = "r2." + "Ab_-" * 15
    assert bridge.post("/v1/devices", json={"token": relayed, "environment": "relay", "agent_ids": ["wellness"]},
                       headers=AUTH).status_code == 200
    boite_message(bridge, fakes, "Après", name="Le point du soir")
    assert wait_until(lambda: fakes.push_service.seen("/v1/relay/push"), timeout=15)
    assert {r.json()["token"] for r in fakes.push_service.seen("/v1/relay/push")} == {relayed}


def test_an_install_with_its_own_apple_key_moves_its_phones_to_the_relay_without_losing_one(bridges, fakes):
    """Alex 2026-11-12: every install sends through the publisher's relay, his own (direct APNs) too. With
    `service_url` the bridge no longer says « apns » (the apps register with the relay at their next launch); a phone
    still known by its APNs token is sent to directly meanwhile; its relay registration (same push key) drops it."""
    import base64
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric import ec
    key = base64.b64encode(ec.generate_private_key(ec.SECP256R1()).public_key().public_bytes(
        serialization.Encoding.X962, serialization.PublicFormat.UncompressedPoint)).decode()
    bridge = bridges.new(change=lambda c: c["push"].update(service_url=fakes.push_service.url)).start()
    health = bridge.get("/health").json()
    assert health["apns"] is True and health["push"] is None and health["push_relay"] is True
    # registered before the switch: its APNs token
    assert bridge.post("/v1/devices", headers=AUTH, json={"token": TOKEN, "environment": "production",
                                                          "agent_ids": ["wellness"], "push_key": key}).status_code == 200
    boite_message(bridge, fakes, "Pendant")
    assert wait_until(lambda: len(fakes.apns.pushes()) == 1, timeout=15)
    # its next launch: a relay token, the same key; the APNs row goes
    relayed = "r2." + "Ab_-" * 15
    assert bridge.post("/v1/devices", headers=AUTH, json={"token": relayed, "environment": "relay",
                                                          "agent_ids": ["wellness"], "push_key": key}).status_code == 200
    with bridge.db() as db:
        assert [r[0] for r in db.execute("SELECT token FROM devices")] == [relayed]
    boite_message(bridge, fakes, "Après", name="Le point du soir")
    assert wait_until(lambda: len(fakes.push_service.seen("/v1/relay/push")) == 1, timeout=15)
    assert len(fakes.apns.pushes()) == 1


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
    assert f"  pocket      {fakes.pocket.url}" in lines and "  voice       pocket:ours (default)" in lines
    assert "  apns        enabled (sandbox)" in lines and "  lan         off" in lines
    assert any(line.startswith("  agent wellness: hermes=") for line in lines)
    assert len(bridge.cli("genkey").stdout.strip()) == 43
