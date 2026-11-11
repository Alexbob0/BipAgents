"""An agent moved from the shared hub into a space of its own by itself (own_space.rs: a project agent other people
talk to leaves the hub once idle), started again by `bipbridge spaces start`, deleted with its space; and `bipbridge
adopt` serving what an export brought to a place. `[spaces] start` / `stop` are the administrator's commands here: the
fake `fake-gateway` serves the space's Hermes on the port the bridge chose (tests/test_spaces.py's ground, over HTTP
and the CLI)."""
import json
import os
import time
import tomllib

import yaml

from bbkit.common import AUTH
from bbkit.servers import free_port, wait_until
from conftest import managed_install, marked, profile, wait_registered


def with_spaces(bridges, fakes):
    """`coach` a project agent (other people talk to it) in a protected hub: it moves out at start."""
    bridge = bridges.new()
    bridge.config["sandbox"] = {"isolate": "on"}
    hub = managed_install(bridge, fakes, agents=("coach",), isolated=True, python=["/usr/bin/false"])
    bridge.state_dir.mkdir(parents=True, exist_ok=True)
    (bridge.state_dir / "project-agents.json").write_text(json.dumps({"projects": {"coach": {
        "created": time.time(), "ends": time.time() + 86400, "archived": None, "from": None, "lang": "fr"}}}))
    (hub / "config.yaml").write_text("model:\n  provider: custom\n  default: glm\n")
    (hub / ".env").write_text("CUSTOM_MODEL_API_KEY=sk-hub\n")
    folder = profile(hub, "coach")
    (folder / "SOUL.md").write_text("Tu es Coach.\n")
    (folder / "memories").mkdir()
    (folder / "memories" / "MEMORY.md").write_text("Alex court.")
    (folder / "gateway.pid").write_text("123")          # a running gateway's file: never copied
    marker = bridge.root / "stopped.txt"
    bridge.config["spaces"] = {"root": str(bridge.root / "spaces"), "first_port": free_port(), "tidy_after": 1,
                               "start": f"{bridge.fakebin}/fake-gateway {{home}}",
                               "stop": f"echo {{agent}} >> {marker}"}
    bridge.start()
    wait_registered(bridge, ["coach"])
    return bridge, hub, marker


def test_an_agent_moves_into_its_own_space(bridges, fakes):
    bridge, hub, marker = with_spaces(bridges, fakes)
    root = bridge.root / "spaces" / "coach"
    assert wait_until(lambda: tomllib.loads(bridge.toml_text())["agents"]["coach"].get("space") == str(root),
                      timeout=120), bridge.log_tail(4000)
    env = dict(line.split("=", 1) for line in (root / ".env").read_text().splitlines())
    port = int(env["API_SERVER_PORT"])
    assert env["API_SERVER_ENABLED"] == "true" and env["API_SERVER_HOST"] == "127.0.0.1"
    assert env["CUSTOM_MODEL_API_KEY"] == "sk-hub" and len(env["API_SERVER_KEY"]) >= 32
    assert port == bridge.config["spaces"]["first_port"]
    assert oct(os.stat(root / ".env").st_mode & 0o777) == "0o600"
    assert (root / "config.yaml").read_text().startswith("model:")
    copied = root / "profiles" / "coach"
    assert (copied / "SOUL.md").read_text().startswith("Tu es Coach.") and (copied / "memories" / "MEMORY.md").exists()
    assert not (copied / "gateway.pid").exists()
    commands = [(e["argv"], e["hermes_home"]) for e in bridge.commands.entries("hermes")]
    settings = yaml.safe_load((copied / "config.yaml").read_text())   # written by the bridge, no `hermes config set`
    assert settings["gateway"]["strict"] is True and settings["browser"]["backend"] == "off"
    assert not any(argv[2:4] == ["config", "set"] and argv[4] in ("gateway.strict", "browser.backend") for argv, _ in commands)
    assert (["profile", "delete", "coach", "--yes"], str(hub)) in commands        # the hub forgets it
    assert bridge.commands.of("fake-gateway") == [[str(root)]]
    # Its tools came with its profile (the same entry and token: nothing to register again).
    assert yaml.safe_load((copied / "config.yaml").read_text())["mcp_servers"]["bipagents"]["timeout"] == 1800
    section = tomllib.loads(bridge.toml_text())["agents"]["coach"]
    assert section["hermes_url"] == f"http://127.0.0.1:{port}/p/coach" and section["space"] == str(root)
    assert section["isolated"] is True and section["sandboxed"] is True
    assert section["hermes_home"] == str(copied) and section["upload_dir_host"] == str(copied / "uploads")
    assert section["upload_dir_container"] == f"{root}/profiles/coach/uploads"
    # Started again after a move of the whole install: its command once more.
    bridge.stop()
    out = bridge.cli("spaces", "start")
    assert out.returncode == 0 and [l for l in out.stdout.splitlines() if not l.startswith("{")] == ["coach: space started"]
    bridge.start(write=False)
    # Deleted: its gateway stopped (the stop command), its folder gone.
    assert bridge.delete("/v1/agents/coach", headers=AUTH).status_code == 204
    assert marker.read_text() == "coach\n" and not root.exists()
    assert "coach" not in tomllib.loads(bridge.toml_text()).get("agents", {})


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
    # no Bot Mode mark: Hermes' own message_agent stays out between agents of this household (peers.rs)
    assert not marked(hub / "profiles" / "coach")
    agents = tomllib.loads(bridge.toml_text())["agents"]
    assert agents["jardin"]["hermes_url"] == f"{fakes.hermes.url}/p/jardin"
    tokens = json.loads((bridge.state_dir / "agent-tools.json").read_text())["tokens"]
    assert set(tokens) == {"coach", "jardin"}
    bridge.start(write=False)
    assert {a["id"] for a in bridge.get("/v1/agents", headers=AUTH).json()["agents"]} == {"wellness", "coach", "jardin"}
