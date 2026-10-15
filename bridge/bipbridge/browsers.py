"""Each agent's visible browser: a real Chrome on a screen of its own, in a container of its own
(`browser/Containerfile`, image built by the installer).

For an agent, this sets up:

- its container, `bipagents-browser-<agent>`, kept running by a systemd user unit: its Chrome profile (cookies and
  logins) in `<browser root>/<agent>/profile`, nothing else of the host; a page it opens (file:// included) only ever
  sees this container;
- two ports on 127.0.0.1, never published: Chrome's DevTools (Hermes' `browser.cdp_url`, so the vault fills in this
  browser) and the screen's VNC server, with a password of its own, which the bridge logs in to and relays to the app
  (« Bureau », behind Face ID; see desktop.py);
- in the agent's Hermes profile, the `stealth` MCP server (`podman exec -i <container> /opt/stealth/mcp.sh`: the
  server runs in the container, next to Chrome) and the skill that tells the agent how to use it.

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


def ports_of(config: Config, agent: str) -> Optional[Dict[str, int]]:
    try:
        return json.loads((folder(config, agent) / "ports.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def _free(port: int) -> bool:
    with socket.socket() as probe:
        return probe.connect_ex(("127.0.0.1", port)) != 0


def pick_ports(config: Config, is_free: Callable[[int], bool] = _free) -> Dict[str, int]:
    """Two ports in a row, from `first_port`, that no other agent's browser has and nothing listens on."""
    taken = set()
    root = Path(config.browser.root)
    for other in root.iterdir() if root.is_dir() else []:
        taken.update((ports_of(config, other.name) or {}).values())
    port = config.browser.first_port
    while port < config.browser.first_port + 400:
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


def mcp_server(engine: str, agent: str) -> Dict[str, Any]:
    # A call that hangs gives up after two minutes, not Hermes' five; get_instance_state hangs on some pages (the agent
    # has get_active_tab and list_tabs).
    return {"command": engine, "args": ["exec", "-i", container(agent), "/opt/stealth/mcp.sh"], "connect_timeout": 90,
            "timeout": 120, "tools": {"exclude": ["get_instance_state"]}}


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
    ports = ports_of(config, agent.name) or pick_ports(config, is_free)
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
    await run(["systemctl", "--user", "enable", "--now", unit.stem], {})
    if changed:   # set up before with other flags: its container starts again with these (its profile stays)
        await run(["systemctl", "--user", "restart", unit.stem], {})

    env = {"HERMES_HOME": home}
    hermes = config.hermes.bin
    await run([hermes, "-p", agent.name, "config", "set", "browser.cdp_url", f"http://127.0.0.1:{ports['cdp']}"], env)
    await run([hermes, "-p", agent.name, "config", "set", "mcp_servers.stealth", json.dumps(mcp_server(engine, agent.name))],
              env)
    # The vault's tools ride with Hermes' browser toolset; they act on this Chrome through its DevTools port.
    await run([hermes, "-p", agent.name, "tools", "enable", "browser", "--platform", "api_server"], env)
    skill = Path(home) / "profiles" / agent.name / "skills" / "visible-browser" / "SKILL.md"
    skill.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(SKILL, skill)

    agent.desktop_vnc = f"tcp://127.0.0.1:{ports['vnc']}"
    agent.desktop_vnc_password_file = str(password)
    log.info("browser set up", extra=fields(agent=agent.name, cdp=ports["cdp"], vnc=ports["vnc"]))
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
