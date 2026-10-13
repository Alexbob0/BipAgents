import asyncio
import json

import httpx
import pytest
import yaml

from bipbridge.config import parse_config
from conftest import AUTH


@pytest.fixture
def managed(config_dict, tmp_path):
    path = tmp_path / "bridge.toml"
    path.write_text("# bridge config\n")
    config_dict["hermes"] = {"bin": "/opt/hermes", "home": str(tmp_path / ".hermes"),
                             "hub_url": "http://127.0.0.1:8642", "public_hub_url": "https://mini.ts.net:8642"}
    config = parse_config(config_dict, source_path=str(path))
    return config, path, tmp_path


def fake_gateway(created):
    def handle(request: httpx.Request) -> httpx.Response:
        if request.url.path.endswith("/v1/capabilities"):
            return httpx.Response(200, json={"runs": True})
        if request.url.path.endswith("/api/sessions"):
            created.append((str(request.url), json.loads(request.content)))
            return httpx.Response(201, json={"id": "api_1", "title": "Bot Chat"})
        return httpx.Response(404)
    return httpx.MockTransport(handle)


def test_create_custom_agent(managed):
    from bipbridge.agents_admin import NewAgent, create_agent

    config, path, tmp = managed
    commands, sessions = [], []

    async def run(args, env):
        commands.append(list(args))
        return ""

    async def go():
        async with httpx.AsyncClient(transport=fake_gateway(sessions)) as http:
            body = NewAgent(name="Fougère", custom={"role": "suit mes plantes", "tone": "drôle", "category": "home"})
            return await create_agent(config, body, http, run=run)

    agent = asyncio.run(go())
    assert agent.name == "fougere" and agent.display_name == "Fougère"
    assert agent.hermes_url == "http://127.0.0.1:8642/p/fougere"
    assert agent.public_url == "https://mini.ts.net:8642/p/fougere"
    assert commands[0][:6] == ["/opt/hermes", "profile", "create", "fougere", "--clone-from", "default"]
    assert commands[1][:6] == ["/opt/hermes", "-p", "fougere", "config", "set", "API_SERVER_KEY"]
    # Nothing says this install's agents are isolated from the bridge: no terminal, files or code until turned on.
    assert commands[2] == ["/opt/hermes", "-p", "fougere", "tools", "disable", "terminal", "file", "code_execution",
                           "browser", "--platform", "api_server"]
    soul = (tmp / ".hermes" / "profiles" / "fougere" / "SOUL.md").read_text()
    assert soul.startswith("Tu es Fougère") and "suit mes plantes" in soul and "drôle" in soul
    assert "hermes-bots" in (tmp / ".hermes" / "profiles" / "fougere" / "profile.yaml").read_text()
    assert sessions == [("http://127.0.0.1:8642/p/fougere/api/sessions", {"title": "Bot Chat"})]
    assert config.agent("fougere") is agent
    reread = parse_config({**{"bridge_key": config.bridge_key, "pocket": {"url": "http://p"}},
                           **__import__("tomllib").loads(path.read_text())})
    assert reread.agent("fougere").hermes_key == agent.hermes_key   # survives a restart


def test_an_agent_created_in_english_thinks_in_english(managed):
    """The app sends its language: a ready-made kind's personality is written in it (French and English exist;
    another language gets the English one), and so is an « Autre » agent's."""
    from bipbridge.agents_admin import NewAgent, create_agent

    config, _, tmp = managed

    async def run(args, env):
        return ""

    async def go(body):
        async with httpx.AsyncClient(transport=fake_gateway([])) as http:
            return await create_agent(config, body, http, run=run)

    asyncio.run(go(NewAgent(name="Max", kind="finance", lang="en")))
    assert (tmp / ".hermes" / "profiles" / "max" / "SOUL.md").read_text().startswith("You are Max, your user's money agent")
    asyncio.run(go(NewAgent(name="Hans", kind="daily", lang="de")))
    assert (tmp / ".hermes" / "profiles" / "hans" / "SOUL.md").read_text().startswith("You are Hans")
    asyncio.run(go(NewAgent(name="Ivy", custom={"role": "tracks my plants", "category": "home"}, lang="en")))
    soul = (tmp / ".hermes" / "profiles" / "ivy" / "SOUL.md").read_text()
    assert soul.startswith("You are Ivy") and "tracks my plants" in soul and "write in English" in soul
    asyncio.run(go(NewAgent(name="Léo", kind="daily")))   # an app from before: French, as it was
    assert (tmp / ".hermes" / "profiles" / "leo" / "SOUL.md").read_text().startswith("Tu es Léo")


