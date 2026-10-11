"""BipAgents on a Hermes the person already has: their model, tools, platforms (Telegram…), agents, memory and
conversations stay as they are.

What is read: the version, the profiles, the local API (on or off, its port and key), each profile's terminal (a
sandbox or this machine), the model (shown only), how the gateway runs. What is changed, and written down in
`~/.config/bipagents/existing-hermes.json` so `bipinstall uninstall` puts it back:

- the local API of the gateway, when it is off: on, on 127.0.0.1, on this install's port, with a new key;
- each profile shown in the app gets its own key when it has none (the multiplex gateway serves it at /p/<profile>/);
- nothing else: no model, terminal, tools or personality is touched.
"""
from __future__ import annotations

import json
import re
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional, Tuple

import yaml

from .hermes import HUB_PROFILE, Runner, hermes_bin, hermes_checkout, hermes_home, new_key

SANDBOX_BACKENDS = ("docker", "singularity", "modal", "daytona", "ssh", "vercel")
RECORD = "existing-hermes.json"


@dataclass
class Profile:
    name: str                    # "default" for the main profile
    api_key: Optional[str]       # its own API_SERVER_KEY, if it has one
    terminal: str                # terminal.backend
    description: str = ""

    @property
    def sandboxed(self) -> bool:
        return self.terminal in SANDBOX_BACKENDS


@dataclass
class Existing:
    home: Path
    version: str
    commit: Optional[str]
    multiplex: bool              # this Hermes serves several profiles under /p/<profile>/
    api_enabled: bool
    api_host: Optional[str]
    api_port: Optional[int]
    profiles: List[Profile] = field(default_factory=list)
    model: str = ""
    gateway: str = "stopped"     # "service" | "manual" | "stopped"

    @property
    def main(self) -> Profile:
        return next(p for p in self.profiles if p.name == HUB_PROFILE)

    @property
    def protected(self) -> bool:
        """Every profile's tools run in a sandbox: none can read the bridge's files (its keys, the vault)."""
        return all(p.sandboxed for p in self.profiles)


def read_env(path: Path) -> Dict[str, str]:
    values: Dict[str, str] = {}
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return values
    for line in lines:
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            key, _, value = line.partition("=")
            values[key.strip().removeprefix("export ").strip()] = value.strip().strip('"').strip("'")
    return values


def read_yaml(path: Path) -> dict:
    try:
        data = yaml.safe_load(path.read_text(encoding="utf-8"))
    except (OSError, yaml.YAMLError):
        return {}
    return data if isinstance(data, dict) else {}


def _profile(name: str, folder: Path) -> Profile:
    config = read_yaml(folder / "config.yaml")
    terminal = str((config.get("terminal") or {}).get("backend") or "local")
    meta = read_yaml(folder / "profile.yaml")
    return Profile(name=name, api_key=read_env(folder / ".env").get("API_SERVER_KEY") or None, terminal=terminal,
                   description=str(meta.get("description") or ""))


def _run(args: List[str]) -> str:
    try:
        return subprocess.run(args, capture_output=True, text=True, timeout=30).stdout
    except (OSError, subprocess.SubprocessError):
        return ""


def gateway_state(run=_run) -> str:
    """How this person's gateway runs: their service (systemd user unit or LaunchAgent written by `hermes gateway
    install`), a process of theirs started by hand, or not at all. (`hermes gateway status` also counts other
    accounts' gateways on the machine.)"""
    import os
    units = list((Path.home() / ".config" / "systemd" / "user").glob("hermes-gateway*.service"))
    agents = list((Path.home() / "Library" / "LaunchAgents").glob("ai.hermes.gateway*.plist"))
    if units or agents:
        return "service"
    found = run(["pgrep", "-u", str(os.getuid()), "-f", "hermes.*gateway run"])
    return "manual" if found.strip() else "stopped"


