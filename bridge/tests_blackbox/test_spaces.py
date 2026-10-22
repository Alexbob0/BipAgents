"""An agent moved from the shared hub into a space of its own (`POST /v1/agents/{id}/space`), started again by
`bipbridge spaces start`, deleted with its space; and `bipbridge adopt` serving what an export brought to a place.
`[spaces] start` / `stop` are the administrator's commands here: the fake `fake-gateway` serves the space's Hermes on
the port the bridge chose (tests/test_spaces.py's ground, over HTTP and the CLI)."""
import json
import os
import tomllib

import yaml

from bbkit.common import AUTH
from bbkit.servers import free_port
from conftest import managed_install, profile, wait_registered


def with_spaces(bridges, fakes):
    bridge = bridges.new()
    hub = managed_install(bridge, fakes, agents=("coach",))
    (hub / "config.yaml").write_text("model:\n  provider: custom\n  default: glm\n")
    (hub / ".env").write_text("CUSTOM_MODEL_API_KEY=sk-hub\n")
    folder = profile(hub, "coach")
    (folder / "SOUL.md").write_text("Tu es Coach.\n")
    (folder / "memories").mkdir()
    (folder / "memories" / "MEMORY.md").write_text("Alex court.")
    (folder / "gateway.pid").write_text("123")          # a running gateway's file: never copied
    marker = bridge.root / "stopped.txt"
    bridge.config["spaces"] = {"root": str(bridge.root / "spaces"), "first_port": free_port(),
                               "start": f"{bridge.fakebin}/fake-gateway {{home}}",
                               "stop": f"echo {{agent}} >> {marker}"}
    bridge.start()
    wait_registered(bridge, ["coach"])
    return bridge, hub, marker


def test_an_agent_moves_into_its_own_space(bridges, fakes):
    bridge, hub, marker = with_spaces(bridges, fakes)
    bridge.commands.clear()
    moved = bridge.post("/v1/agents/coach/space", headers=AUTH)
    assert moved.status_code == 200, moved.text
    assert moved.json() == {"id": "coach", "name": "Coach", "url": None, "key": None, "isolated": True}
    root = bridge.root / "spaces" / "coach"
    env = dict(line.split("=", 1) for line in (root / ".env").read_text().splitlines())
    port = int(env["API_SERVER_PORT"])
    assert env["API_SERVER_ENABLED"] == "true" and env["API_SERVER_HOST"] == "127.0.0.1"
    assert env["CUSTOM_MODEL_API_KEY"] == "sk-hub" and len(env["API_SERVER_KEY"]) >= 32
    assert port == bridge.config["spaces"]["first_port"]
    assert oct(os.stat(root / ".env").st_mode & 0o777) == "0o600"
    assert (root / "config.yaml").read_text().startswith("model:")
    copied = root / "profiles" / "coach"
    soul = (copied / "SOUL.md").read_text()   # with the Carnet's rule (on by default), added when its tools were given
    # with the bridge's rules (added at start, every agent talks plainly) and the Carnet's
    assert soul.split("\n\n<!-- bipagents:")[0] == "Tu es Coach." and "<!-- /bipagents:language -->" in soul and "<!-- /bipagents:notebook -->" in soul and (copied / "memories" / "MEMORY.md").exists()
    assert not (copied / "gateway.pid").exists()
    commands = [(e["argv"], e["hermes_home"]) for e in bridge.commands.entries("hermes")]
    assert (["-p", "coach", "config", "set", "gateway.strict", "true"], str(root)) in commands
    assert (["-p", "coach", "config", "set", "browser.backend", "off"], str(root)) in commands
    assert (["profile", "delete", "coach", "--yes"], str(hub)) in commands        # the hub forgets it
    assert bridge.commands.of("fake-gateway") == [[str(root)]]
    # Its tools came with its profile (the same entry and token: nothing to register again).
    assert yaml.safe_load((copied / "config.yaml").read_text())["mcp_servers"]["bipagents"]["timeout"] == 1800
    section = tomllib.loads(bridge.toml_text())["agents"]["coach"]
    assert section["hermes_url"] == f"http://127.0.0.1:{port}/p/coach" and section["space"] == str(root)
    assert section["isolated"] is True and section["sandboxed"] is True
    assert section["hermes_home"] == str(copied) and section["upload_dir_host"] == str(copied / "uploads")
    assert section["upload_dir_container"] == f"{root}/profiles/coach/uploads"
    vault = {a["id"]: a["why"] for a in bridge.get("/v1/vault", headers=AUTH).json()["agents"]}
    assert vault["coach"] == "unprotected_install"                                 # the install itself isn't isolated
    again = bridge.post("/v1/agents/coach/space", headers=AUTH)
    assert again.status_code == 409 and again.json() == {"detail": "this agent is not in the shared hub"}
    # Started again after a move of the whole install: its command once more.
    bridge.stop()
    out = bridge.cli("spaces", "start")
    assert out.returncode == 0 and [l for l in out.stdout.splitlines() if not l.startswith("{")] == ["coach: space started"]
    bridge.start(write=False)
    # Deleted: its gateway stopped (the stop command), its folder gone.
    assert bridge.delete("/v1/agents/coach", headers=AUTH).status_code == 204
    assert marker.read_text() == "coach\n" and not root.exists()
    assert "coach" not in tomllib.loads(bridge.toml_text()).get("agents", {})


def test_a_space_that_does_not_start_is_undone(bridges, fakes):
    bridge = bridges.new()
    hub = managed_install(bridge, fakes, agents=("coach",))
    profile(hub, "coach").joinpath("SOUL.md").write_text("x")
    bridge.config["spaces"] = {"root": str(bridge.root / "spaces"), "first_port": free_port(), "start": "exit 3"}
    bridge.start()
    failed = bridge.post("/v1/agents/coach/space", headers=AUTH)
    assert failed.status_code == 502 and failed.json() == {"detail": "the space could not be created"}
    assert not (bridge.root / "spaces" / "coach").exists() and (hub / "profiles" / "coach").is_dir()
    assert bridge.post("/v1/agents/wellness/space", headers=AUTH).status_code == 404


def test_adopt_serves_what_an_export_brought(bridges, fakes):
    """`bipbridge adopt` (bridge stopped, gateway up): the place's agents get Bot Mode, their sandbox's files and the
    bridge's tools; the profiles no agent serves yet are claimed."""
    bridge = bridges.new()
    hub = managed_install(bridge, fakes, agents=("coach",))
    profile(hub, "coach").joinpath("config.yaml").write_text("model:\n  provider: custom\n")
    profile(hub, "jardin").joinpath("SOUL.md").write_text("Tu es Jardin.")
    bridge.write_config()
    out = bridge.cli("adopt")
    assert out.returncode == 0, out.stderr
    printed = [line for line in out.stdout.splitlines() if not line.startswith("{")]   # its JSON logs go there too
    assert printed == ["coach: ready", "jardin: claimed"]
    assert "hermes-bots" in yaml.safe_load((hub / "profiles" / "coach" / "profile.yaml").read_text())["ui_meta"]
    agents = tomllib.loads(bridge.toml_text())["agents"]
    assert agents["jardin"]["hermes_url"] == f"{fakes.hermes.url}/p/jardin"
    tokens = json.loads((bridge.state_dir / "agent-tools.json").read_text())["tokens"]
    assert set(tokens) == {"coach", "jardin"}
    bridge.start(write=False)
    assert {a["id"] for a in bridge.get("/v1/agents", headers=AUTH).json()["agents"]} == {"wellness", "coach", "jardin"}
