import asyncio
import base64
import json
import secrets

import httpx
import pytest

from bipbridge.cloud_setup import CLAIM_CONTEXT, SETUP_CONTEXT, proof
from bipbridge.config import parse_config
from conftest import BRIDGE_KEY

SECRET = bytes(range(32))
NONCE = "ab" * 16


@pytest.fixture
def cloud_dict(config_dict, tmp_path):
    setup = tmp_path / "setup.json"
    setup.write_text(json.dumps({"pair": base64.urlsafe_b64encode(SECRET).decode().rstrip("=")}))
    path = tmp_path / "bridge.toml"
    path.write_text("# bridge config\n")
    config_dict.update({
        "cloud": True, "public_url": "https://203.0.113.7",
        "lan": {"enabled": True, "address": "203.0.113.7", "port": 443, "cert_dir": str(tmp_path / "lan")},
        "setup": {"file": str(setup), "progress": str(tmp_path / "progress.json")},
        "hermes": {"bin": "/opt/hermes", "home": str(tmp_path / ".hermes"), "hub_url": "http://127.0.0.1:8642"},
    })
    config_dict["_path"] = str(path)
    return config_dict


@pytest.fixture
def cloud(cloud_dict):
    from bipbridge.lan import ensure_certificate
    path = cloud_dict.pop("_path")
    config = parse_config(cloud_dict, source_path=path)
    ensure_certificate(config)
    return config


def done(config, flag=True):
    with open(config.setup_progress, "w") as fh:
        json.dump({"step": 6, "total": 6, "label": "Checks", "done": flag}, fh)


@pytest.fixture
def client(cloud, backend, apns, monkeypatch):
    from fastapi.testclient import TestClient

    from bipbridge import cloud_setup
    from bipbridge.app import create_app
    from bipbridge.guard import Guard

    async def fake_claim(config, request, http, run=None):
        fake_claim.requests.append(request)
        return [{"id": "quotidien", "name": request.agents[0].name}]
    fake_claim.requests = []
    monkeypatch.setattr(cloud_setup, "claim_server", fake_claim)
    app = create_app(cloud, transport=backend.transport(), apns_transport=apns.transport())
    app.state.guard = Guard(limit=3)
    with TestClient(app) as test_client:
        test_client.claims = fake_claim.requests   # type: ignore[attr-defined]
        yield test_client
        backend.release.set()


def claim_body(challenge, request=None, secret=SECRET):
    request = request or json.dumps({"model": {"provider": "custom", "name": "glm-5.3", "key": "sk-eu-0123456789",
                                               "base_url": "https://api.eurouter.example/v1"},
                                     "agents": [{"name": "Quotidien", "kind": "daily"}], "language": "fr"})
    return {"challenge": challenge, "mac": proof(secret, CLAIM_CONTEXT, challenge, request), "request": request}


def test_the_server_proves_its_certificate_before_anything(client, cloud):
    from bipbridge.lan import fingerprint

    state = client.get("/v1/setup", params={"nonce": NONCE}).json()
    assert state["state"] == "installing" and state["fingerprint"] == fingerprint(cloud)
    # Only a server that knows the secret from the user-data can make this proof for this certificate.
    assert state["proof"] == proof(SECRET, SETUP_CONTEXT, NONCE, state["fingerprint"], state["challenge"])
    assert state["proof"] != proof(b"x" * 32, SETUP_CONTEXT, NONCE, state["fingerprint"], state["challenge"])
    done(cloud)
    assert client.get("/v1/setup", params={"nonce": NONCE}).json()["state"] == "ready"
    assert client.get("/v1/setup", params={"nonce": "short"}).status_code == 422


def test_a_claim_with_the_secret_pairs_the_app_once(client, cloud):
    from bipbridge.lan import fingerprint

    done(cloud)
    challenge = client.get("/v1/setup", params={"nonce": NONCE}).json()["challenge"]
    paired = client.post("/v1/setup/claim", json=claim_body(challenge))
    assert paired.status_code == 200, paired.text
    assert paired.json() == {"bridgeURL": "https://203.0.113.7", "bridgeKey": BRIDGE_KEY,
                             "fingerprint": fingerprint(cloud), "agents": [{"id": "quotidien", "name": "Quotidien"}]}
    assert client.claims[0].model.key == "sk-eu-0123456789"
    # The secret is gone: the server says so, and a second claim, even with the same proof, is refused.
    assert client.get("/v1/setup", params={"nonce": NONCE}).json() == {"state": "claimed"}
    assert client.post("/v1/setup/claim", json=claim_body(challenge)).status_code == 410


