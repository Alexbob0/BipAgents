import asyncio
import json

import pytest

from bipbridge import browsers
from bipbridge.config import parse_config


@pytest.fixture
def setup(config_dict, tmp_path, monkeypatch):
    hub = tmp_path / ".hermes"
    (hub / "profiles" / "budget").mkdir(parents=True)
    config_path = tmp_path / "bridge.toml"
    config_path.write_text("# bridge\n")
    config_dict["hermes"] = {"bin": "/opt/hermes", "home": str(hub), "hub_url": "http://127.0.0.1:8642",
                             "isolated": True}
    config_dict["browser"] = {"enabled": True, "runtime": "podman", "root": str(tmp_path / "browsers"),
                              "first_port": 19600, "language": "fr-FR", "timezone": "Europe/Paris", "gpu": "none"}
    config_dict["agents"]["Budget"] = {"hermes_url": "http://127.0.0.1:8642/p/budget", "hermes_key": "b" * 32}
    config = parse_config(config_dict, source_path=str(config_path))
    monkeypatch.setattr(browsers.platform, "system", lambda: "Linux")
    monkeypatch.setattr(browsers, "runtime", lambda config: "/usr/bin/podman")
    monkeypatch.setattr(browsers, "unit_path", lambda agent: tmp_path / "units" / f"bipagents-browser-{agent}.service")
    commands = []

    async def run(args, env):
        commands.append((list(args), dict(env)))
        return ""

    return config, hub, run, commands, tmp_path


def test_an_agent_gets_its_own_browser_container_and_desktop(setup):
    from bipbridge.agents_admin import save_agent

    config, hub, run, commands, tmp_path = setup
    agent = asyncio.run(browsers.setup(config, config.agent("budget"), run, str(hub), is_free=lambda p: p != 19600))
    # Its two local ports (19600 is in use), its VNC password and its Chrome profile, kept apart.
    ports = json.loads((tmp_path / "browsers" / "budget" / "ports.json").read_text())
    assert ports == {"cdp": 19602, "vnc": 19603}
    password = tmp_path / "browsers" / "budget" / "vnc.pass"
    assert len(password.read_text().strip()) == 8 and oct(password.stat().st_mode & 0o777) == "0o600"
    assert (tmp_path / "browsers" / "budget" / "profile").is_dir()
    # The container: nothing of the host but its profile and password, ports on 127.0.0.1 only.
    unit = (tmp_path / "units" / "bipagents-browser-budget.service").read_text()
    start = next(line for line in unit.splitlines() if line.startswith("ExecStart="))
    assert "--name bipagents-browser-budget" in start and "--cap-drop ALL" in start
    assert f"-v {tmp_path}/browsers/budget/profile:/home/bip/profile:Z" in start
    assert "-p 127.0.0.1:19602:9223 -p 127.0.0.1:19603:5900" in start
    assert "--userns=keep-id:uid=1000,gid=1000" in start and "--device" not in start
    assert "--accept-lang=fr-FR,fr" in start and "TZ=Europe/Paris" in start
    assert start.endswith(" bipagents-browser")
    # Started, then the agent's Hermes pointed at it: DevTools for the vault, the MCP server inside the container.
    ran = [c[0] for c in commands]
    assert ["systemctl", "--user", "enable", "--now", "bipagents-browser-budget"] in ran
    assert ["/opt/hermes", "-p", "budget", "config", "set", "browser.cdp_url", "http://127.0.0.1:19602"] in ran
    mcp = next(c for c in ran if c[4:6] == ["set", "mcp_servers.stealth"])
    assert json.loads(mcp[6]) == {"command": "/usr/bin/podman", "connect_timeout": 90, "timeout": 120,
                                  "args": ["exec", "-i", "bipagents-browser-budget", "/opt/stealth/mcp.sh"],
                                  "tools": {"exclude": ["get_instance_state"]}}
    assert all(env == {"HERMES_HOME": str(hub)} for args, env in commands if args[0] == "/opt/hermes")
    assert (hub / "profiles" / "budget" / "skills" / "visible-browser" / "SKILL.md").read_text().startswith("---")
    # The app's « Bureau »: the bridge logs in to its VNC server.
    assert agent.desktop_vnc == "tcp://127.0.0.1:19603" and agent.desktop_vnc_password_file == str(password)
    save_agent(config, agent)
    text = (tmp_path / "bridge.toml").read_text()
    assert 'desktop_vnc = "tcp://127.0.0.1:19603"' in text and f'desktop_vnc_password_file = "{password}"' in text
    # A second agent takes the next ports; setting one up again keeps its own.
    assert browsers.pick_ports(config, is_free=lambda p: True) == {"cdp": 19600, "vnc": 19601}
    commands.clear()
    asyncio.run(browsers.setup(config, config.agent("budget"), run, str(hub), is_free=lambda p: True))
    assert json.loads((tmp_path / "browsers" / "budget" / "ports.json").read_text()) == ports


def test_deleting_the_agent_removes_its_browser(setup):
    from bipbridge.agents_admin import delete_agent

    config, hub, run, commands, tmp_path = setup
    asyncio.run(browsers.setup(config, config.agent("budget"), run, str(hub), is_free=lambda p: True))
    commands.clear()
    asyncio.run(delete_agent(config, config.agent("budget"), run))
    ran = [c[0] for c in commands]
    assert ["systemctl", "--user", "disable", "--now", "bipagents-browser-budget"] in ran
    assert ["/usr/bin/podman", "rm", "-f", "bipagents-browser-budget"] in ran
    assert not (tmp_path / "browsers" / "budget").exists()
    assert not (tmp_path / "units" / "bipagents-browser-budget.service").exists()


def test_with_a_gpu_and_docker(setup, monkeypatch):
    config, hub, run, commands, tmp_path = setup
    args = browsers.run_args(config, "budget", {"cdp": 1, "vnc": 2}, "/usr/bin/docker", gpu=True)
    assert "--device" in args and "keep-groups" not in args and "--userns=keep-id:uid=1000,gid=1000" not in args
    assert any("--use-angle=vulkan" in a for a in args)


def test_only_on_linux(setup, monkeypatch):
    config, hub, run, commands, tmp_path = setup
    monkeypatch.setattr(browsers.platform, "system", lambda: "Darwin")
    with pytest.raises(browsers.BrowserError):
        asyncio.run(browsers.setup(config, config.agent("budget"), run, str(hub)))
    assert commands == []
