import asyncio
import sqlite3

import httpx

from bipbridge.config import parse_config


def test_an_agent_moves_into_its_own_space_and_keeps_everything(config_dict, tmp_path):
    from bipbridge.agents_admin import delete_agent, managed, profile_folder, save_agent
    from bipbridge.spaces import Spaces
    from bipbridge.vault import holder

    hub = tmp_path / ".hermes"
    folder = hub / "profiles" / "budget"
    (folder / "memories").mkdir(parents=True)
    (folder / "memories" / "MEMORY.md").write_text("Aime les tableaux.")
    (folder / "SOUL.md").write_text("Tu es Budget.")
    (folder / "gateway.pid").write_text("123")                       # runtime files stay behind
    with sqlite3.connect(folder / "state.db") as db:
        db.execute("create table messages (content text)")
        db.execute("insert into messages values ('bonjour')")
    (hub / "config.yaml").write_text("model:\n  default: glm\n")
    (hub / ".env").write_text("OPENROUTER_API_KEY=or-key\nAPI_SERVER_PORT=8642\nAPI_SERVER_KEY=hub-key\n")
    config_path = tmp_path / "bridge.toml"
    config_path.write_text("# bridge\n")
    config_dict["public_url"] = "https://box.ts.net:8643"
    config_dict["hermes"] = {"bin": "/opt/hermes", "home": str(hub), "hub_url": "http://127.0.0.1:8642",
                             "python": "/opt/py -I", "root": "/opt/hermes-agent"}
    config_dict["spaces"] = {"root": str(tmp_path / "spaces"), "first_port": 18660, "sandbox": "none",
                             "start": "start-space {agent} {home}", "stop": "stop-space {agent}",
                             "container_home": "/home/hermes/.hermes"}
    config_dict["agents"]["Budget"] = {"hermes_url": "http://127.0.0.1:8642/p/budget", "hermes_key": "b" * 32}
    config = parse_config(config_dict, source_path=str(config_path))
    commands = []

    async def run(args, env):
        commands.append((list(args), dict(env)))
        return ""

    def gateway(request):
        assert request.headers["authorization"] == "Bearer " + "b" * 32   # the agent keeps its own key
        return httpx.Response(200, json={})

    async def go():
        async with httpx.AsyncClient(transport=httpx.MockTransport(gateway)) as http:
            return await Spaces(run).move(config, config.agent("budget"), http)

    moved = asyncio.run(go())
    space = tmp_path / "spaces" / "budget"
    assert moved.hermes_url == "http://127.0.0.1:18660/p/budget" and moved.isolated and moved.space == str(space)
    assert moved.public_url == "https://box.ts.net:8643/hermes/budget"   # through the bridge: no new tailnet port
    assert moved.python == ["/opt/py", "-I"] and moved.hermes_home == str(space / "profiles" / "budget")
    assert moved.upload_dir_host == str(space / "profiles" / "budget" / "uploads")
    assert moved.upload_dir_container == "/home/hermes/.hermes/profiles/budget/uploads"   # as the agent sees it
    env = (space / ".env").read_text()
    assert "OPENROUTER_API_KEY=or-key" in env and "API_SERVER_PORT=18660" in env and "hub-key" not in env
    assert (space / "profiles" / "budget" / "memories" / "MEMORY.md").read_text() == "Aime les tableaux."
    assert not (space / "profiles" / "budget" / "gateway.pid").exists()
    with sqlite3.connect(space / "profiles" / "budget" / "state.db") as db:
        assert db.execute("select content from messages").fetchall() == [("bonjour",)]
    assert commands[0] == (["/opt/hermes", "-p", "budget", "config", "set", "gateway.strict", "true"], {"HERMES_HOME": str(space)})
    assert commands[1][0][-2:] == ["browser.backend", "off"]
    assert commands[2][0][:2] == ["sh", "-c"] and "start-space budget" in commands[2][0][2]
    assert commands[-1] == (["/opt/hermes", "profile", "delete", "budget", "--yes"], {"HERMES_HOME": str(hub)})

    save_agent(config, moved)
    assert moved.sandboxed                                             # its own start command: a container
    assert holder(config, config.agent("budget")) is None             # may now hold credentials
    moved.sandboxed = False
    assert holder(config, moved) == "needs_sandbox"                    # same account as the hub, no sandbox
    moved.sandboxed = True
    assert managed(config, "budget") is moved and profile_folder(config, moved) == space / "profiles" / "budget"
    assert "space = " in config_path.read_text() and "isolated = true" in config_path.read_text()
    reread = parse_config({"bridge_key": config.bridge_key, "pocket": {"url": "http://p"},
                           **__import__("tomllib").loads(config_path.read_text())})
    assert reread.agent("budget").space == str(space) and reread.agent("budget").isolated

    commands.clear()
    asyncio.run(delete_agent(config, moved, run=run))
    assert "stop-space budget" in commands[0][0][2] and not space.exists() and config.agent("budget") is None
