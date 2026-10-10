"""The rest of one person's install around Hermes: the bridge (its config and service) and the Bips' voices (Pocket,
one server per machine, shared: the first install brings it up, the next ones reuse it)."""
from __future__ import annotations

import json
import secrets
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional
from xml.sax.saxutils import escape

from .hermes import Model, agent_url, profile_dir
from .ports import Ports

POCKET_PORT = 8098


@dataclass
class AgentEntry:
    name: str            # profile name (= key in the bridge config)
    display_name: str
    key: str             # its API_SERVER_KEY
    voice: Optional[str] = None


@dataclass
class Install:
    repo: Path                       # the BipAgents checkout
    home: Path                       # this person's home
    ports: Ports
    tailnet_name: Optional[str]      # e.g. "aibox.tail38a3d9.ts.net"
    lan_address: Optional[str]
    agents: List[AgentEntry] = field(default_factory=list)
    model: Optional[Model] = None
    relay_url: Optional[str] = None  # the administrator's bridge, when this install has no APNs key
    relay_key: Optional[str] = None
    bridge_key: str = field(default_factory=lambda: secrets.token_urlsafe(32))
    kyutai_url: Optional[str] = None
    pocket_url: str = f"http://127.0.0.1:{POCKET_PORT}"

    @property
    def config_dir(self) -> Path:
        return self.home / ".config" / "bipagents"

    @property
    def data_dir(self) -> Path:
        return self.home / ".local" / "share" / "bipagents"

    @property
    def bridge_config(self) -> Path:
        return self.config_dir / "bridge.toml"

    def public(self, port: int) -> Optional[str]:
        return f"https://{self.tailnet_name}:{port}" if self.tailnet_name else None


def _q(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)   # a TOML basic string is a JSON string for these values


def bridge_toml(inst: Install) -> str:
    """The bridge's config for this install: its agents behind the person's multiplex gateway."""
    hub_local = f"http://127.0.0.1:{inst.ports.hermes}"
    hub_public = inst.public(inst.ports.hermes)
    lines = [
        "# Written by the BipAgents installer. Keys here: chmod 600.",
        f"bridge_key = {_q(inst.bridge_key)}",
        'host = "127.0.0.1"',
        f"port = {inst.ports.bridge}",
        f"data_dir = {_q(str(inst.data_dir))}",
    ]
    if inst.public(inst.ports.bridge):
        lines.append(f"public_url = {_q(inst.public(inst.ports.bridge))}")
    lines += ["", "[kyutai]"] + ([f"url = {_q(inst.kyutai_url)}"] if inst.kyutai_url else ["enabled = false"])
    lines += ["", "[pocket]", f"url = {_q(inst.pocket_url)}", 'default_voice = "pocket:colibri"']
    lines += ["", "[cron]", "watch = true"]
    media = inst.home / ".hermes" / "media"
    lines += ["", "[media.roots]", f"{_q(str(media))} = {_q(str(media))}"]   # native Hermes: same path both sides
    if inst.relay_url and inst.relay_key:
        lines += ["", "[push]", f"relay_url = {_q(inst.relay_url)}", f"relay_key = {_q(inst.relay_key)}"]
    if inst.model and inst.model.base_url:
        lines += ["", "[model]", f"health_url = {_q(inst.model.base_url.rstrip('/') + '/models')}"]
        if inst.model.api_key:
            lines.append(f"health_key = {_q(inst.model.api_key)}")
    lines += ["", "[lan]", f"enabled = {'true' if inst.lan_address else 'false'}", f"port = {inst.ports.lan}"]
    if inst.lan_address:
        lines.append(f"address = {_q(inst.lan_address)}")
    for agent in inst.agents:
        uploads = profile_dir(agent.name) / "uploads"
        lines += ["", f"[agents.{agent.name}]", f"display_name = {_q(agent.display_name)}",
                  f"hermes_url = {_q(agent_url(hub_local, agent.name))}", f"hermes_key = {_q(agent.key)}",
                  f"upload_dir_host = {_q(str(uploads))}", f"upload_dir_container = {_q(str(uploads))}"]
        if hub_public:
            lines.append(f"public_url = {_q(agent_url(hub_public, agent.name))}")
        if agent.voice:
            lines.append(f"voice = {_q(agent.voice)}")
    return "\n".join(lines) + "\n"


