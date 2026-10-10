"""An agent's own space: its own Hermes, apart from the shared hub, so it may hold credentials.

In the hub, every agent runs in one gateway under one account: an agent with a terminal could read another's
files, vault included. Moving an agent gives it:

- its own HERMES_HOME (`<spaces root>/<agent>`) and gateway, on a local port of its own; the app reaches it
  through the bridge (`/hermes/<agent>/`), so no new port is published on the tailnet;
- with podman, its terminal, files, code and browser in a sandbox container (Hermes' `docker` terminal backend,
  which runs podman), and attachments limited to its own files (`gateway.strict`);
- its conversations, memory, routines, personality and settings, copied from the hub (the database through
  SQLite's backup, consistent while the hub keeps running), then the hub forgets it.

The service that runs the space's gateway is a systemd user unit (Linux) or a launchd agent (macOS), or the
administrator's own commands (`[spaces] start` / `stop`), e.g. a container per space on a server where Hermes
itself runs in containers.
"""
from __future__ import annotations

import asyncio
import os
import platform
import secrets
import shlex
import shutil
import socket
import sqlite3
import logging
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

import httpx

from .config import AgentConfig, Config
from .logs import fields

log = logging.getLogger("bipbridge.spaces")

# Runtime files of a running gateway or sandbox: never copied (the space starts its own).
SKIP = {"sandboxes", "bot-desktop", "gateway.pid", "gateway.lock", "gateway.sock", "gateway_state.json",
        "state.db", "state.db-wal", "state.db-shm"}


class SpaceError(Exception):
    """Safe to show."""


def unit_name(agent: str) -> str:
    return f"bipagents-space-{agent}"


def systemd_unit(agent: str, hermes: str, home: str) -> str:
    env = {"HERMES_HOME": home, "PYTHONUNBUFFERED": "1",
           "PATH": f"{Path.home()}/.local/bin:/usr/local/bin:/usr/bin:/bin"}
    lines = ["[Unit]", f"Description=BipAgents space of {agent} (Hermes gateway)", "", "[Service]",
             f"ExecStart={shlex.join([hermes, 'gateway', 'run'])}", f"WorkingDirectory={home}", "Restart=on-failure",
             "RestartSec=5"] + [f"Environment={k}={v}" for k, v in env.items()] + ["", "[Install]", "WantedBy=default.target", ""]
    return "\n".join(lines)


def launchd_plist(agent: str, hermes: str, home: str, log_path: str) -> str:
    env = {"HERMES_HOME": home, "PYTHONUNBUFFERED": "1",
           "PATH": f"{Path.home()}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"}
    args = "".join(f"<string>{a}</string>" for a in (hermes, "gateway", "run"))
    envs = "".join(f"<key>{k}</key><string>{v}</string>" for k, v in env.items())
    return ('<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" '
            '"http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict>'
            f"<key>Label</key><string>io.github.bipagents.space.{agent}</string>"
            f"<key>ProgramArguments</key><array>{args}</array>"
            f"<key>EnvironmentVariables</key><dict>{envs}</dict>"
            f"<key>WorkingDirectory</key><string>{home}</string>"
            "<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>"
            f"<key>StandardOutPath</key><string>{log_path}</string><key>StandardErrorPath</key><string>{log_path}</string>"
            "</dict></plist>\n")


def free_port(config: Config) -> int:
    taken = set()
    for agent in config.agents.values():
        try:
            taken.add(httpx.URL(agent.hermes_url).port)
        except Exception:
            pass
    port = config.spaces.first_port
    while port < config.spaces.first_port + 200:
        if port not in taken:
            with socket.socket() as probe:
                if probe.connect_ex(("127.0.0.1", port)) != 0:
                    return port
        port += 1
    raise SpaceError("no free port for a new space")


def set_env(path: Path, values: Dict[str, str]) -> None:
    """KEY=value lines of a .env: the given ones replaced or added, the others kept."""
    lines = path.read_text(encoding="utf-8").splitlines() if path.exists() else []
    kept = [line for line in lines if line.split("=", 1)[0].strip() not in values]
    path.write_text("\n".join(kept + [f"{k}={v}" for k, v in values.items()]) + "\n", encoding="utf-8")
    os.chmod(path, 0o600)


def copy_profile(source: Path, target: Path) -> None:
    """The agent's folder, its database included (SQLite backup: consistent while the hub gateway writes)."""
    shutil.copytree(source, target, symlinks=True,   # links stay links, even dangling ones
                    ignore=lambda folder, names: [n for n in names if n in SKIP or n.endswith(".sock")])
    database = source / "state.db"
    if database.exists():
        src = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
        dst = sqlite3.connect(target / "state.db")
        with dst:
            src.backup(dst)
        src.close()
        dst.close()


def sandbox_available(config: Config) -> bool:
    if config.spaces.sandbox == "none":
        return False
    return bool(shutil.which("podman") or shutil.which("docker"))


