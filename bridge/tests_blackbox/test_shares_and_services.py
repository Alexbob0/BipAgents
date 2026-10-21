"""Read-only data between agents (`/v1/agent-shares`, `bipbridge shares-mounts`) and the services (MCP servers)
connected to an agent from the app's catalog (`/v1/tool-catalog`, `/v1/agents/{id}/services…`) — tests/test_shares.py
and test_services.py, over HTTP; the profiles' changes go through the fake `hermes`."""
import json

from bbkit.common import AUTH
from conftest import managed_install, profile, wait_registered


def sandboxed(bridges, fakes):
    """vie and wellness run their tools in Hermes' docker sandbox, budget on this machine."""
    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness")
    hub = managed_install(bridge, fakes, agents=("vie", "wellness", "budget"), isolated=True)
    for name, terminal in (("vie", "docker"), ("wellness", "docker"), ("budget", "local")):
        volumes = '\n  docker_volumes:\n    - "/home/x/photos:/photos:ro"' if name == "vie" else ""
        profile(hub, name).joinpath("config.yaml").write_text(f"terminal:\n  backend: {terminal}{volumes}\n")
    return bridge, hub


def volumes_set(bridge, agent):
    return next(json.loads(args[5]) for args in reversed(bridge.commands.of("hermes"))
                if args[:5] == ["-p", agent, "config", "set", "terminal.docker_volumes"])


def test_an_agent_reads_what_another_shares_and_nothing_else(bridges, fakes):
    bridge, hub = sandboxed(bridges, fakes)
    bridge.start()
    wait_registered(bridge, ["vie", "wellness", "budget"])
    listing = bridge.get("/v1/agent-shares", headers=AUTH).json()
    assert listing == {"agents": [{"id": "vie", "name": "Vie", "why_not": None, "external": False},
                                  {"id": "wellness", "name": "Wellness", "why_not": None, "external": False},
                                  {"id": "budget", "name": "Budget", "why_not": "no_sandbox", "external": False}],
                       "reads": []}
    bridge.commands.clear()
    set_reads = bridge.put("/v1/agent-shares", headers=AUTH, json={"reads": [{"reader": "Vie", "source": "wellness"}]})
    assert set_reads.json() == {"reads": [{"reader": "vie", "source": "wellness"}]}
    root = bridge.state_dir / "shares"
    # Wellness writes in /partage; Vie reads it, read-only; the person's own mount is kept.
    assert volumes_set(bridge, "wellness") == [f"{root}/wellness:/partage:z"]
    assert volumes_set(bridge, "vie") == ["/home/x/photos:/photos:ro", f"{root}/wellness:/lecture/wellness:ro,z"]
    assert (root / "wellness").is_dir() and oct((root / "wellness").stat().st_mode & 0o777) == "0o777"
    assert (hub / "profiles" / "vie" / "skills" / "shared-data" / "SKILL.md").exists()
    assert json.loads((bridge.state_dir / "shares.json").read_text()) == {"reads": [["vie", "wellness"]]}
    assert bridge.get("/v1/agent-shares", headers=AUTH).json()["reads"] == [{"reader": "vie", "source": "wellness"}]
    # Taken back: the mounts go, the person's stays.
    bridge.commands.clear()
    bridge.put("/v1/agent-shares", headers=AUTH, json={"reads": []})
    assert volumes_set(bridge, "vie") == ["/home/x/photos:/photos:ro"] and volumes_set(bridge, "wellness") == []
    assert not (hub / "profiles" / "vie" / "skills" / "shared-data").exists()


def test_only_agents_in_a_sandbox_take_part(bridges, fakes):
    bridge, _ = sandboxed(bridges, fakes)
    bridge.start()
    wait_registered(bridge, ["vie", "wellness", "budget"])
    bridge.commands.clear()
    refused = bridge.put("/v1/agent-shares", headers=AUTH, json={"reads": [{"reader": "budget", "source": "wellness"}]})
    assert refused.status_code == 400 and refused.json() == {"detail": "budget: no_sandbox"}
    itself = bridge.put("/v1/agent-shares", headers=AUTH, json={"reads": [{"reader": "vie", "source": "vie"}]})
    assert itself.status_code == 400 and itself.json() == {"detail": "an agent always reads its own files"}
    ghost = bridge.put("/v1/agent-shares", headers=AUTH, json={"reads": [{"reader": "vie", "source": "ghost"}]})
    assert ghost.json() == {"detail": "unknown agent ghost"}
    assert bridge.commands.of("hermes") == [] and not (bridge.state_dir / "shares.json").exists()


