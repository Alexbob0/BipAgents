import asyncio
import json

import httpx
import pytest

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
    assert commands[0][:5] == ["/opt/hermes", "profile", "create", "fougere", "--clone"]
    assert commands[1][:6] == ["/opt/hermes", "-p", "fougere", "config", "set", "API_SERVER_KEY"]
    soul = (tmp / ".hermes" / "profiles" / "fougere" / "SOUL.md").read_text()
    assert soul.startswith("Tu es Fougère") and "suit mes plantes" in soul and "drôle" in soul
    assert "hermes-bots" in (tmp / ".hermes" / "profiles" / "fougere" / "profile.yaml").read_text()
    assert sessions == [("http://127.0.0.1:8642/p/fougere/api/sessions", {"title": "Bot Chat"})]
    assert config.agent("fougere") is agent
    reread = parse_config({**{"bridge_key": config.bridge_key, "pocket": {"url": "http://p"}},
                           **__import__("tomllib").loads(path.read_text())})
    assert reread.agent("fougere").hermes_key == agent.hermes_key   # survives a restart


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
