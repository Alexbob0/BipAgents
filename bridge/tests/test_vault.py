import asyncio

import pytest

from bipbridge.config import parse_config
from bipbridge.vault import CannotHold, Vault, VaultError, normalize_origin


@pytest.fixture
def setup(config_dict, tmp_path):
    config_dict["agents"]["Vie"] = {"hermes_url": "http://127.0.0.1:8641", "hermes_key": "k" * 32, "isolated": True,
                                   "python": "podman exec -i hermes-vie /opt/hermes/venv/bin/python",
                                   "hermes_home": "/home/hermes/.hermes"}
    config_dict["agents"]["Budget"] = {"hermes_url": "http://127.0.0.1:8646/p/budget", "hermes_key": "b" * 32}
    config = parse_config(config_dict)
    calls = []

    async def run(agent, request):
        calls.append((agent.name, dict(request)))
        return {"id": f"vault_{len(calls)}"} if request["op"] == "add" else {"removed": True}

    return config, Vault(str(tmp_path / "vault"), run=run), calls


LOGIN = {"label": "SNCF", "site": "sncf-connect.com/login", "identifier": "alex@example.org",
         "password": "s3cret-Pass!", "otp_secret": "JBSWY3DPEHPK3PXP"}


def test_items_reach_only_the_agents_allowed_and_never_come_back(setup, tmp_path):
    config, vault, calls = setup
    item = asyncio.run(vault.create(config, {**LOGIN, "agents": ["vie"]}))
    assert item.origin == "https://sncf-connect.com" and item.has_otp and item.agents == {"vie": "vault_1"}
    name, request = calls[0]
    assert name == "vie" and request["op"] == "add" and request["kind"] == "login"
    assert request["secret"] == {"password": "s3cret-Pass!", "otp_secret": "JBSWY3DPEHPK3PXP",
                                 "identifier_type": "email", "identifier": "alex@example.org"}
    assert config.agent("vie").python[:3] == ["podman", "exec", "-i"]
    # The app only ever reads metadata; on disk, the secret is encrypted.
    public = vault.items()[0].public()
    assert "secret" not in public and "s3cret" not in str(public) and public["agents"] == ["vie"]
    assert b"s3cret" not in (tmp_path / "vault" / "vault.json.enc").read_bytes()
    assert oct((tmp_path / "vault" / "vault.key").stat().st_mode & 0o777) == "0o600"


def test_a_shared_hub_agent_cannot_hold_credentials(setup):
    config, vault, calls = setup
    with pytest.raises(CannotHold) as caught:
        asyncio.run(vault.create(config, {**LOGIN, "agents": ["vie", "budget"]}))
    assert caught.value.reasons == {"budget": "needs_own_space"}
    assert calls == [] and vault.items() == []


def test_update_rewrites_the_item_and_follows_the_agents(setup):
    config, vault, calls = setup
    item = asyncio.run(vault.create(config, {**LOGIN, "agents": ["vie"]}))
    calls.clear()
    # Only the agents change: nothing to rewrite for Vie.
    same = asyncio.run(vault.update(config, item.id, {"agents": ["vie"]}))
    assert calls == [] and same.agents == {"vie": "vault_1"}
    # A new password: taken back from Vie, then given again; the password left out of later edits is kept.
    asyncio.run(vault.update(config, item.id, {"password": "n3w"}))
    assert [(n, r["op"]) for n, r in calls] == [("vie", "remove"), ("vie", "add")]
    assert calls[1][1]["secret"]["password"] == "n3w"
    calls.clear()
    asyncio.run(vault.update(config, item.id, {"label": "SNCF Connect"}))
    assert calls[1][1]["secret"]["password"] == "n3w" and calls[1][1]["label"] == "SNCF Connect"
    calls.clear()
    asyncio.run(vault.update(config, item.id, {"agents": []}))
    assert [(n, r["op"]) for n, r in calls] == [("vie", "remove")] and vault.items()[0].agents == {}
    asyncio.run(vault.delete(config, item.id))
    assert vault.items() == []


def test_what_the_app_must_send(setup):
    config, vault, _ = setup
    for body in ({**LOGIN, "password": ""}, {**LOGIN, "identifier": " "}, {**LOGIN, "site": "ftp://x"},
                 {**LOGIN, "label": ""}, {**LOGIN, "kind": "ssh"}):
        with pytest.raises(VaultError):
            asyncio.run(vault.create(config, {**body, "agents": []}))
    assert normalize_origin("HTTPS://Example.com:443/a?b") == "https://example.com"
    assert normalize_origin("example.com:8443") == "https://example.com:8443"


def test_routes(client, config_dict):
    from conftest import AUTH

    listing = client.get("/v1/vault", headers=AUTH)
    assert listing.status_code == 200
    assert listing.json()["agents"] == [{"id": "wellness", "name": "Wellness", "can_hold": False,
                                         "why": "needs_own_space"}]
    refused = client.post("/v1/vault", headers=AUTH, json={**LOGIN, "agents": ["wellness"]})
    assert refused.status_code == 409 and refused.json()["detail"]["agents"] == {"wellness": "needs_own_space"}
    kept = client.post("/v1/vault", headers=AUTH, json={**LOGIN, "agents": []})
    assert kept.status_code == 201 and "password" not in kept.text
    assert client.delete(f"/v1/vault/{kept.json()['id']}", headers=AUTH).status_code == 204
    assert client.get("/v1/vault").status_code == 401
