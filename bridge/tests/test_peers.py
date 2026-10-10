import asyncio
import json

import pytest

from bipbridge.config import parse_config
from bipbridge.peers import Links, apply, gateway_of, manual_token, sender_of


@pytest.fixture
def setup(config_dict, tmp_path):
    hub = tmp_path / ".hermes"
    space = tmp_path / "spaces" / "jardin"
    for folder in (hub / "profiles" / "budget", hub / "profiles" / "maison", space / "profiles" / "jardin"):
        folder.mkdir(parents=True)
    config_dict["hermes"] = {"bin": "/opt/hermes", "home": str(hub), "hub_url": "http://127.0.0.1:8642"}
    config_dict["agents"]["Budget"] = {"hermes_url": "http://127.0.0.1:8642/p/budget", "hermes_key": "b" * 32}
    config_dict["agents"]["Maison"] = {"hermes_url": "http://127.0.0.1:8642/p/maison", "hermes_key": "m" * 32}
    config_dict["agents"]["Jardin"] = {"hermes_url": "http://127.0.0.1:8660/p/jardin", "hermes_key": "j" * 32,
                                       "isolated": True, "sandboxed": True, "space": str(space)}
    config = parse_config(config_dict)
    commands = []

    async def run(args, env):
        commands.append((list(args), env))
        return ""

    return config, Links(str(tmp_path / "peers.json")), run, commands, hub, space


def test_each_sending_gateway_gets_a_token_in_its_own_env(setup):
    config, links, run, commands, hub, space = setup
    result = asyncio.run(apply(config, links, [("budget", "jardin"), ("jardin", "wellness")], run))
    assert result["manual"] == {}
    hub_token = (hub / ".env").read_text().split("HERMES_PEER_BIP_KEY=")[1].strip()
    space_token = (space / ".env").read_text().split("HERMES_PEER_BIP_KEY=")[1].strip()
    assert links.caller(hub_token) == str(hub) and links.caller(space_token) == str(space)
    assert hub_token not in links.path.read_text()                 # only digests are kept
    assert (["/opt/hermes", "config", "set", "bot_peers.bip.url", "http://127.0.0.1:8643/peer"],
            {"HERMES_HOME": str(space)}) in commands
    assert links.allowed("jardin", "wellness") and not links.allowed("wellness", "jardin")
    # Wellness (set up by hand) sends: the administrator gives it a token; the hub sends nothing any more.
    result = asyncio.run(apply(config, links, [("wellness", "budget")], run))
    assert result["manual"] == {"wellness": ""}
    assert "HERMES_PEER_BIP_KEY" not in (hub / ".env").read_text() and links.caller(hub_token) is None
    token = manual_token(config, links, "wellness")
    assert links.caller(token) == "manual:wellness" == gateway_of(config, config.agent("wellness"))
    with pytest.raises(ValueError):
        asyncio.run(apply(config, links, [("budget", "budget")], run))


def test_the_sender_is_the_agent_that_signed_from_that_gateway(setup):
    config, links, run, commands, hub, space = setup
    signed = lambda handle: json.dumps({"message": f"Message from 🤖 Budget (@{handle}): bonjour"}).encode()
    assert sender_of(config, str(hub), signed("budget")) == "budget"
    assert sender_of(config, str(hub), signed("jardin")) is None     # jardin is not served by the hub
    assert sender_of(config, str(space), signed("budget")) is None
    assert sender_of(config, str(hub), b'{"message": "no signature"}') is None


def test_the_bridge_refuses_what_the_person_did_not_allow(client):
    from conftest import AUTH
    services = client.app.state.services
    token = manual_token(services.config, services.links, "wellness")
    assert client.get("/peer/p/wellness/api/sessions", headers={"Authorization": "Bearer nope"}).status_code == 401
    refused = client.get("/peer/p/wellness/api/sessions", headers={"Authorization": f"Bearer {token}"})
    assert refused.status_code == 403
    assert client.get("/v1/agent-links", headers=AUTH).json()["agents"][0] == {"id": "wellness", "name": "Wellness",
                                                                               "managed": False}