def test_kinds_in_the_apps_language(client):
    english = client.get("/v1/agent-kinds", params={"lang": "en"}, headers=AUTH).json()["kinds"]
    assert next(k for k in english if k["id"] == "finance")["label"] == "Money"
    french = client.get("/v1/agent-kinds", headers=AUTH).json()["kinds"]
    assert next(k for k in french if k["id"] == "finance")["label"] == "Finances"


def test_kinds_route_on_a_bridge_without_hermes(client):
    kinds = client.get("/v1/agent-kinds", headers=AUTH).json()
    assert kinds["can_create"] is False   # the test bridge does not manage a Hermes
    assert {k["id"] for k in kinds["kinds"]} >= {"daily", "finance", "custom"}
    assert next(k for k in kinds["kinds"] if k["id"] == "custom")["custom"] is True
    assert client.post("/v1/agents", json={"name": "Léa", "kind": "daily"}, headers=AUTH).status_code == 409
    assert client.post("/v1/agents", json={"name": "Léa", "kind": "daily"}).status_code == 401


def test_hermes_in_a_container(managed):
    from bipbridge.agents_admin import NewAgent, create_agent

    config, _, tmp = managed
    config.hermes.container_home = "/home/hermes/.hermes"

    async def run(args, env):
        return ""

    async def go():
        async with httpx.AsyncClient(transport=fake_gateway([])) as http:
            return await create_agent(config, NewAgent(name="Budget", kind="finance"), http, run=run)

    agent = asyncio.run(go())
    assert agent.upload_dir_host == str(tmp / ".hermes" / "profiles" / "budget" / "uploads")
    assert agent.upload_dir_container == "/home/hermes/.hermes/profiles/budget/uploads"


def test_refusals(managed):
    from bipbridge.agents_admin import NewAgent, create_agent

    config, _, _ = managed

    async def run(args, env):
        return ""

    async def attempt(body):
        async with httpx.AsyncClient(transport=fake_gateway([])) as http:
            return await create_agent(config, body, http, run=run)

    with pytest.raises(ValueError):
        asyncio.run(attempt(NewAgent(name="X", kind="nope")))
    with pytest.raises(ValueError):
        asyncio.run(attempt(NewAgent(name="X")))
    taken = asyncio.run(attempt(NewAgent(name="Wellness", kind="wellness")))
    assert taken.name == "wellness-2"   # « wellness » already exists in this bridge


