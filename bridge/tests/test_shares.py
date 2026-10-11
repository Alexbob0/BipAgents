import asyncio
import json

import pytest

from bipbridge import shares
from bipbridge.config import parse_config
from conftest import AUTH


@pytest.fixture
def setup(config_dict, tmp_path):
    hub = tmp_path / ".hermes"
    for name, terminal in (("vie", "docker"), ("wellness", "docker"), ("budget", "local")):
        folder = hub / "profiles" / name
        folder.mkdir(parents=True)
        volumes = '\n  docker_volumes:\n    - "/home/x/photos:/photos:ro"' if name == "vie" else ""
        (folder / "config.yaml").write_text(f"terminal:\n  backend: {terminal}{volumes}\n")
    config_dict["hermes"] = {"bin": "/opt/hermes", "home": str(hub), "hub_url": "http://127.0.0.1:8642",
                             "isolated": True}
    config_dict["outbox"] = {"db_path": str(tmp_path / "data" / "bridge.db")}
    for name in ("vie", "wellness", "budget"):
        config_dict["agents"][name.title()] = {"hermes_url": f"http://127.0.0.1:8642/p/{name}", "hermes_key": "k" * 32}
    config = parse_config(config_dict)
    commands = []

    async def run(args, env):
        commands.append((list(args), dict(env)))
        return ""

    return config, hub, run, commands, tmp_path


def volumes_set(commands, agent):
    return next(json.loads(args[6]) for args, _ in reversed(commands)
                if args[1:6] == ["-p", agent, "config", "set", "terminal.docker_volumes"])


def test_an_agent_reads_what_another_shares_and_nothing_else(setup):
    config, hub, run, commands, tmp = setup
    root = tmp / "data" / "shares"
    asyncio.run(shares.apply(config, [("vie", "wellness")], run))
    # Wellness writes in /partage; Vie reads it, read-only; the person's own mount is kept.
    assert volumes_set(commands, "wellness") == [f"{root}/wellness:/partage"]
    assert volumes_set(commands, "vie") == ["/home/x/photos:/photos:ro", f"{root}/wellness:/lecture/wellness:ro"]
    assert (root / "wellness").is_dir() and oct((root / "wellness").stat().st_mode & 0o777) == "0o700"
    assert (hub / "profiles" / "vie" / "skills" / "shared-data" / "SKILL.md").exists()
    assert shares.load(config) == [("vie", "wellness")]
    # Taken back: the mounts go, the person's stays.
    commands.clear()
    asyncio.run(shares.apply(config, [], run))
    assert volumes_set(commands, "vie") == ["/home/x/photos:/photos:ro"] and volumes_set(commands, "wellness") == []
    assert not (hub / "profiles" / "vie" / "skills" / "shared-data").exists()


def test_only_agents_in_a_sandbox_take_part(setup):
    config, hub, run, commands, tmp = setup
    assert shares.why_not(config, config.agent("budget")) == "no_sandbox"
    with pytest.raises(ValueError):
        asyncio.run(shares.apply(config, [("budget", "wellness")], run))
    with pytest.raises(ValueError):
        asyncio.run(shares.apply(config, [("vie", "vie")], run))
    assert commands == [] and shares.load(config) == []


def test_deleting_an_agent_stops_the_others_reading_it(setup):
    config, hub, run, commands, tmp = setup
    asyncio.run(shares.apply(config, [("vie", "wellness")], run))
    config.agents.pop("wellness")
    commands.clear()
    asyncio.run(shares.forget(config, "wellness", run))
    assert volumes_set(commands, "vie") == ["/home/x/photos:/photos:ro"]
    assert shares.load(config) == [] and not (tmp / "data" / "shares" / "wellness").exists()


def test_routes(client):
    listing = client.get("/v1/agent-shares", headers=AUTH).json()
    assert listing == {"agents": [{"id": "wellness", "name": "Wellness", "why_not": "not_managed"}], "reads": []}
    assert client.put("/v1/agent-shares", headers=AUTH, json={"reads": [{"reader": "wellness", "source": "x"}]}
                      ).status_code == 400
