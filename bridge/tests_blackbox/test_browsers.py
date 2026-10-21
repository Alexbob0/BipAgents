"""An agent's own browser turned off and on from the app (`PATCH /v1/agents/{id}/settings {"browser": …}`), and
removed with `bipbridge browser remove` (tests/test_browsers.py's routes; its container is the fake `podman`)."""
import yaml

from bbkit.common import AUTH
from conftest import managed_install, profile, wait_registered

STEALTH = ("mcp_servers:\n  stealth:\n    url: http://127.0.0.1:9601/mcp\n    enabled: true\n"
           "skills:\n  disabled:\n  - something-else\n")


def with_browser(bridges, fakes):
    bridge = bridges.new()
    hub = managed_install(bridge, fakes, agents=("coach",))
    folder = profile(hub, "coach")
    (folder / "config.yaml").write_text(STEALTH)
    (bridge.data / "browsers" / "coach").mkdir(parents=True)
    (bridge.data / "browsers" / "coach" / "ports.json").write_text('{"devtools": 9600, "vnc": 9601}')
    bridge.start()
    wait_registered(bridge, ["coach"])
    return bridge, folder


def test_the_person_turns_the_agents_browser_off_and_on(bridges, fakes):
    bridge, folder = with_browser(bridges, fakes)
    assert bridge.get("/v1/agents/coach/settings", headers=AUTH).json()["browser"] == {"on": True, "ready": True}
    bridge.commands.clear()
    off = bridge.patch("/v1/agents/coach/settings", headers=AUTH, json={"browser": False})
    assert off.status_code == 200 and off.json()["browser"] == {"on": False, "ready": True}
    data = yaml.safe_load((folder / "config.yaml").read_text())
    assert data["mcp_servers"]["stealth"]["enabled"] is False
    assert data["skills"]["disabled"] == ["something-else", "visible-browser"]
    assert bridge.commands.of("hermes") == [["-p", "coach", "tools", "disable", "browser", "--platform", "api_server"]]
    assert bridge.commands.of("systemctl") == [["--user", "stop", "bipagents-browser-coach"]]
    bridge.commands.clear()
    on = bridge.patch("/v1/agents/coach/settings", headers=AUTH, json={"browser": True})
    assert on.json()["browser"] == {"on": True, "ready": True}
    data = yaml.safe_load((folder / "config.yaml").read_text())
    assert data["mcp_servers"]["stealth"]["enabled"] is True and data["skills"]["disabled"] == ["something-else"]
    assert bridge.commands.of("hermes") == [["-p", "coach", "tools", "enable", "browser", "--platform", "api_server"]]


def test_without_a_browser_and_none_to_give(bridges, fakes):
    bridge = bridges.new()
    managed_install(bridge, fakes, agents=("coach",))
    bridge.start()
    assert bridge.get("/v1/agents/coach/settings", headers=AUTH).json()["browser"] is None
    refused = bridge.patch("/v1/agents/coach/settings", headers=AUTH, json={"browser": True})
    assert refused.status_code == 400 and refused.json() == {"detail": "this install has no browser for its agents"}


def test_the_browser_command_removes_one(bridges, fakes):
    bridge, folder = with_browser(bridges, fakes)
    bridge.stop()
    bridge.commands.clear()
    out = bridge.cli("browser", "remove", "--agent", "coach")
    assert out.returncode == 0, out.stderr
    assert [l for l in out.stdout.splitlines() if not l.startswith("{")] == ["coach: browser removed"]
    assert ["rm", "-f", "bipagents-browser-coach"] in bridge.commands.of("podman")
    assert not (bridge.data / "browsers" / "coach").exists()
    assert bridge.cli("browser").returncode == 2
    wrong = bridge.cli("browser", "remove", "--agent", "wellness")
    assert wrong.returncode == 1 and "wellness: not an agent this install manages" in wrong.stderr
