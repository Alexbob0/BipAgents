import json

import httpx
import pytest

from bipbridge.config import parse_config
from conftest import AUTH, HERMES_KEY


@pytest.fixture
def lan_config(config_dict):
    config_dict["lan"] = {"enabled": True, "address": "192.168.8.177", "port": 8650}
    config_dict["public_url"] = "https://aibox.example.ts.net:8643"
    config_dict["agents"]["Wellness"]["public_url"] = "https://aibox.example.ts.net:8642"
    return parse_config(config_dict)


def test_certificate_is_made_once_and_pinned_by_its_hash(lan_config):
    import os
    from bipbridge.lan import ensure_certificate, fingerprint, lan_info

    cert, key = ensure_certificate(lan_config)
    assert oct(os.stat(key).st_mode & 0o777) == "0o600"
    first = fingerprint(lan_config)
    assert first and len(first) == 64
    assert ensure_certificate(lan_config) == (cert, key) and fingerprint(lan_config) == first  # kept, not remade
    assert lan_info(lan_config) == {"url": "https://192.168.8.177:8650", "fingerprint": first}


def test_pairing_payload_carries_tailnet_addresses_keys_and_lan(lan_config):
    from bipbridge.lan import ensure_certificate, fingerprint, pairing_payload

    ensure_certificate(lan_config)
    payload = pairing_payload(lan_config, lan_config.agent("wellness"))
    assert payload["baseURL"] == "https://aibox.example.ts.net:8642" and payload["agent"] == "wellness"
    assert payload["apiKey"] == HERMES_KEY
    assert payload["bridgeURL"] == "https://aibox.example.ts.net:8643"
    assert payload["lan"] == {"url": "https://192.168.8.177:8650", "fingerprint": fingerprint(lan_config)}
    lan_config.agent("wellness").public_url = None
    with pytest.raises(ValueError, match="public_url"):
        pairing_payload(lan_config, lan_config.agent("wellness"))


def test_lan_off_by_default(config):
    from bipbridge.lan import lan_info
    assert not config.lan.enabled and lan_info(config) is None


def test_pairing_route_and_hermes_relay(lan_config):
    from fastapi.testclient import TestClient

    from bipbridge.app import create_app
    from bipbridge.lan import ensure_certificate, fingerprint

    ensure_certificate(lan_config)
    seen = []

    def hermes(request: httpx.Request) -> httpx.Response:
        seen.append(request)
        if request.url.path == "/v1/runs/run_1/events":
            return httpx.Response(200, headers={"content-type": "text/event-stream"},
                                  content=b"data: {\"event\": \"message.delta\", \"delta\": \"Hi\"}\n\n")
        return httpx.Response(201, json={"echo": json.loads(request.content or b"null")})

    app = create_app(lan_config, transport=httpx.MockTransport(hermes), start_background=False)
    with TestClient(app) as client:
        assert client.get("/v1/pairing").status_code == 401
        assert client.get("/v1/pairing", headers=AUTH).json() == {
            "lan": {"url": "https://192.168.8.177:8650", "fingerprint": fingerprint(lan_config)}}

        hermes_auth = {"Authorization": f"Bearer {HERMES_KEY}"}
        posted = client.post("/hermes/wellness/v1/runs?x=1", json={"input": "Salut"}, headers=hermes_auth)
        assert posted.status_code == 201 and posted.json() == {"echo": {"input": "Salut"}}
        assert str(seen[-1].url) == "http://hermes-wellness.test/v1/runs?x=1"
        assert seen[-1].headers["authorization"] == f"Bearer {HERMES_KEY}"  # Hermes checks its own key

        events = client.get("/hermes/wellness/v1/runs/run_1/events", headers=hermes_auth)
        assert events.headers["content-type"].startswith("text/event-stream") and b"message.delta" in events.content

        assert client.get("/hermes/nobody/v1/runs").status_code == 404
        assert client.get("/hermes/wellness/admin").status_code == 404  # only Hermes' API paths


def test_relay_absent_when_lan_off(client):
    assert client.get("/hermes/wellness/v1/capabilities").status_code == 404