class Spaces:
    def __init__(self, run: Callable[..., Any]):
        self.run = run   # run(args, env) -> stdout, raises RuntimeError

    async def _service(self, config: Config, agent: str, home: str, start: bool) -> None:
        spaces = config.spaces
        if spaces.start and start:
            await self.run(["sh", "-c", spaces.start.format(agent=agent, home=shlex.quote(home))], {})
            return
        if spaces.stop and not start:
            await self.run(["sh", "-c", spaces.stop.format(agent=agent, home=shlex.quote(home))], {})
            return
        if platform.system() == "Darwin":
            path = Path.home() / "Library" / "LaunchAgents" / f"io.github.bipagents.space.{agent}.plist"
            domain = f"gui/{os.getuid()}"
            if start:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(launchd_plist(agent, config.hermes.bin, home,
                                              str(Path.home() / "Library" / "Logs" / f"bipagents-space-{agent}.log")))
                await self.run(["launchctl", "bootstrap", domain, str(path)], {})
            else:
                await self.run(["launchctl", "bootout", f"{domain}/io.github.bipagents.space.{agent}"], {})
                path.unlink(missing_ok=True)
            return
        path = Path.home() / ".config" / "systemd" / "user" / f"{unit_name(agent)}.service"
        if start:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(systemd_unit(agent, config.hermes.bin, home))
            await self.run(["systemctl", "--user", "daemon-reload"], {})
            await self.run(["systemctl", "--user", "enable", "--now", unit_name(agent)], {})
        else:
            await self.run(["systemctl", "--user", "disable", "--now", unit_name(agent)], {})
            path.unlink(missing_ok=True)
            await self.run(["systemctl", "--user", "daemon-reload"], {})

    async def move(self, config: Config, agent: AgentConfig, http: httpx.AsyncClient) -> AgentConfig:
        """The agent, from the hub into its own space. Returns its new configuration (the caller saves it)."""
        hub = Path(config.hermes.home or Path.home() / ".hermes")
        source = hub / "profiles" / agent.name
        if agent.isolated or not source.is_dir():
            raise SpaceError("this agent is not in the shared hub")
        root = Path(config.spaces.root) / agent.name
        if root.exists():
            raise SpaceError("a space with this name already exists")
        port = free_port(config)
        space_key = secrets.token_urlsafe(32)
        root.mkdir(parents=True, mode=0o700)
        try:
            # The hub's settings (model, providers), its own listener.
            for name in ("config.yaml", ".env"):
                if (hub / name).exists():
                    shutil.copy2(hub / name, root / name)
            set_env(root / ".env", {"API_SERVER_ENABLED": "true", "API_SERVER_HOST": "127.0.0.1",
                                    "API_SERVER_PORT": str(port), "API_SERVER_KEY": space_key})
            copy_profile(source, root / "profiles" / agent.name)
            env = {"HERMES_HOME": str(root)}
            sandboxed = bool(config.spaces.start) or sandbox_available(config)
            # Attachments only from its own files (a routed profile reads this from its own config).
            await self.run([config.hermes.bin, "-p", agent.name, "config", "set", "gateway.strict", "true"], env)
            if not config.spaces.start and sandbox_available(config):
                # Terminal, files, code and the browser's desktop in a container; nothing of the host mounted.
                await self.run([config.hermes.bin, "-p", agent.name, "config", "set", "terminal.backend", "docker"], env)
            await self._service(config, agent.name, str(root), start=True)
            moved = AgentConfig(**{**agent.__dict__})
            moved.hermes_url = f"http://127.0.0.1:{port}/p/{agent.name}"
            moved.public_url = f"{config.public_url}/hermes/{agent.name}" if config.public_url else None
            moved.isolated = True
            moved.sandboxed = sandboxed
            moved.space = str(root)
            moved.python = list(config.hermes.python)
            moved.hermes_root = config.hermes.root or None
            moved.hermes_home = str(root / "profiles" / agent.name)
            moved.upload_dir_host = moved.upload_dir_container = str(root / "profiles" / agent.name / "uploads")
            headers = {"Authorization": f"Bearer {agent.hermes_key}"}
            for _ in range(120):
                try:
                    resp = await http.get(f"{moved.hermes_url}/v1/capabilities", headers=headers, timeout=3)
                    if resp.status_code == 200:
                        break
                except httpx.HTTPError:
                    pass
                await asyncio.sleep(1)
            else:
                raise SpaceError("the new space did not start")
        except Exception:
            try:
                await self._service(config, agent.name, str(root), start=False)
            except (RuntimeError, OSError, asyncio.TimeoutError):
                pass
            shutil.rmtree(root, ignore_errors=True)
            raise
        # Served by its own gateway now: the hub forgets it.
        await self.run([config.hermes.bin, "profile", "delete", agent.name, "--yes"],
                       {"HERMES_HOME": config.hermes.home} if config.hermes.home else {})
        log.info("agent moved into its own space", extra=fields(agent=agent.name, port=port,
                                                                 sandbox=sandbox_available(config)))
        return moved

    async def remove(self, config: Config, agent: AgentConfig) -> None:
        """A deleted agent's space: its gateway stopped, its folder gone."""
        if not agent.space:
            return
        try:
            await self._service(config, agent.name, agent.space, start=False)
        except (RuntimeError, OSError, asyncio.TimeoutError):
            log.warning("space service not stopped", extra=fields(agent=agent.name))
        shutil.rmtree(agent.space, ignore_errors=True)