def launchd_plist(label: str, program: List[str], workdir: Path, env: Dict[str, str], log: Path) -> str:
    args = "".join(f"<string>{escape(a)}</string>" for a in program)
    envs = "".join(f"<key>{escape(k)}</key><string>{escape(v)}</string>" for k, v in env.items())
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{escape(label)}</string>
  <key>ProgramArguments</key><array>{args}</array>
  <key>WorkingDirectory</key><string>{escape(str(workdir))}</string>
  <key>EnvironmentVariables</key><dict>{envs}</dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>StandardOutPath</key><string>{escape(str(log))}</string>
  <key>StandardErrorPath</key><string>{escape(str(log))}</string>
</dict>
</plist>
"""


def systemd_unit(description: str, program: List[str], workdir: Path, env: Dict[str, str]) -> str:
    environment = "".join(f"Environment={k}={v}\n" for k, v in env.items())
    return f"""[Unit]
Description={description}
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory={workdir}
{environment}ExecStart={" ".join(program)}
Restart=on-failure
RestartSec=5
UMask=0027
NoNewPrivileges=true

[Install]
WantedBy=default.target
"""


def bridge_service(inst: Install, system: str) -> tuple[str, Path, str]:
    """(file name, path, content) of the bridge's service for this person."""
    python = inst.repo / "bridge" / ".venv" / "bin" / "python"
    program = [str(python), "-m", "bipbridge", "serve"]
    env = {"BRIDGE_CONFIG": str(inst.bridge_config), "PYTHONUNBUFFERED": "1"}
    if system == "macos":
        path = inst.home / "Library" / "LaunchAgents" / "io.github.bipagents.bridge.plist"
        return "io.github.bipagents.bridge", path, launchd_plist(
            "io.github.bipagents.bridge", program, inst.repo / "bridge", env, inst.home / "Library" / "Logs" / "bipagents-bridge.log")
    path = inst.home / ".config" / "systemd" / "user" / "bipagents-bridge.service"
    return "bipagents-bridge", path, systemd_unit("BipAgents bridge", program, inst.repo / "bridge", env)


def pocket_service(inst: Install, system: str, languages: str) -> tuple[str, Path, str]:
    python = inst.repo / "pocket" / ".venv" / "bin" / "python"
    program = [str(python), str(inst.repo / "pocket" / "pocket_server.py"), "--voices", str(inst.repo / "voices"),
               "--languages", languages, "--port", str(POCKET_PORT)]
    env = {"PYTHONUNBUFFERED": "1"}
    if system == "macos":
        path = inst.home / "Library" / "LaunchAgents" / "io.github.bipagents.pocket.plist"
        return "io.github.bipagents.pocket", path, launchd_plist(
            "io.github.bipagents.pocket", program, inst.repo / "pocket", env, inst.home / "Library" / "Logs" / "bipagents-pocket.log")
    path = inst.home / ".config" / "systemd" / "user" / "bipagents-pocket.service"
    return "bipagents-pocket", path, systemd_unit("BipAgents voices (Pocket TTS)", program, inst.repo / "pocket", env)


def tailscale_commands(cli: str, ports: Ports, desk: bool = False) -> List[str]:
    """What publishes this install on the tailnet (HTTPS with the tailnet's certificate), never on the Internet. The
    agents' desk only when it is installed."""
    return [f"{cli} serve --bg --https={p} http://127.0.0.1:{p}" for p in ports.tailnet if desk or p != ports.desk]