def test_routes_on_a_bridge_without_hermes(bridge):
    listing = bridge.get("/v1/agent-shares", headers=AUTH).json()
    assert listing == {"agents": [{"id": "wellness", "name": "Wellness", "why_not": "not_managed", "external": False}],
                       "reads": []}
    assert bridge.put("/v1/agent-shares", headers=AUTH, json={"reads": [{"reader": "wellness", "source": "x"}]}
                      ).status_code == 400


def test_deleting_an_agent_stops_the_others_reading_it(bridges, fakes):
    bridge, hub = sandboxed(bridges, fakes)
    bridge.start()
    wait_registered(bridge, ["vie", "wellness", "budget"])
    bridge.put("/v1/agent-shares", headers=AUTH, json={"reads": [{"reader": "vie", "source": "wellness"}]})
    bridge.commands.clear()
    assert bridge.delete("/v1/agents/wellness", headers=AUTH).status_code == 204
    assert volumes_set(bridge, "vie") == ["/home/x/photos:/photos:ro"]
    assert json.loads((bridge.state_dir / "shares.json").read_text()) == {"reads": []}
    assert not (bridge.state_dir / "shares" / "wellness").exists()


def test_an_agent_set_up_by_hand_mounts_its_folders_itself(bridges, fakes):
    """Vie in a container of its own: the bridge writes nothing in it, its start script mounts what `shares-mounts`
    lists, and the administrator's command runs when that changed."""
    bridge, hub = sandboxed(bridges, fakes)
    flag = bridge.root / "applied.txt"
    bridge.config["agents"]["vie"] = {"hermes_url": fakes.down.url, "hermes_key": "v" * 32, "shares": "external",
                                      "shares_apply": f"echo {{agent}} >> {flag}"}
    bridge.start()
    wait_registered(bridge, ["wellness", "budget"])
    assert bridge.get("/v1/agent-shares", headers=AUTH).json()["agents"][0] == {
        "id": "vie", "name": "Vie", "why_not": None, "external": True}
    bridge.commands.clear()
    bridge.put("/v1/agent-shares", headers=AUTH, json={"reads": [{"reader": "vie", "source": "wellness"}]})
    root = bridge.state_dir / "shares"
    out = bridge.cli("shares-mounts", "--agent", "vie")
    assert out.returncode == 0 and out.stdout == f"-v\n{root}/wellness:/lecture/wellness:ro,z\n"
    assert bridge.cli("shares-mounts", "--agent", "wellness").stdout == f"-v\n{root}/wellness:/partage:z\n"
    assert bridge.cli("shares-mounts").returncode == 2
    assert flag.read_text() == "vie\n"
    assert not any(args[:2] == ["-p", "vie"] for args in bridge.commands.of("hermes"))   # nothing written in its config
    bridge.put("/v1/agent-shares", headers=AUTH, json={"reads": [{"reader": "vie", "source": "wellness"}]})
    assert flag.read_text() == "vie\n"                                                    # unchanged: not run again


# -- connected services -----------------------------------------------------------------------------------------------

def maison(bridges, fakes, isolated=False):
    bridge = bridges.new()
    hub = managed_install(bridge, fakes, agents=("maison",), isolated=True)
    folder = profile(hub, "maison")
    if isolated:
        bridge.config["agents"]["maison"].update(isolated=True, python=["/opt/py"], hermes_home=str(folder))
    bridge.start()
    wait_registered(bridge, ["maison"])
    return bridge, folder


def test_the_catalog(bridge):
    services = bridge.get("/v1/tool-catalog", headers=AUTH).json()["services"]
    assert [s["id"] for s in services] == ["home_assistant", "github", "custom"]
    assert set(services[0]) == {"id", "label", "description", "icon", "personal", "fields"}
    assert services[0]["fields"][1] == {"id": "token", "label": "Jeton d'accès longue durée", "secret": True}
    assert bridge.get("/v1/tool-catalog").status_code == 401


def test_a_service_with_a_token_needs_the_agents_own_space(bridges, fakes):
    bridge, folder = maison(bridges, fakes)
    body = {"catalog": "home_assistant", "address": "homeassistant.local:8123", "token": "ha-secret"}
    bridge.commands.clear()
    refused = bridge.post("/v1/agents/maison/services", headers=AUTH, json=body)
    assert refused.status_code == 409 and refused.json() == {"detail": {"code": "cannot_hold",
                                                                         "agents": {"maison": "needs_own_space"}}}
    assert bridge.commands.of("hermes") == [] and not (folder / ".env").exists()
    # Without a token and not personal: fine in the shared hub.
    added = bridge.post("/v1/agents/maison/services", headers=AUTH,
                        json={"catalog": "custom", "name": "Météo", "address": "https://meteo.example/mcp"})
    assert added.status_code == 201
    assert added.json() == {"name": "meteo", "url": "https://meteo.example/mcp", "enabled": True, "catalog": "custom"}
    commands = bridge.commands.of("hermes")
    assert commands[-2] == ["-p", "maison", "config", "set", "mcp_servers.meteo.url", "https://meteo.example/mcp"]
    # Its tools that write ask the person first (what the agent read cannot make it act through the service).
    assert commands[-1] == ["-p", "maison", "config", "set", "mcp_servers.meteo.trust", "untrusted"]
    twice = bridge.post("/v1/agents/maison/services", headers=AUTH,
                        json={"catalog": "custom", "name": "Météo", "address": "https://meteo.example/mcp"})
    assert twice.status_code == 400 and twice.json() == {"detail": "a service with this name already exists"}
    assert bridge.post("/v1/agents/wellness/services", headers=AUTH, json=body).status_code == 404


