import asyncio

import pytest

from bipbridge.config import parse_config
from bipbridge import services
from bipbridge.services import ServiceError, add, catalog_of, listing, plan, remove
from bipbridge.vault import CannotHold


@pytest.fixture
def setup(config_dict, tmp_path):
    hub = tmp_path / ".hermes"
    (hub / "profiles" / "maison").mkdir(parents=True)
    config_dict["hermes"] = {"bin": "/opt/hermes", "home": str(hub), "hub_url": "http://127.0.0.1:8642"}
    config_dict["agents"]["Maison"] = {"hermes_url": "http://127.0.0.1:8642/p/maison", "hermes_key": "m" * 32}
    config = parse_config(config_dict)
    commands = []

    async def run(args, env):
        commands.append(list(args))
        if args[-1:] == ["ha"] and "test" in args:
            return "  Transport: HTTP\n  ✓ Connected (318ms)\n  ✓ Tools discovered: 7\n"
        return ""

    return config, config.agent("maison"), run, commands, hub / "profiles" / "maison"


def test_a_service_with_a_token_needs_the_agents_own_space(setup):
    config, agent, run, commands, folder = setup
    body = {"catalog": "home_assistant", "address": "homeassistant.local:8123", "token": "ha-secret"}
    assert plan(body)["url"] == "https://homeassistant.local:8123/api/mcp"
    with pytest.raises(CannotHold):
        asyncio.run(add(config, agent, body, run))
    assert commands == [] and not (folder / ".env").exists()
    # Without a token and not personal: fine in the shared hub.
    asyncio.run(add(config, agent, {"catalog": "custom", "name": "Météo", "address": "https://meteo.example/mcp"}, run))
    assert commands[-1] == ["/opt/hermes", "-p", "maison", "config", "set", "mcp_servers.meteo.url",
                            "https://meteo.example/mcp"]


def test_the_token_goes_to_the_env_file_never_on_a_command_line(setup):
    config, agent, run, commands, folder = setup
    agent.isolated = True
    agent.python = ["/opt/py"]
    agent.hermes_home = str(folder)
    result = asyncio.run(add(config, agent, {"catalog": "home_assistant", "name": "ha",
                                             "address": "http://ha.local:8123", "token": "ha-secret"}, run))
    assert result["catalog"] == "home_assistant"
    assert all("ha-secret" not in " ".join(c) for c in commands)
    assert "MCP_HOME_ASSISTANT_API_KEY=ha-secret" in (folder / ".env").read_text()
    assert oct((folder / ".env").stat().st_mode & 0o777) == "0o600"
    assert ["/opt/hermes", "-p", "maison", "config", "set", "mcp_servers.home_assistant.headers.Authorization",
            "Bearer ${MCP_HOME_ASSISTANT_API_KEY}"] in commands
    (folder / "config.yaml").write_text("mcp_servers:\n  ha:\n    url: http://ha.local:8123/api/mcp\n"
                                        "  garmin:\n    command: uvx\n")
    assert [(s["name"], s["catalog"]) for s in listing(config, agent)] == [("ha", "home_assistant"), ("garmin", None)]
    assert asyncio.run(services.test(config, agent, "ha", run)) == {"ok": True, "message": None, "tools": 7}
    asyncio.run(remove(config, agent, "garmin", run))
    assert commands[-1][-2:] == ["unset", "mcp_servers.garmin"]
    with pytest.raises(KeyError):
        asyncio.run(remove(config, agent, "nope", run))


def test_what_the_form_must_hold():
    with pytest.raises(ServiceError):
        plan({"catalog": "github"})
    with pytest.raises(ServiceError):
        plan({"catalog": "custom", "name": "x", "address": "ftp://x"})
    with pytest.raises(ServiceError):
        plan({"catalog": "nope"})
    assert plan({"catalog": "github", "token": "t"})["url"] == "https://api.githubcopilot.com/mcp"
    assert catalog_of("https://api.githubcopilot.com/mcp") == "github" and catalog_of("https://x.org/mcp") == "custom"
