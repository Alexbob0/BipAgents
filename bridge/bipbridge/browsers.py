"""Each agent's visible browser: a real Chrome on a screen of its own, in a container of its own
(`browser/Containerfile`, image built by the installer).

For an agent, this sets up:

- its container, `bipagents-browser-<agent>`, run by a systemd user unit: its Chrome profile (cookies and logins) in
  `<browser root>/<agent>/profile`, nothing else of the host; a page it opens (file:// included) only ever sees this
  container. Started when the agent (or the person, in « Bureau ») uses it, stopped after `idle_minutes` unused
  (browser_mcp.py): a machine with several agents only holds the browsers in use; `idle_minutes = 0` keeps it running;
- two ports on 127.0.0.1, never published: Chrome's DevTools (Hermes' `browser.cdp_url`, so the vault fills in this
  browser) and the screen's VNC server, with a password of its own, which the bridge logs in to and relays to the app
  (« Bureau », behind Face ID; see desktop.py);
- in the agent's Hermes profile, the `stealth` MCP server (`podman exec -i <container> /opt/stealth/mcp.sh`: the
  server runs in the container, next to Chrome; behind browser_mcp.py, which starts it when used) and the skill that
  tells the agent how to use it.

Linux only: the screen is an X server in the container.
"""
from __future__ import annotations

import json
import locale
import logging
import os
import platform
import secrets
import shutil
import socket
import string
import sys
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

from .config import AgentConfig, Config
from .logs import fields

log = logging.getLogger("bipbridge.browsers")

SKILL = Path(__file__).with_name("skills") / "visible-browser" / "SKILL.md"
GPU_FLAGS = ["--use-gl=angle", "--use-angle=vulkan", "--enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan"]


class BrowserError(Exception):
    """Safe to show."""


def container(agent: str) -> str:
    return f"bipagents-browser-{agent}"


def runtime(config: Config) -> Optional[str]:
    if config.browser.runtime:
        return shutil.which(config.browser.runtime)
    return shutil.which("podman") or shutil.which("docker")


def folder(config: Config, agent: str) -> Path:
    return Path(config.browser.root) / agent


def unit_path(agent: str) -> Path:
    return Path.home() / ".config" / "systemd" / "user" / f"{container(agent)}.service"


HOLD_FLAG = "/tmp/bipagents-hold"   # browser/hold_gate.py: while it is there, the agent's browser tools wait
SCREEN = "/tmp/.X11-unix/X99"       # browser/entrypoint.sh: its screen is up
PROXY = Path(__file__).with_name("browser_mcp.py")


def on_demand(config: Config) -> bool:
    return config.browser.idle_minutes > 0


def busy(config: Config, agent: str) -> None:
    """Someone uses the agent's browser (the person watching its desktop): not stopped for `idle_minutes` from now."""
    try:
        (folder(config, agent) / "busy").touch()
    except OSError:
        pass


async def wake(config: Config, agent: str, wait: float = 60) -> bool:
    """The agent's browser container, started if it was stopped, its screen up. Returns whether this agent has one."""
    import asyncio
    import time
    engine = runtime(config)
    if not engine or not ports_of(config, agent):
        return False
    if not on_demand(config):
        return True
    busy(config, agent)

    async def run(*args: str) -> int:
        process = await asyncio.create_subprocess_exec(*args, stdout=asyncio.subprocess.DEVNULL,
                                                       stderr=asyncio.subprocess.DEVNULL)
        return await process.wait()

    if await run(engine, "exec", container(agent), "test", "-e", SCREEN) == 0:
        return True
    await run("systemctl", "--user", "start", unit_path(agent).stem)
    deadline = time.monotonic() + wait
    while await run(engine, "exec", container(agent), "test", "-e", SCREEN) != 0:
        if time.monotonic() > deadline:
            log.warning("browser did not wake", extra=fields(agent=agent))
            break
        await asyncio.sleep(0.5)
    else:
        log.info("browser woken", extra=fields(agent=agent))
    return True


async def hold(config: Config, agent: str, on: bool) -> bool:
    """The person takes over the agent's browser (or hands it back): its MCP tools and its DevTools wait meanwhile.
    Returns whether this agent has such a browser (its container, set up here)."""
    import asyncio
    engine = runtime(config)
    if not engine or not ports_of(config, agent):
        return False
    if on:
        await wake(config, agent)
    command = f"touch {HOLD_FLAG}" if on else f"rm -f {HOLD_FLAG}"
    process = await asyncio.create_subprocess_exec(engine, "exec", container(agent), "sh", "-c", command,
                                                   stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.DEVNULL)
    if await process.wait() != 0:
        log.warning("browser hold not set", extra=fields(agent=agent, on=on))
    return True