def test_the_token_goes_to_the_env_file_never_on_a_command_line(bridges, fakes):
    bridge, folder = maison(bridges, fakes, isolated=True)
    bridge.commands.clear()
    result = bridge.post("/v1/agents/maison/services", headers=AUTH, json={
        "catalog": "home_assistant", "name": "ha", "address": "http://ha.local:8123", "token": "ha-secret"})
    assert result.status_code == 201 and result.json()["catalog"] == "home_assistant"
    assert result.json()["url"] == "http://ha.local:8123/api/mcp"
    commands = bridge.commands.of("hermes")
    assert all("ha-secret" not in " ".join(c) for c in commands)
    assert "MCP_HOME_ASSISTANT_API_KEY=ha-secret" in (folder / ".env").read_text()   # its catalog id: no name field
    assert oct((folder / ".env").stat().st_mode & 0o777) == "0o600"
    assert ["-p", "maison", "config", "set", "mcp_servers.home_assistant.headers.Authorization",
            "Bearer ${MCP_HOME_ASSISTANT_API_KEY}"] in commands
    assert not any("trust" in c[4] for c in commands)                 # Home Assistant stays direct
    (folder / "config.yaml").write_text("mcp_servers:\n  ha:\n    url: http://ha.local:8123/api/mcp\n"
                                        "  garmin:\n    command: uvx\n")
    settings = bridge.get("/v1/agents/maison/settings", headers=AUTH).json()
    assert settings["services"] == [{"name": "ha", "url": "http://ha.local:8123/api/mcp", "command": None, "enabled": True,
                                     "catalog": "home_assistant"},
                                    {"name": "garmin", "url": None, "command": "uvx", "enabled": True, "catalog": None}]
    bridge.commands.rule("hermes", ["-p", "maison", "mcp", "test", "ha"],
                         stdout="  Transport: HTTP\n  ✓ Connected (318ms)\n  ✓ Tools discovered: 7\n")
    tested = bridge.post("/v1/agents/maison/services/ha/test", headers=AUTH)
    assert tested.json() == {"ok": True, "message": None, "tools": 7}
    bridge.commands.rule("hermes", ["-p", "maison", "mcp", "test", "garmin"],
                         stdout="  ✗ Connection refused: uvx not found\n")
    assert bridge.post("/v1/agents/maison/services/garmin/test", headers=AUTH).json() == {
        "ok": False, "message": "Connection refused: uvx not found", "tools": None}
    bridge.commands.clear_rules()
    bridge.commands.rule("hermes", ["-p", "maison", "mcp", "test"], exit=1, stderr="boom")
    failed = bridge.post("/v1/agents/maison/services/ha/test", headers=AUTH).json()
    assert failed["ok"] is False and failed["tools"] is None and "boom" in failed["message"]
    removed = bridge.delete("/v1/agents/maison/services/garmin", headers=AUTH)
    assert removed.status_code == 204
    assert bridge.commands.of("hermes")[-1] == ["-p", "maison", "config", "unset", "mcp_servers.garmin"]
    assert bridge.delete("/v1/agents/maison/services/nope", headers=AUTH).status_code == 404
    assert bridge.post("/v1/agents/maison/services/nope/test", headers=AUTH).status_code == 404


def test_what_the_form_must_hold(bridges, fakes):
    bridge, _ = maison(bridges, fakes, isolated=True)
    for body, detail in (({"catalog": "github"}, "Jeton personnel GitHub is required"),
                         ({"catalog": "custom", "name": "x", "address": "ftp://x"}, "the address must be a web address"),
                         ({"catalog": "nope"}, "unknown service")):
        resp = bridge.post("/v1/agents/maison/services", headers=AUTH, json=body)
        assert resp.status_code == 400 and resp.json() == {"detail": detail}, body
    github = bridge.post("/v1/agents/maison/services", headers=AUTH, json={"catalog": "github", "token": "t"})
    assert github.json()["url"] == "https://api.githubcopilot.com/mcp"