def test_settings_and_delete(managed):
    from bipbridge.agents_admin import (AgentSettings, NewAgent, create_agent, delete_agent, managed as managed_agent,
                                        read_settings, write_settings)

    config, path, tmp = managed
    commands = []

    async def run(args, env):
        commands.append(list(args))
        return ""

    def gateway(request: httpx.Request) -> httpx.Response:
        if request.url.path.endswith("/v1/toolsets"):
            return httpx.Response(200, json={"data": [{"name": "web", "label": "Web", "description": "Search", "enabled": True,
                                                       "configured": True, "tools": ["web_search"]}]})
        if request.url.path.endswith("/v1/capabilities"):
            return httpx.Response(200, json={})
        return httpx.Response(201, json={})

    async def go():
        async with httpx.AsyncClient(transport=httpx.MockTransport(gateway)) as http:
            agent = await create_agent(config, NewAgent(name="Budget", kind="finance"), http, run=run)
            settings = await read_settings(config, agent, http)
            await write_settings(config, agent, AgentSettings(soul="Tu es Budget, sobre.", toolsets={"web": False, "terminal": True}), run=run)
            with pytest.raises(ValueError):
                await write_settings(config, agent, AgentSettings(toolsets={"web; rm -rf": True}), run=run)
            return agent, settings

    agent, settings = asyncio.run(go())
    assert settings["soul"].startswith("Tu es Budget") and settings["toolsets"][0]["name"] == "web"
    written = (tmp / ".hermes" / "profiles" / "budget" / "SOUL.md").read_text()
    assert written.startswith("Tu es Budget, sobre.\n") and "write in French." in written   # its language block kept
    assert ["/opt/hermes", "-p", "budget", "tools", "enable", "terminal", "--platform", "api_server"] in commands
    assert ["/opt/hermes", "-p", "budget", "tools", "disable", "web", "--platform", "api_server"] in commands
    with pytest.raises(LookupError):
        managed_agent(config, "wellness")   # set up by hand: never edited or deleted from the app
    assert "[agents.budget]" in path.read_text()
    asyncio.run(delete_agent(config, agent, run=run))
    assert commands[-1] == ["/opt/hermes", "profile", "delete", "budget", "--yes"]
    assert "[agents.budget]" not in path.read_text() and config.agent("budget") is None


def test_an_isolated_install_keeps_the_agents_tools(managed):
    from bipbridge.agents_admin import NewAgent, create_agent

    config, path, tmp = managed
    config.hermes.isolated = True
    commands, sessions = [], []

    async def run(args, env):
        commands.append(list(args))
        return ""

    async def go():
        async with httpx.AsyncClient(transport=fake_gateway(sessions)) as http:
            return await create_agent(config, NewAgent(name="Budget", kind="finance"), http, run=run)

    asyncio.run(go())
    assert not any("disable" in c for c in commands)


def test_profiles_created_on_the_server_can_be_claimed(managed):
    """A profile made with `hermes profile create` (not from the app) is offered to the app, then served: it keeps its
    own key when it has one, and nothing else of it changes."""
    from bipbridge.agents_admin import claim_profile, unclaimed_profiles

    config, path, tmp = managed
    profiles = tmp / ".hermes" / "profiles"
    (profiles / "cuisine").mkdir(parents=True)
    (profiles / "cuisine" / "profile.yaml").write_text("description: Recettes et menus\n")
    (profiles / "jardin").mkdir()
    (profiles / "jardin" / "SOUL.md").write_text("Tu es Jardin.")
    (profiles / "jardin" / ".env").write_text("API_SERVER_KEY=own-key\n")
    (profiles / "ancien").mkdir()
    (profiles / "ancien" / "config.yaml").write_text("{}")
    (profiles / ".deleted").mkdir()
    (profiles / ".deleted" / "ancien").write_text("deleted")          # Hermes' tombstone
    (profiles / "logs-only").mkdir()                                     # not a profile
    assert unclaimed_profiles(config) == [
        {"id": "cuisine", "name": "Cuisine", "description": "Recettes et menus"},
        {"id": "jardin", "name": "Jardin", "description": ""}]
    commands, sessions = [], []

    async def run(args, env):
        commands.append(list(args))
        return ""

    async def go(name, title):
        async with httpx.AsyncClient(transport=fake_gateway(sessions)) as http:
            return await claim_profile(config, name, title, http, run=run)

    jardin = asyncio.run(go("jardin", "Mon jardin"))
    assert jardin.hermes_key == "own-key" and commands == []            # its key kept, nothing written
    assert jardin.display_name == "Mon jardin" and jardin.public_url == "https://mini.ts.net:8642/p/jardin"
    cuisine = asyncio.run(go("cuisine", None))
    assert commands == [["/opt/hermes", "-p", "cuisine", "config", "set", "API_SERVER_KEY", cuisine.hermes_key]]
    assert [url for url, _ in sessions] == ["http://127.0.0.1:8642/p/jardin/api/sessions",
                                            "http://127.0.0.1:8642/p/cuisine/api/sessions"]   # their « Discussion »
    assert "hermes-bots" in (profiles / "cuisine" / "profile.yaml").read_text()          # Bot Mode
    assert unclaimed_profiles(config) == [] and "[agents.cuisine]" in path.read_text()
    with pytest.raises(LookupError):
        asyncio.run(go("ancien", None))


