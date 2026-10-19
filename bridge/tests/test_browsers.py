import asyncio
import json
import sys

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


def test_an_install_hands_out_only_its_own_range_and_moves_a_browser_from_outside_it(setup):
    """Two installs on one machine each have a range (installer/bipinstall/ports.py): a browser set up before it
    existed, on ports outside it, gets ports of its own range when it is set up again; its profile stays."""
    config, hub, run, commands, tmp_path = setup
    config.browser.ports = 4
    place = tmp_path / "browsers" / "budget"
    (place / "profile").mkdir(parents=True)
    (place / "profile" / "Cookies").write_text("kept")
    (place / "ports.json").write_text(json.dumps({"cdp": 9600, "vnc": 9601}))
    assert not browsers.in_range(config, {"cdp": 9600, "vnc": 9601})
    agent = asyncio.run(browsers.setup(config, config.agent("budget"), run, str(hub), is_free=lambda p: True))
    assert json.loads((place / "ports.json").read_text()) == {"cdp": 19600, "vnc": 19601}
    assert agent.desktop_vnc == "tcp://127.0.0.1:19601" and (place / "profile" / "Cookies").read_text() == "kept"
    assert ["/opt/hermes", "-p", "budget", "config", "set", "browser.cdp_url", "http://127.0.0.1:19600"] in \
        [c[0] for c in commands]
    (tmp_path / "browsers" / "other").mkdir()
    (tmp_path / "browsers" / "other" / "ports.json").write_text(json.dumps({"cdp": 19602, "vnc": 19603}))
    with pytest.raises(browsers.BrowserError):   # 19600-19603 all handed out: the range is full
        browsers.pick_ports(config, is_free=lambda p: True)


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
    # Not started with the session: the agent's Hermes pointed at it (DevTools for the vault), its MCP server behind
    # browser_mcp.py, which starts the container at the first call and stops it after 10 minutes unused.
    ran = [c[0] for c in commands]
    assert ["systemctl", "--user", "disable", "bipagents-browser-budget"] in ran
    assert not any(c[:3] == ["systemctl", "--user", "enable"] or "start" in c for c in ran)
    assert ["/opt/hermes", "-p", "budget", "config", "set", "browser.cdp_url", "http://127.0.0.1:19602"] in ran
    mcp = next(c for c in ran if c[4:6] == ["set", "mcp_servers.stealth"])
    place = tmp_path / "browsers" / "budget"
    assert json.loads(mcp[6]) == {"command": sys.executable, "connect_timeout": 90, "timeout": 120,
                                  "args": [str(browsers.PROXY), str(place / "mcp.json")],
                                  "tools": {"exclude": ["get_instance_state"]}}
    proxy = json.loads((place / "mcp.json").read_text())
    assert proxy["server"] == ["/usr/bin/podman", "exec", "-i", "bipagents-browser-budget", "/opt/stealth/mcp.sh"]
    assert proxy["start"][1:] == ["--user", "start", "bipagents-browser-budget"]
    assert proxy["stop"][1:] == ["--user", "stop", "bipagents-browser-budget"]
    assert proxy["idle"] == 600 and proxy["cdp_port"] == 19602 and proxy["busy"] == str(place / "busy")
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
    assert ["systemctl", "--user", "restart", "bipagents-browser-budget"] not in [c[0] for c in commands]
    # A unit written by an earlier bridge (a 1600×900 window): rewritten, and its container restarted with these flags.
    unit_file = tmp_path / "units" / "bipagents-browser-budget.service"
    unit_file.write_text(unit_file.read_text().replace("--start-maximized", "--window-size=1600,900"))
    commands.clear()
    asyncio.run(browsers.setup(config, config.agent("budget"), run, str(hub), is_free=lambda p: True))
    assert "--start-maximized" in unit_file.read_text() and "--window-size" not in unit_file.read_text()
    assert ["systemctl", "--user", "try-restart", "bipagents-browser-budget"] in [c[0] for c in commands]


def test_a_browser_kept_running(setup):
    config, hub, run, commands, tmp_path = setup
    config.browser.idle_minutes = 0
    asyncio.run(browsers.setup(config, config.agent("budget"), run, str(hub), is_free=lambda p: True))
    ran = [c[0] for c in commands]
    assert ["systemctl", "--user", "enable", "--now", "bipagents-browser-budget"] in ran
    mcp = next(c for c in ran if c[4:6] == ["set", "mcp_servers.stealth"])
    assert json.loads(mcp[6])["command"] == "/usr/bin/podman"
    assert json.loads(mcp[6])["args"] == ["exec", "-i", "bipagents-browser-budget", "/opt/stealth/mcp.sh"]


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


