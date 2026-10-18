import asyncio
import json

import httpx
import pytest

from bipbridge.config import ConfigError, parse_config
from conftest import AUTH, BRIDGE_KEY, HERMES_KEY

MEMBER_KEY = "member-relay-key-0123456789abcdef012345"
TOKEN = "ab" * 32


@pytest.fixture
def relay_app(config_dict, apns):
    from fastapi.testclient import TestClient

    from bipbridge.app import create_app

    config_dict["relay"] = {"clients": {"chi": MEMBER_KEY}, "per_hour": 3}
    app = create_app(parse_config(config_dict), transport=httpx.MockTransport(lambda r: httpx.Response(404)),
                     apns_transport=apns.transport(), start_background=False)
    with TestClient(app) as client:
        yield client


def test_relay_forwards_to_apns_with_the_admin_key(relay_app, apns):
    push = {"token": TOKEN, "environment": "sandbox", "payload": {"aps": {"alert": {"title": "Vie", "body": "Prêt"}}}}
    assert relay_app.post("/v1/relay/push", json=push).status_code == 401
    assert relay_app.post("/v1/relay/push", json=push, headers=AUTH).status_code == 401  # the bridge key is not a relay key
    member = {"Authorization": f"Bearer {MEMBER_KEY}"}
    resp = relay_app.post("/v1/relay/push", json=push, headers=member)
    assert resp.status_code == 200 and resp.json()["status"] == 200
    assert apns.payloads()[-1]["aps"]["alert"]["body"] == "Prêt"
    assert apns.requests[-1].url.path.endswith(TOKEN)
    bad = dict(push, token="not-hex")
    assert relay_app.post("/v1/relay/push", json=bad, headers=member).status_code == 400
    big = dict(push, payload={"aps": {}, "blob": "x" * 5000})
    assert relay_app.post("/v1/relay/push", json=big, headers=member).status_code == 413
    for _ in range(2):
        relay_app.post("/v1/relay/push", json=push, headers=member)
    assert relay_app.post("/v1/relay/push", json=push, headers=member).status_code == 429  # 3 per hour here


def test_relay_off_by_default(client):
    push = {"token": TOKEN, "environment": "sandbox", "payload": {"aps": {}}}
    assert client.post("/v1/relay/push", json=push, headers={"Authorization": f"Bearer {MEMBER_KEY}"}).status_code == 404


def test_relay_client_reports_the_relays_answer():
    from bipbridge.relay import RelayClient

    seen = []

    def relay(request: httpx.Request) -> httpx.Response:
        seen.append(request)
        body = json.loads(request.content)
        if body["token"] == "dead":
            return httpx.Response(200, json={"status": 410, "reason": "Unregistered"})
        return httpx.Response(200, json={"status": 200, "apns_id": "x"})

    async def run():
        async with httpx.AsyncClient(transport=httpx.MockTransport(relay)) as http:
            sender = RelayClient("https://admin.example.ts.net:8643/", MEMBER_KEY, http)
            ok = await sender.send(TOKEN, {"aps": {}}, environment="production", collapse_id="c1")
            dead = await sender.send("dead", {"aps": {}}, environment="production")
            return ok, dead

    ok, dead = asyncio.run(run())
    assert ok.ok and dead.token_is_dead
    assert str(seen[0].url) == "https://admin.example.ts.net:8643/v1/relay/push"
    assert seen[0].headers["authorization"] == f"Bearer {MEMBER_KEY}"
    assert json.loads(seen[0].content)["collapse_id"] == "c1"


def test_relay_unreachable_is_a_soft_failure():
    from bipbridge.relay import RelayClient

    def down(request):
        raise httpx.ConnectError("refused")

    async def run():
        async with httpx.AsyncClient(transport=httpx.MockTransport(down)) as http:
            return await RelayClient("https://admin:8643", MEMBER_KEY, http).send(TOKEN, {}, environment="sandbox")

    result = asyncio.run(run())
    assert not result.ok and not result.token_is_dead  # the device stays registered


def test_member_install_sends_through_the_relay(config_dict):
    from bipbridge.app import Services

    config_dict["apns"] = {}
    config_dict["push"] = {"relay_url": "https://admin.example.ts.net:8643", "relay_key": MEMBER_KEY}
    services = Services(parse_config(config_dict))
    try:
        assert services.push_mode == "relay" and services.apns is None
    finally:
        asyncio.run(services.http.aclose())
        asyncio.run(services.apns_http.aclose())
        services.store.close()


