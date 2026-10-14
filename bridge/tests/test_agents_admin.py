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


def test_thinking_level_is_set_in_the_agents_config(managed):
    """A level from none to xhigh: Hermes' `reasoning_effort` and the chat template's `enable_thinking` (bound to the
    model's address). Unset reads as Hermes' default. Only for a model on an OpenAI-compatible server of the person's."""
    from bipbridge.agents_admin import read_thinking, set_thinking

    config, _, tmp = managed
    agent = next(iter(config.agents.values()))
    folder = tmp / ".hermes" / "profiles" / agent.name
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "config.yaml").write_text("model:\n  provider: openrouter\n  default: x\n")
    assert read_thinking(config, agent) is None
    with pytest.raises(LookupError):
        set_thinking(config, agent, "high")
    (folder / "config.yaml").write_text("model:\n  provider: custom\n  default: glm\n  base_url: http://gpu:8888/v1\n"
                                        "agent:\n  max_turns: 60\n")
    assert read_thinking(config, agent) == "medium"
    assert set_thinking(config, agent, "xhigh") == "xhigh" and read_thinking(config, agent) == "xhigh"
    data = yaml.safe_load((folder / "config.yaml").read_text())
    assert "providers" not in data   # never enable_thinking on: it would win over a turn asking for none (Live)
    assert data["agent"] == {"max_turns": 60, "reasoning_effort": "xhigh"} and data["model"]["default"] == "glm"
    set_thinking(config, agent, "none")
    data = yaml.safe_load((folder / "config.yaml").read_text())
    assert data["agent"]["reasoning_effort"] == "none" and read_thinking(config, agent) == "none"
    assert data["providers"]["bipagents-thinking"] == {
        "api": "http://gpu:8888/v1", "extra_body": {"chat_template_kwargs": {"enable_thinking": False}}}
    set_thinking(config, agent, "medium")
    assert "providers" not in yaml.safe_load((folder / "config.yaml").read_text())
    with pytest.raises(LookupError):
        set_thinking(config, agent, "ultra")


def test_an_agent_set_up_by_hand_gets_its_personality_and_thinking_changed(config_dict, tmp_path):
    """Given its HERMES_HOME on the machine (`host_home`), an agent in a container of its own has its personality,
    language and thinking changed from the app; never its tools, its container or its deletion."""
    from bipbridge.agents_admin import (AgentSettings, editable, managed, read_settings, read_soul, read_thinking,
                                        set_language, set_thinking, write_settings)

    home = tmp_path / "home-vie"
    (home / "profiles" / "vie").mkdir(parents=True)
    (home / "profiles" / "vie" / "SOUL.md").write_text("Tu es Vie.\n")
    (home / "profiles" / "vie" / "config.yaml").write_text(
        "model:\n  provider: custom\n  default: dgx-spark\n  base_url: http://gpu:8888/v1\nagent:\n  reasoning_effort: medium\n")
    config_dict["agents"]["vie"] = {"display_name": "Vie", "hermes_url": "http://127.0.0.1:8644", "hermes_key": "k" * 32,
                                    "isolated": True, "host_home": str(home)}
    config = parse_config(config_dict)
    agent = editable(config, "vie")
    with pytest.raises(LookupError):
        managed(config, "vie")                     # no deleting, no space, no services from the app
    with pytest.raises(LookupError):
        editable(config, "wellness")               # set up by hand without its home: nothing changes

    async def go():
        async with httpx.AsyncClient(transport=httpx.MockTransport(lambda r: httpx.Response(500))) as http:
            return await read_settings(config, agent, http)   # its gateway isn't asked for its tools

    settings = asyncio.run(go())
    assert settings["soul"] == "Tu es Vie." and settings["by_hand"] and settings["thinking"] == "medium"
    set_language(config, agent, "fr")
    asyncio.run(write_settings(config, agent, AgentSettings(soul="Tu es Vie, brève.")))
    assert read_soul(config, agent).startswith("Tu es Vie, brève.") and "French" in read_soul(config, agent)
    with pytest.raises(ValueError):
        asyncio.run(write_settings(config, agent, AgentSettings(toolsets={"web": False})))
    assert set_thinking(config, agent, "high") == "high" and read_thinking(config, agent) == "high"


def test_rewriting_an_agents_section_keeps_what_the_bridge_does_not_set(tmp_path, config_dict):
    """After a move into its own space (or a browser added), the agent keeps its voice, ntfy topic, shares and a key
    kept in a file: only what the bridge sets is replaced."""
    from dataclasses import replace
    from bipbridge.agents_admin import save_agent

    key_file = tmp_path / "vie.key"
    key_file.write_text("k" * 32)
    path = tmp_path / "bridge.toml"
    path.write_text('bridge_key = "' + "b" * 40 + '"\n\n[pocket]\nurl = "http://p"\n\n[agents.vie]\n'
                    'display_name = "Vie"\nhermes_url = "http://127.0.0.1:9212/p/vie"\nhermes_key_file = "' + str(key_file) + '"\n'
                    '# her voice\nvoice = "pocket:colibri"\nntfy_topic = "vie-out"\nshares = "external"\n\n[agents.wellness]\n'
                    'display_name = "Wellness"\nhermes_url = "http://127.0.0.1:9212/p/wellness"\nhermes_key = "' + "w" * 32 + '"\n')
    import tomllib
    config = parse_config(tomllib.loads(path.read_text()), source_path=str(path))
    moved = replace(config.agent("vie"), hermes_url="http://127.0.0.1:8660/p/vie", space="/s/vie", isolated=True)
    save_agent(config, moved)
    reread = parse_config(tomllib.loads(path.read_text()), source_path=str(path))
    vie = reread.agent("vie")
    assert vie.voice == "pocket:colibri" and vie.ntfy_topic == "vie-out" and vie.shares == "external"
    assert vie.space == "/s/vie" and vie.isolated and vie.hermes_url.startswith("http://127.0.0.1:8660")
    assert vie.hermes_key == "k" * 32 and "hermes_key = " not in path.read_text().split("[agents.vie]")[1].split("[agents")[0]
    assert path.read_text().count("hermes_url") == 2 and reread.agent("wellness").hermes_key == "w" * 32



def test_thinking_forced_on_by_an_earlier_bridge_is_healed(managed):
    from bipbridge.agents_admin import heal_thinking, read_thinking

    config, _, tmp = managed
    agent = next(iter(config.agents.values()))
    agent.hermes_url = f"http://127.0.0.1:8642/p/{agent.name}"   # served by the hub: the bridge manages it
    folder = tmp / ".hermes" / "profiles" / agent.name
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "config.yaml").write_text(
        "model:\n  provider: custom\n  base_url: http://gpu:8888/v1\nagent:\n  reasoning_effort: xhigh\n"
        "providers:\n  bipagents-thinking:\n    api: http://gpu:8888/v1\n    extra_body:\n"
        "      chat_template_kwargs:\n        enable_thinking: true\n")
    heal_thinking(config)
    data = yaml.safe_load((folder / "config.yaml").read_text())
    assert "providers" not in data and read_thinking(config, agent) == "xhigh"