def test_agents_route_lists_unclaimed_profiles(client, tmp_path):
    services = client.app.state.services
    services.config.hermes.bin, services.config.hermes.hub_url = "/opt/hermes", "http://127.0.0.1:8642"
    services.config.hermes.home = str(tmp_path / ".hermes")
    (tmp_path / ".hermes" / "profiles" / "cuisine").mkdir(parents=True)
    (tmp_path / ".hermes" / "profiles" / "cuisine" / "SOUL.md").write_text("Tu es Cuisine.")
    listing = client.get("/v1/agents", headers=AUTH).json()
    assert listing["unclaimed"] == [{"id": "cuisine", "name": "Cuisine", "description": ""}]
    assert client.post("/v1/agents/claim", headers=AUTH, json={"id": "nope"}).status_code == 404


def test_the_language_is_changed_without_touching_the_personality(managed, client):
    """The app sets an agent's language at any time: only the bridge's language block changes; the person never sees
    it in the personality they edit, and editing that personality keeps it."""
    from bipbridge.agents_admin import AgentSettings, NewAgent, create_agent, read_soul, set_language, write_settings
    from bipbridge import souls

    config, _, tmp = managed

    async def run(args, env):
        return ""

    async def go():
        async with httpx.AsyncClient(transport=fake_gateway([])) as http:
            return await create_agent(config, NewAgent(name="Max", kind="daily", lang="en"), http, run=run)

    agent = asyncio.run(go())
    assert "write in English." in read_soul(config, agent)
    assert set_language(config, agent, "es-ES") == "es"
    soul = read_soul(config, agent)
    assert "write in Spanish." in soul and soul.startswith("You are Max") and soul.count(souls.BLOCK_START) == 1
    asyncio.run(write_settings(config, agent, AgentSettings(soul="You are Max, blunt and funny."), run=run))
    soul = read_soul(config, agent)
    assert soul.startswith("You are Max, blunt and funny.") and "write in Spanish." in soul
    body, lang = souls.split_language(soul)
    assert lang == "es" and souls.BLOCK_START not in body


def test_thinking_is_switched_in_the_agents_config(managed):
    """Thinking on: the chat template's `enable_thinking` (bound to the model's address) and a high effort; off: the
    template flag off and Hermes' default effort. Only for a model on an OpenAI-compatible server of the person's."""
    from bipbridge.agents_admin import read_thinking, set_thinking

    config, _, tmp = managed
    agent = next(iter(config.agents.values()))
    folder = tmp / ".hermes" / "profiles" / agent.name
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "config.yaml").write_text("model:\n  provider: openrouter\n  default: x\n")
    assert read_thinking(config, agent) is None
    with pytest.raises(LookupError):
        set_thinking(config, agent, True)
    (folder / "config.yaml").write_text("model:\n  provider: custom\n  default: glm\n  base_url: http://gpu:8888/v1\n"
                                        "agent:\n  max_turns: 60\n")
    assert read_thinking(config, agent) is False
    assert set_thinking(config, agent, True) is True and read_thinking(config, agent) is True
    data = yaml.safe_load((folder / "config.yaml").read_text())
    assert data["providers"]["bipagents-thinking"] == {
        "api": "http://gpu:8888/v1", "extra_body": {"chat_template_kwargs": {"enable_thinking": True}}}
    assert data["agent"] == {"max_turns": 60, "reasoning_effort": "xhigh"} and data["model"]["default"] == "glm"
    set_thinking(config, agent, False)
    data = yaml.safe_load((folder / "config.yaml").read_text())
    assert data["agent"] == {"max_turns": 60} and read_thinking(config, agent) is False