def test_relay_config_checks(config_dict, tmp_path):
    clients = tmp_path / "relay-clients"
    clients.write_text(f"# one install per line\nchi {MEMBER_KEY}\n")
    config_dict["relay"] = {"clients_file": str(clients)}
    assert parse_config(dict(config_dict)).relay.clients == {MEMBER_KEY: "chi"}
    config_dict["relay"] = {"clients": {"chi": "short"}}
    with pytest.raises(ConfigError, match="32 characters"):
        parse_config(dict(config_dict))
    config_dict["relay"] = {}
    config_dict["push"] = {"relay_url": "https://admin:8643"}
    with pytest.raises(ConfigError, match="relay_key"):
        parse_config(dict(config_dict))


def test_agents_discovery_and_install_qr(config_dict):
    from fastapi.testclient import TestClient

    from bipbridge.app import create_app
    from bipbridge.lan import install_payload

    config_dict["public_url"] = "https://box.example.ts.net:8643"
    config_dict["agents"]["Wellness"]["public_url"] = "https://box.example.ts.net:8642"
    config = parse_config(config_dict)
    assert install_payload(config) == {"v": 2, "bridgeURL": "https://box.example.ts.net:8643", "bridgeKey": BRIDGE_KEY}
    app = create_app(config, transport=httpx.MockTransport(lambda r: httpx.Response(404)), start_background=False)
    with TestClient(app) as client:
        assert client.get("/v1/agents").status_code == 401
        listed = client.get("/v1/agents", headers=AUTH).json()
    assert listed["bridge"] == {"url": "https://box.example.ts.net:8643", "lan": None}
    wellness = next(a for a in listed["agents"] if a["id"] == "wellness")
    assert wellness["name"] == "Wellness" and wellness["url"] == "https://box.example.ts.net:8642"
    assert wellness["key"] == HERMES_KEY


def test_status_tells_server_from_model(config_dict):
    from fastapi.testclient import TestClient

    from bipbridge.app import create_app

    config_dict["model"] = {"health_url": "http://spark.test:8888/v1/models"}

    def backend(request: httpx.Request) -> httpx.Response:
        if request.url.path.endswith("/v1/capabilities"):
            return httpx.Response(200, json={"runs": True}) if "wellness" in str(request.url) else httpx.Response(502)
        if request.url.host == "spark.test":
            raise httpx.ConnectError("model machine off")
        return httpx.Response(404)

    app = create_app(parse_config(config_dict), transport=httpx.MockTransport(backend), start_background=False)
    with TestClient(app) as client:
        status = client.get("/v1/status", headers=AUTH).json()
    assert status["agents"]["wellness"] == {"ok": True}
    assert status["model"] == {"ok": False, "error": "ConnectError"}


def test_devices_registered_with_the_publishers_relay_go_through_it(config_dict):
    from fastapi.testclient import TestClient

    from bipbridge.app import create_app

    config_dict["push"] = {"service_url": "https://push.example/"}
    sent = []

    def backend(request):
        if request.url.host == "push.example":
            body = json.loads(request.content)
            sent.append((request.url.path, dict(request.headers), body))
            dead = body["token"].endswith("dead")
            return httpx.Response(200, json={"status": 410 if dead else 200, "reason": "Unregistered" if dead else None})
        return httpx.Response(404)

    app = create_app(parse_config(config_dict), transport=httpx.MockTransport(backend), start_background=False)
    live, dead = "r1." + "Ab_-" * 15, "r1." + "x" * 56 + "dead"
    with TestClient(app) as client:
        services = app.state.services
        assert client.get("/health").json()["push_relay"] is True
        for token in (live, dead):
            device = client.post("/v1/devices", json={"token": token, "environment": "relay", "agent_ids": ["wellness"]},
                                 headers=AUTH).json()
            assert device["token"] == token and device["environment"] == "relay"   # kept as is: case-sensitive
        assert client.post("/v1/devices", json={"token": TOKEN, "environment": "relay"}, headers=AUTH).status_code == 400
        sent_count = client.portal.call(services.push.notify_message, "wellness", "out-1", "Le texte du message")
        assert sent_count == 1
        assert {body["token"] for _, _, body in sent} == {live, dead}
        assert all(path == "/v1/relay/push" and "authorization" not in headers for path, headers, _ in sent)
        # The relay said the second device is gone: forgotten here.
        assert client.portal.call(services.store.get_device, dead) is None
        assert client.portal.call(services.store.get_device, live) is not None
        # The app takes it back: its relay token, as it is.
        assert client.delete(f"/v1/devices/{live}", headers=AUTH).status_code == 204
        assert client.portal.call(services.store.get_device, live) is None


def test_a_bridge_without_the_relay_refuses_relay_tokens(client):
    assert client.get("/health").json()["push_relay"] is False
    response = client.post("/v1/devices", json={"token": "r1." + "A" * 60, "environment": "relay"}, headers=AUTH)
    assert response.status_code == 409