def test_challenges_are_single_use_and_claims_wait_for_the_install(client, cloud):
    challenge = client.get("/v1/setup", params={"nonce": NONCE}).json()["challenge"]
    assert client.post("/v1/setup/claim", json=claim_body(challenge)).status_code == 409   # still installing
    done(cloud)
    assert client.post("/v1/setup/claim", json=claim_body(challenge)).status_code == 400   # used up by the 409
    fresh = client.get("/v1/setup", params={"nonce": NONCE}).json()["challenge"]
    bad = json.dumps({"model": {"provider": "custom", "name": "x", "key": "sk-0123456789"}, "agents": []})
    assert client.post("/v1/setup/claim", json=claim_body(fresh, bad)).status_code == 400   # no base_url
    assert client.claims == []


def test_wrong_proofs_get_the_address_blocked(client, cloud):
    done(cloud)
    for _ in range(3):
        challenge = client.get("/v1/setup", params={"nonce": NONCE}).json()["challenge"]
        assert client.post("/v1/setup/claim", json=claim_body(challenge, secret=b"y" * 32)).status_code == 401
    assert client.get("/v1/setup", params={"nonce": NONCE}).status_code == 429
    assert client.claims == []


def test_wrong_bridge_keys_get_the_address_blocked_and_the_console_is_off(client):
    for _ in range(3):
        assert client.get("/v1/agents", headers={"Authorization": "Bearer nope"}).status_code == 401
    # Blocked for a while, even with the right key; requests without a key don't count.
    assert client.get("/v1/agents", headers={"Authorization": f"Bearer {BRIDGE_KEY}"}).status_code == 429
    assert client.get("/console/").status_code == 404


def test_claiming_sets_the_model_and_creates_agents_reached_through_the_door(cloud, tmp_path):
    from bipbridge.cloud_setup import ClaimRequest, claim_server, secret_of

    commands = []

    async def run(args, env):
        commands.append(list(args))
        if args[1:3] == ["profile", "create"] and args[3] == "maison":
            raise RuntimeError("disk full")   # the second agent fails the first time
        return ""

    def gateway(request):
        if request.url.path.endswith("/v1/capabilities"):
            return httpx.Response(200, json={})
        return httpx.Response(201, json={"id": "s1"})

    wanted = ClaimRequest.model_validate({"model": {"provider": "openrouter", "name": "z-ai/glm-5.3", "key": "sk-or-0123456789",
                                                    "vision": True},
                                          "agents": [{"name": "Quotidien", "kind": "daily"}, {"name": "Maison", "kind": "home"}],
                                          "language": "fr"})

    async def go():
        async with httpx.AsyncClient(transport=httpx.MockTransport(gateway)) as http:
            return await claim_server(cloud, wanted, http, run=run)

    with pytest.raises(RuntimeError):
        asyncio.run(go())
    hermes = "/opt/hermes"
    assert [hermes, "config", "set", "model.provider", "openrouter"] in commands
    assert [hermes, "config", "set", "model.base_url", "https://openrouter.ai/api/v1"] in commands
    assert [hermes, "config", "set", "OPENROUTER_API_KEY", "sk-or-0123456789"] in commands
    assert [hermes, "config", "set", "model.supports_vision", "true"] in commands
    assert [hermes, "gateway", "restart"] in commands
    assert cloud.agent("quotidien").public_url == "https://203.0.113.7/hermes/quotidien"
    toml = open(cloud.source_path).read()
    assert '[model]\nhealth_url = "https://openrouter.ai/api/v1/models"\nhealth_key = "sk-or-0123456789"' in toml
    assert secret_of(cloud) == SECRET   # not paired yet: the app may try again
    # Again: the model is not set twice, Quotidien is not made twice, Maison is made this time.
    commands.clear()

    async def run_again(args, env):
        commands.append(list(args))
        return ""

    async def again():
        async with httpx.AsyncClient(transport=httpx.MockTransport(gateway)) as http:
            return await claim_server(cloud, wanted, http, run=run_again)

    agents = asyncio.run(again())
    assert agents == [{"id": "quotidien", "name": "Quotidien"}, {"id": "maison", "name": "Maison"}]
    assert not any(c[1:3] == ["config", "set"] for c in commands)
    assert sum(1 for c in commands if c[1:3] == ["profile", "create"]) == 1