def detect_existing(home: Optional[Path] = None, run=_run) -> Optional["Existing"]:
    """The person's Hermes, or None when there is none to keep."""
    home = home or hermes_home()
    if not Path(hermes_bin()).exists() or not (home / "config.yaml").exists():
        return None
    env = read_env(home / ".env")
    config = read_yaml(home / "config.yaml")
    version = (run([hermes_bin(), "--version"]).splitlines() or ["?"])[0].strip()
    commit = _run(["git", "-C", str(hermes_checkout()), "rev-parse", "--short=9", "HEAD"]).strip() or None
    defaults = hermes_checkout() / "hermes_cli" / "config_defaults.py"
    multiplex = defaults.exists() and "multiplex_profiles" in defaults.read_text(encoding="utf-8", errors="replace")
    profiles = [_profile(HUB_PROFILE, home)]
    for folder in sorted((home / "profiles").iterdir()) if (home / "profiles").is_dir() else []:
        if (folder / "config.yaml").exists():
            profiles.append(_profile(folder.name, folder))
    gateway = gateway_state(run)
    model = config.get("model") or {}
    port = env.get("API_SERVER_PORT", "")
    return Existing(home=home, version=version, commit=commit, multiplex=multiplex,
                    api_enabled=env.get("API_SERVER_ENABLED", "").lower() == "true",
                    api_host=env.get("API_SERVER_HOST") or None, api_port=int(port) if port.isdigit() else None,
                    profiles=profiles, gateway=gateway,
                    model=str(model.get("default") or model.get("model") or "") if isinstance(model, dict) else str(model))


def plan_changes(existing: Existing, shown: List[str], port: int) -> List[Tuple[str, str, str, Optional[str]]]:
    """(profile, key, new value, value before or None) for each setting to write, nothing else."""
    changes: List[Tuple[str, str, str, Optional[str]]] = []
    env = read_env(existing.home / ".env")
    if not existing.api_enabled:
        for key, value in (("API_SERVER_ENABLED", "true"), ("API_SERVER_HOST", "127.0.0.1"),
                           ("API_SERVER_PORT", str(port))):
            changes.append((HUB_PROFILE, key, value, env.get(key)))
    if not existing.main.api_key:
        changes.append((HUB_PROFILE, "API_SERVER_KEY", new_key(), None))
    for profile in existing.profiles:
        if profile.name in shown and profile.name != HUB_PROFILE and not profile.api_key:
            changes.append((profile.name, "API_SERVER_KEY", new_key(), None))
    return changes


def apply_changes(runner: Runner, changes: List[Tuple[str, str, str, Optional[str]]], record: Path,
                  gateway: str, patched: Optional[List[str]] = None) -> None:
    """Writes them through Hermes' own `config set`, after noting what to put back: the settings, the patches
    applied to its code, how the gateway ran."""
    runner.log.append(f"write {record}")
    if not runner.dry:
        record.parent.mkdir(parents=True, exist_ok=True)
        record.write_text(json.dumps({"preexisting": True, "gateway": gateway, "patched": patched or [], "restore": [
            {"profile": p, "key": k, "before": before} for p, k, _, before in changes]}, indent=2) + "\n",
                          encoding="utf-8")
        record.chmod(0o600)   # the values before may include a key
    for profile, key, value, _ in changes:
        args = [hermes_bin()] + ([] if profile == HUB_PROFILE else ["-p", profile]) + ["config", "set", key, value]
        runner.run(args)


def restore(runner: Runner, record: Path) -> Optional[dict]:
    """`bipinstall uninstall` on a Hermes that was there before: only our settings are undone. Returns what was
    recorded (None: Hermes came with BipAgents)."""
    try:
        data = json.loads(record.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    for item in data.get("restore", []):
        profile = [] if item["profile"] == HUB_PROFILE else ["-p", item["profile"]]
        if item.get("before") is None:
            runner.run([hermes_bin()] + profile + ["config", "unset", item["key"]], check=False)
        else:
            runner.run([hermes_bin()] + profile + ["config", "set", item["key"], item["before"]], check=False)
    for patch in reversed(data.get("patched", [])):
        runner.run(["git", "-C", str(hermes_checkout()), "apply", "--reverse", patch], check=False)
    return data if data.get("preexisting") else None


def key_of(existing: Existing, changes, profile: str) -> Optional[str]:
    """The key the app uses for a profile: the one it had, or the one just given."""
    for name, key, value, _ in changes:
        if name == profile and key == "API_SERVER_KEY":
            return value
    found = next((p for p in existing.profiles if p.name == profile), None)
    return found.api_key if found else None


def same_version(existing: Existing, wanted: str) -> bool:
    return bool(existing.commit and wanted and (existing.commit.startswith(wanted) or wanted.startswith(existing.commit)))


def profile_label(name: str) -> str:
    return re.sub(r"[-_]+", " ", name).strip().title() if name != HUB_PROFILE else "Hermes"