def test_taking_over_the_browser_makes_the_agents_tools_wait(setup, monkeypatch):
    config, hub, run, commands, tmp_path = setup
    asyncio.run(browsers.setup(config, config.agent("budget"), run, str(hub), is_free=lambda p: True))
    ran = []

    class Done:
        async def wait(self):
            return 0

    async def fake_exec(*args, **kwargs):
        ran.append(list(args))
        return Done()

    monkeypatch.setattr(asyncio, "create_subprocess_exec", fake_exec)
    assert asyncio.run(browsers.hold(config, "budget", True)) is True
    # Its screen checked first (a browser stopped while unused starts again), then the hold.
    assert ran[0] == ["/usr/bin/podman", "exec", "bipagents-browser-budget", "test", "-e", "/tmp/.X11-unix/X99"]
    assert ran[-1] == ["/usr/bin/podman", "exec", "bipagents-browser-budget", "sh", "-c", "touch /tmp/bipagents-hold"]
    asyncio.run(browsers.hold(config, "budget", False))
    assert ran[-1][-1] == "rm -f /tmp/bipagents-hold"
    assert asyncio.run(browsers.hold(config, "nobody", True)) is False and len(ran) == 3   # no browser of its own


def test_the_desktop_wakes_a_stopped_browser(setup, monkeypatch):
    config, hub, run, commands, tmp_path = setup
    asyncio.run(browsers.setup(config, config.agent("budget"), run, str(hub), is_free=lambda p: True))
    ran, screen = [], {"up": False}

    class Done:
        def __init__(self, code):
            self.code = code

        async def wait(self):
            return self.code

    async def fake_exec(*args, **kwargs):
        ran.append(list(args))
        if args[:3] == ("systemctl", "--user", "start"):
            screen["up"] = True
        return Done(0 if screen["up"] or args[0] == "systemctl" else 1)

    monkeypatch.setattr(asyncio, "create_subprocess_exec", fake_exec)
    assert asyncio.run(browsers.wake(config, "budget")) is True
    assert ["systemctl", "--user", "start", "bipagents-browser-budget"] in ran
    assert (tmp_path / "browsers" / "budget" / "busy").exists()   # not stopped again right away
    ran.clear()
    assert asyncio.run(browsers.wake(config, "budget")) is True and len(ran) == 1   # already up: nothing started
    assert asyncio.run(browsers.wake(config, "nobody")) is False


def test_the_apps_switch_turns_the_browser_off_and_on_keeping_its_profile(setup):
    import yaml

    config, hub, run, commands, tmp_path = setup
    agent = config.agent("budget")
    assert browsers.state(str(hub), "budget") is None and browsers.available(config)
    # The first time: set up (its container, its MCP server in the profile).
    asyncio.run(browsers.switch(config, agent, True, run, str(hub)))
    ran = [c[0] for c in commands]
    assert any(c[4:6] == ["set", "mcp_servers.stealth"] for c in ran)
    profile = hub / "profiles" / "budget" / "config.yaml"   # what `hermes config set` wrote
    profile.write_text(yaml.safe_dump({"model": {"provider": "custom"}, "skills": {"disabled": ["arxiv"]},
                                       "mcp_servers": {"stealth": {"command": "python3", "args": ["proxy"]}}}))
    assert browsers.state(str(hub), "budget") is True
    # Off: its tools and skill off, its Chrome stopped; its profile and its MCP entry stay.
    commands.clear()
    asyncio.run(browsers.switch(config, agent, False, run, str(hub)))
    data = yaml.safe_load(profile.read_text())
    assert data["mcp_servers"]["stealth"] == {"command": "python3", "args": ["proxy"], "enabled": False}
    assert data["skills"]["disabled"] == ["arxiv", "visible-browser"] and data["model"] == {"provider": "custom"}
    ran = [c[0] for c in commands]
    assert ["/opt/hermes", "-p", "budget", "tools", "disable", "browser", "--platform", "api_server"] in ran
    assert ["systemctl", "--user", "stop", "bipagents-browser-budget"] in ran
    assert (tmp_path / "browsers" / "budget" / "profile").is_dir() and browsers.state(str(hub), "budget") is False
    # On again: instant, nothing set up anew.
    commands.clear()
    asyncio.run(browsers.switch(config, agent, True, run, str(hub)))
    data = yaml.safe_load(profile.read_text())
    assert data["mcp_servers"]["stealth"]["enabled"] is True and data["skills"]["disabled"] == ["arxiv"]
    assert [c[0] for c in commands] == [["/opt/hermes", "-p", "budget", "tools", "enable", "browser", "--platform", "api_server"]]


def test_no_browser_to_give_without_the_installers_image(setup):
    config, hub, run, commands, tmp_path = setup
    config.browser.enabled = False
    assert not browsers.available(config)
    with pytest.raises(browsers.BrowserError):
        asyncio.run(browsers.switch(config, config.agent("budget"), True, run, str(hub)))
    assert commands == []
