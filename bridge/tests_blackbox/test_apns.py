"""The APNs client as Apple sees it: an ES256 provider token signed with the team's key (`kid`, `iss`, `iat`), kept
50 minutes, made again once when Apple calls it expired; dead tokens dropped, a network error kept (tests/test_outbox_push.py's
test_apns_jwt_refresh_and_retry, over HTTP)."""
import json
import time

import jwt
from cryptography.hazmat.primitives import serialization

from bbkit.common import AUTH, sse
from bbkit.servers import wait_until

NOW = "2026-10-21T08:00:00.000Z"
LIVE, GONE, BAD = "1" * 64, "2" * 64, "3" * 64


def with_devices(bridges, *tokens, environment="production"):
    bridge = bridges.new()
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (token, environment, "[]", NOW, NOW)) for token in tokens])
    return bridge


def finish_a_run(bridge, fakes, run_id):
    fakes.hermes.runs[run_id] = [sse("run.completed", output="Fini.")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": run_id})


def devices(bridge):
    return sorted(row[0] for row in bridge.db().execute("SELECT token FROM devices"))


def test_the_provider_token_is_signed_with_the_team_key_and_kept(bridges, fakes, p8_path):
    bridge = with_devices(bridges, LIVE).start()
    finish_a_run(bridge, fakes, "run_1")
    assert wait_until(lambda: fakes.apns.pushes())
    first = fakes.apns.pushes()[0]
    assert first.header("authorization").startswith("bearer ") and fakes.apns.environment(first) == "production"
    token = first.header("authorization").split(" ", 1)[1]
    assert jwt.get_unverified_header(token) == {"alg": "ES256", "kid": "TESTKEY123", "typ": "JWT"}
    public = serialization.load_pem_private_key(open(p8_path, "rb").read(), password=None).public_key()
    claims = jwt.decode(token, public, algorithms=["ES256"])
    assert set(claims) == {"iss", "iat"} and claims["iss"] == "TEAM123456" and abs(claims["iat"] - time.time()) < 60
    finish_a_run(bridge, fakes, "run_2")
    assert wait_until(lambda: len(fakes.apns.pushes()) == 2)
    assert fakes.apns.pushes()[1].header("authorization") == first.header("authorization")   # kept, not remade


def test_an_expired_provider_token_is_made_again_once(bridges, fakes):
    bridge = with_devices(bridges, LIVE).start()
    fakes.apns.script = [(403, "ExpiredProviderToken")]
    finish_a_run(bridge, fakes, "run_1")
    assert wait_until(lambda: len(fakes.apns.pushes()) == 2)
    first, second = fakes.apns.pushes()
    assert first.header("authorization") != second.header("authorization") and first.body == second.body
    time.sleep(0.5)
    assert len(fakes.apns.pushes()) == 2 and devices(bridge) == [LIVE]
    logged = bridge.logged("apns push")
    assert logged[-1]["status"] == 200 and logged[-1]["token"] == LIVE[:6] + "…" and logged[-1]["type"] == "alert"   # « reply ready »


def test_dead_tokens_go_and_a_network_error_keeps_them(bridges, fakes):
    bridge = with_devices(bridges, LIVE, GONE, BAD).start()
    fakes.apns.responses = {GONE: (410, "Unregistered"), BAD: (400, "BadDeviceToken"), LIVE: (500, "InternalServerError")}
    finish_a_run(bridge, fakes, "run_1")
    assert wait_until(lambda: devices(bridge) == [LIVE])
    assert bridge.logged("device token removed")
    down = bridges.new("down", change=lambda c: c["apns"].update(base_url=fakes.down.url, sandbox_url=fakes.down.url))
    down.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                   (LIVE, "sandbox", "[]", NOW, NOW))])
    fakes.ntfy.lines["hermes-wellness-out"] = [json.dumps({"id": "m1", "event": "message", "message": "Bonjour"})]
    down.start()
    assert wait_until(lambda: down.logged("apns unreachable"), timeout=15)
    assert devices(down) == [LIVE]