def ports_of(config: Config, agent: str) -> Optional[Dict[str, int]]:
    try:
        return json.loads((folder(config, agent) / "ports.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def _free(port: int) -> bool:
    with socket.socket() as probe:
        return probe.connect_ex(("127.0.0.1", port)) != 0


def in_range(config: Config, ports: Dict[str, int]) -> bool:
    """The ports are this install's own (first_port and the `ports` after it): another install on the machine never
    hands them out, even while its browsers are stopped."""
    first = config.browser.first_port
    return all(first <= port < first + config.browser.ports for port in ports.values())


def pick_ports(config: Config, is_free: Callable[[int], bool] = _free) -> Dict[str, int]:
    """Two ports in a row, in this install's range, that no other agent's browser has and nothing listens on."""
    taken = set()
    root = Path(config.browser.root)
    for other in root.iterdir() if root.is_dir() else []:
        taken.update((ports_of(config, other.name) or {}).values())
    port = config.browser.first_port
    while port + 1 < config.browser.first_port + config.browser.ports:
        if port not in taken and port + 1 not in taken and is_free(port) and is_free(port + 1):
            return {"cdp": port, "vnc": port + 1}
        port += 2
    raise BrowserError("no free port for a new browser")


def _language(config: Config) -> str:
    if config.browser.language:
        return config.browser.language
    found = (os.environ.get("LANG") or locale.getlocale()[0] or "en_US").split(".")[0]
    return found.replace("_", "-") if found not in ("C", "POSIX") else "en-US"


def _timezone(config: Config) -> str:
    if config.browser.timezone:
        return config.browser.timezone
    try:
        link = os.readlink("/etc/localtime")
        return link.split("zoneinfo/", 1)[1]
    except (OSError, IndexError):
        return os.environ.get("TZ", "UTC")


def run_args(config: Config, agent: str, ports: Dict[str, int], engine: str, gpu: bool) -> List[str]:
    """`podman run` (or docker) for the agent's container."""
    place = folder(config, agent)
    lang = _language(config)
    flags = [f"--accept-lang={lang},{lang.split('-')[0]}"] + (GPU_FLAGS if gpu else [])
    # --test-type and --hide-crash-restore-bubble: no warning bar about --no-sandbox (the container is the sandbox)
    # and no « Restore pages? » after the container was stopped, over what the person watches.
    # Maximized on the 1366×768 screen, its panel kept: as on a laptop (never full screen, which sites read as a robot).
    base = ["--start-maximized", "--no-first-run", "--no-default-browser-check", "--disable-dev-shm-usage",
            "--ignore-gpu-blocklist", "--test-type", "--hide-crash-restore-bubble"]
    args = [engine, "run", "--rm", "--name", container(agent), "--hostname", "desktop",
            "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--memory", "3g", "--shm-size", "1g",
            "-v", f"{place / 'profile'}:/home/bip/profile:Z",
            "-v", f"{place / 'vnc.pass'}:/run/bipagents/vnc.pass:ro,Z",
            "-p", f"127.0.0.1:{ports['cdp']}:9223", "-p", f"127.0.0.1:{ports['vnc']}:5900",
            "-e", f"TZ={_timezone(config)}", "-e", f"STEALTH_TIMEZONE={_timezone(config)}",
            "-e", f"LANG={lang.replace('-', '_')}.UTF-8", "-e", f"LANGUAGE={lang.replace('-', '_')}:{lang.split('-')[0]}",
            "-e", "STEALTH_BROWSER_ARGS=" + ";".join(base + flags)]
    if Path(engine).name == "podman":
        # The agent's files stay the person's on the host (uid 1000 in the container is them).
        args[2:2] = ["--userns=keep-id:uid=1000,gid=1000"]
    else:
        args[2:2] = ["--user", f"{os.getuid()}:{os.getgid()}", "-e", "HOME=/tmp"]
    if gpu:
        args += ["--device", "/dev/dri"] + (["--group-add", "keep-groups"] if Path(engine).name == "podman" else [])
    return args + [config.browser.image]


def systemd_unit(agent: str, engine: str, args: List[str]) -> str:
    import shlex
    name = container(agent)
    return "\n".join([
        "[Unit]", f"Description=Visible browser of the agent {agent} (BipAgents)", "",
        "[Service]",
        f"ExecStartPre=-{shlex.join([engine, 'rm', '-f', name])}",
        f"ExecStart={shlex.join(args)}",
        f"ExecStop={shlex.join([engine, 'stop', '-t', '10', name])}",
        "Restart=always", "RestartSec=10", "",
        "[Install]", "WantedBy=default.target", ""])


def vnc_password() -> str:
    # VNC Authentication only uses the first 8 characters; the port only listens on 127.0.0.1.
    return "".join(secrets.choice(string.ascii_letters + string.digits) for _ in range(8))


def gpu_available(config: Config) -> bool:
    return config.browser.gpu == "auto" and os.access("/dev/dri", os.R_OK) and any(Path("/dev/dri").glob("renderD*"))


def mcp_server(engine: str, agent: str, proxy: Optional[Path] = None) -> Dict[str, Any]:
    """Hermes' `stealth` MCP server: the one in the container, or (`proxy`: its browser_mcp.py config) the one that
    starts it when used."""
    # A call that hangs gives up after two minutes, not Hermes' five; get_instance_state hangs on some pages (the agent
    # has get_active_tab and list_tabs).
    command = ([sys.executable, str(PROXY), str(proxy)] if proxy
               else [engine, "exec", "-i", container(agent), "/opt/stealth/mcp.sh"])
    return {"command": command[0], "args": command[1:], "connect_timeout": 90, "timeout": 120,
            "tools": {"exclude": ["get_instance_state"]}}


def proxy_config(config: Config, agent: str, engine: str, ports: Dict[str, int]) -> Dict[str, Any]:
    """browser_mcp.py's settings for the agent."""
    place, unit = folder(config, agent), unit_path(agent).stem
    systemctl = shutil.which("systemctl") or "systemctl"   # Hermes may start it with a PATH of its own
    return {"start": [systemctl, "--user", "start", unit],
            "ready": [engine, "exec", container(agent), "test", "-e", SCREEN],
            "active": [systemctl, "--user", "is-active", "--quiet", unit],
            "server": [engine, "exec", "-i", container(agent), "/opt/stealth/mcp.sh"],
            "stop": [systemctl, "--user", "stop", unit],
            "cache": str(place / "mcp-cache.json"), "busy": str(place / "busy"), "cdp_port": ports["cdp"],
            "idle": config.browser.idle_minutes * 60}


async def setup(config: Config, agent: AgentConfig, run: Callable[..., Any], home: str,
                is_free: Callable[[int], bool] = _free) -> AgentConfig:
    """The agent's browser, started; its Hermes profile pointed at it. `home` is the HERMES_HOME of the gateway that
    serves the agent. Returns the agent with its desktop (the caller saves it)."""
    if platform.system() != "Linux":
        raise BrowserError("the visible browser needs Linux")
    engine = runtime(config)
    if not engine:
        raise BrowserError("the visible browser needs podman (or docker)")
    place = folder(config, agent.name)
    ports = ports_of(config, agent.name)
    if not ports or not in_range(config, ports):   # none yet, or from before this install had a range of its own
        ports = pick_ports(config, is_free)
    (place / "profile").mkdir(parents=True, exist_ok=True)
    os.chmod(place, 0o700)
    password = place / "vnc.pass"
    if not password.exists():
        password.write_text(vnc_password() + "\n", encoding="utf-8")
    os.chmod(password, 0o600)
    (place / "ports.json").write_text(json.dumps(ports) + "\n", encoding="utf-8")

    unit = unit_path(agent.name)
    unit.parent.mkdir(parents=True, exist_ok=True)
    text = systemd_unit(agent.name, engine, run_args(config, agent.name, ports, engine, gpu_available(config)))
    changed = unit.exists() and unit.read_text() != text
    unit.write_text(text)
    await run(["systemctl", "--user", "daemon-reload"], {})
    proxy = None
    if on_demand(config):
        # Started by its first use (browser_mcp.py), not with the session; one running now is stopped once unused.
        proxy = place / "mcp.json"
        proxy.write_text(json.dumps(proxy_config(config, agent.name, engine, ports), indent=2) + "\n", encoding="utf-8")
        await run(["systemctl", "--user", "disable", unit.stem], {})
        if changed:   # set up before with other flags: started again with these if it runs (its profile stays)
            await run(["systemctl", "--user", "try-restart", unit.stem], {})
    else:
        await run(["systemctl", "--user", "enable", "--now", unit.stem], {})
        if changed:   # set up before with other flags: its container starts again with these (its profile stays)
            await run(["systemctl", "--user", "restart", unit.stem], {})

    env = {"HERMES_HOME": home}
    hermes = config.hermes.bin
    await run([hermes, "-p", agent.name, "config", "set", "browser.cdp_url", f"http://127.0.0.1:{ports['cdp']}"], env)
    await run([hermes, "-p", agent.name, "config", "set", "mcp_servers.stealth",
               json.dumps(mcp_server(engine, agent.name, proxy))], env)
    # The vault's tools ride with Hermes' browser toolset; they act on this Chrome through its DevTools port.
    await run([hermes, "-p", agent.name, "tools", "enable", "browser", "--platform", "api_server"], env)
    skill = Path(home) / "profiles" / agent.name / "skills" / "visible-browser" / "SKILL.md"
    skill.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(SKILL, skill)

    agent.desktop_vnc = f"tcp://127.0.0.1:{ports['vnc']}"
    agent.desktop_vnc_password_file = str(password)
    log.info("browser set up", extra=fields(agent=agent.name, cdp=ports["cdp"], vnc=ports["vnc"]))
    return agent


def available(config: Config) -> bool:
    """This install can give an agent its own browser from the app: Linux, a container runtime, and the image the
    installer built (`browser.enabled`)."""
    return config.browser.enabled and platform.system() == "Linux" and runtime(config) is not None


def _profile_config(home: str, agent: str) -> Path:
    return Path(home) / "profiles" / agent / "config.yaml"


def state(home: str, agent: str) -> Optional[bool]:
    """Whether the agent's browser is on (its MCP server enabled in its Hermes profile); None: it has none."""
    import yaml
    try:
        data = yaml.safe_load(_profile_config(home, agent).read_text(encoding="utf-8")) or {}
    except (OSError, yaml.YAMLError):
        return None
    server = (data.get("mcp_servers") or {}).get("stealth") if isinstance(data, dict) else None
    if not isinstance(server, dict):
        return None
    return str(server.get("enabled", True)).strip().lower() not in ("false", "0", "no", "off")


def _mark(home: str, agent: str, on: bool) -> None:
    """Its MCP server and its skill on or off in its profile, the server's entry kept (Hermes stops or starts it within
    a minute; a request after this one already has its tools or not)."""
    import yaml
    path = _profile_config(home, agent)
    data = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
    data["mcp_servers"]["stealth"]["enabled"] = on
    skills = data.get("skills") if isinstance(data.get("skills"), dict) else {}
    off = [name for name in (skills.get("disabled") or []) if name != "visible-browser"]
    skills["disabled"] = off if on else off + ["visible-browser"]
    data["skills"] = skills
    path.write_text(yaml.safe_dump(data, allow_unicode=True, sort_keys=False), encoding="utf-8")


async def switch(config: Config, agent: AgentConfig, on: bool, run: Callable[..., Any], home: str) -> AgentConfig:
    """The app's « Naviguer à ta place »: on sets the agent's browser up the first time (`setup`), then only turns it
    back on; off turns its tools off and stops its Chrome, keeping its profile (the sites it is logged in to)."""
    env = {"HERMES_HOME": home}
    hermes = config.hermes.bin
    if on:
        if state(home, agent.name) is None or not folder(config, agent.name).exists():
            if not available(config):
                raise BrowserError("this install has no browser for its agents")
            return await setup(config, agent, run, home)
        _mark(home, agent.name, True)
        await run([hermes, "-p", agent.name, "tools", "enable", "browser", "--platform", "api_server"], env)
        log.info("browser on", extra=fields(agent=agent.name))
        return agent
    if state(home, agent.name) is not None:
        _mark(home, agent.name, False)
    await run([hermes, "-p", agent.name, "tools", "disable", "browser", "--platform", "api_server"], env)
    try:
        await run(["systemctl", "--user", "stop", unit_path(agent.name).stem], {})
    except RuntimeError:
        pass
    log.info("browser off", extra=fields(agent=agent.name))
    return agent


async def remove(config: Config, agent: str, run: Callable[..., Any]) -> None:
    """The agent's browser stopped, its container and profile gone (its Hermes profile is the caller's)."""
    unit = unit_path(agent)
    if unit.exists():
        try:
            await run(["systemctl", "--user", "disable", "--now", unit.stem], {})
        except RuntimeError:
            pass
        unit.unlink(missing_ok=True)
        try:
            await run(["systemctl", "--user", "daemon-reload"], {})
            await run(["systemctl", "--user", "reset-failed", unit.stem], {})
        except RuntimeError:
            pass
    engine = runtime(config)
    if engine:
        try:
            await run([engine, "rm", "-f", container(agent)], {})
        except RuntimeError:
            pass
    shutil.rmtree(folder(config, agent), ignore_errors=True)
    log.info("browser removed", extra=fields(agent=agent))
