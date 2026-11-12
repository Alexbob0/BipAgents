"""BipAgents on a Hermes the person already has: their model, tools, platforms (Telegram…), agents, memory and
conversations stay as they are.

What is read: the version, the profiles, the local API (on or off, its port and key), each profile's terminal (a
sandbox or this machine), the model (shown only), how the gateway runs. What is changed, and written down in
`~/.config/bipagents/existing-hermes.json` so `bipinstall uninstall` puts it back:

- the local API of the gateway, when it is off: on, on 127.0.0.1, on this install's port, with a new key;
- each profile shown in the app gets its own key when it has none (the multiplex gateway serves it at /p/<profile>/);
- nothing else: no model, terminal, tools or personality is touched. The bridge, once running, adds only its tools'
  entry (`mcp_servers.bipagents`) to the shown profiles but the main one (profile_files.rs hands_off), and
  `bipinstall uninstall` puts each file back byte for byte from the copies taken here (clean_profiles).
"""
from __future__ import annotations

import json
import re
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional, Tuple

import yaml

from .hermes import HUB_PROFILE, UNFINISHED, Runner, hermes_bin, hermes_checkout, hermes_home, new_key, write_env

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
    if (home / UNFINISHED).exists():
        return None   # the Hermes an earlier run of this installer started and did not finish
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


# The files of a profile BipAgents may change (the installer: the .env's API lines; the bridge: its tools' entry in
# config.yaml), kept as they were before (SNAPSHOT/<profile>/) so uninstalling puts them back byte for byte.
KEPT_FILES = ("config.yaml", ".env", "SOUL.md")
SNAPSHOT = "existing-hermes-files"
# What BipAgents adds to a profile and takes out again when uninstalled (files or folders, relative to the profile):
# the bridge's tools' entries, settings record, lock, temporary files, and what older bridges wrote (styled documents
# skills, Hermes' config backups of its `config set`s, the prompt-cache day file of patch 0023).
OUR_SERVERS = ("bipagents", "bipagents_checkout", "bipagents_files")
OUR_FILES = ("bipagents-settings.json", ".bipagents-profile.lock", "skills/.bipagents-style-skills.json",
             "skills/docs-styles", "skills/slides-styles", "memories/.prompt-day.json")
OUR_ENV = ("API_SERVER_ENABLED", "API_SERVER_HOST", "API_SERVER_PORT", "API_SERVER_KEY", "TERMINAL_DOCKER_VOLUMES")
BLOCK = re.compile(r"\n*<!-- bipagents:([\w-]+) -->.*?<!-- /bipagents:\1 -->[ \t]*\n?", re.S)


def profile_folder(home: Path, profile: str) -> Path:
    return home if profile == HUB_PROFILE else home / "profiles" / profile


def snapshot(home: Path, profiles: List[str], into: Path) -> dict:
    """Each profile's files as they are now (KEPT_FILES copied, 0600) and the Hermes config backups already there:
    what uninstalling compares with."""
    kept: dict = {}
    for profile in profiles:
        folder = profile_folder(home, profile)
        target = into / profile
        target.mkdir(parents=True, exist_ok=True, mode=0o700)
        files = []
        for name in KEPT_FILES:
            if (folder / name).is_file():
                (target / name).write_bytes((folder / name).read_bytes())
                (target / name).chmod(0o600)
                files.append(name)
        backups = folder / "backups" / "config"
        kept[profile] = {"files": files, "backups": sorted(p.name for p in backups.iterdir()) if backups.is_dir() else [],
                         "had": [rel for rel in OUR_FILES if (folder / rel).exists()]}
    return kept


def _without_ours(data) -> dict:
    data = dict(data) if isinstance(data, dict) else {}
    servers = data.get("mcp_servers")
    if isinstance(servers, dict):
        servers = {k: v for k, v in servers.items() if k not in OUR_SERVERS}
        if servers:
            data["mcp_servers"] = servers
        else:
            data.pop("mcp_servers")
    return data


def _strip_blocks(text: str) -> str:
    return BLOCK.sub("\n", text)


def _put_back_config(path: Path, before: Optional[bytes], runner: Runner) -> None:
    if not path.is_file():
        return
    now = read_yaml(path)
    if before is not None:
        try:
            was = yaml.safe_load(before.decode("utf-8")) or {}
        except (UnicodeDecodeError, yaml.YAMLError):
            was = None
        if was is not None and _without_ours(now) == _without_ours(was):
            if path.read_bytes() != before:
                runner.log.append(f"put back {path} as it was")
                if not runner.dry:
                    path.write_bytes(before)
            return
    servers = now.get("mcp_servers")
    if isinstance(servers, dict) and any(k in servers for k in OUR_SERVERS):   # changed since: only ours taken out
        runner.log.append(f"take the bridge's tools out of {path}")
        if not runner.dry:
            path.write_text(yaml.safe_dump(_without_ours(now), allow_unicode=True, sort_keys=False), encoding="utf-8")


def _put_back_soul(path: Path, before: Optional[bytes], runner: Runner) -> None:
    if not path.is_file():
        return
    text = path.read_text(encoding="utf-8", errors="replace")
    stripped = _strip_blocks(text)
    if before is not None and stripped.rstrip() == before.decode("utf-8", "replace").rstrip():
        if path.read_bytes() != before:
            runner.log.append(f"put back {path} as it was")
            if not runner.dry:
                path.write_bytes(before)
    elif stripped != text:
        runner.log.append(f"take the bridge's blocks out of {path}")
        if not runner.dry:
            path.write_text(stripped.rstrip() + "\n", encoding="utf-8")


def _put_back_env(path: Path, before: Optional[bytes], runner: Runner) -> None:
    if not path.is_file() or before is None:
        return
    def theirs(values: Dict[str, str]) -> Dict[str, str]:
        return {k: v for k, v in values.items() if k not in OUR_ENV}
    was_file = path.with_name(".env.bipagents-before")
    try:
        was_file.write_bytes(before)
        was = read_env(was_file)
    finally:
        was_file.unlink(missing_ok=True)
    now = read_env(path)
    ours_before = {k: v for k, v in was.items() if k in OUR_ENV}
    if theirs(now) == theirs(was) and path.read_bytes() != before:
        runner.log.append(f"put back {path} as it was")
        if not runner.dry:
            path.write_bytes(before)
    elif "TERMINAL_DOCKER_VOLUMES" in now and "TERMINAL_DOCKER_VOLUMES" not in ours_before:
        runner.log.append(f"take TERMINAL_DOCKER_VOLUMES out of {path}")
        if not runner.dry:
            lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
            path.write_text("".join(l for l in lines if not l.strip().removeprefix("export ").startswith(
                "TERMINAL_DOCKER_VOLUMES=")), encoding="utf-8")


def clean_profiles(runner: Runner, home: Path, data: dict, snapshots: Optional[Path]) -> None:
    """Every profile put back as before BipAgents (`bipinstall uninstall`, after the bridge stopped): the files it
    changed byte for byte when nothing else changed in them meanwhile (else only what BipAgents added taken out),
    and the files and folders it added removed. A record of an older install (no snapshot): every profile cleaned of
    what BipAgents adds."""
    import shutil
    kept = data.get("snapshot")
    if kept is None:
        names = [HUB_PROFILE] + (sorted(p.name for p in (home / "profiles").iterdir() if p.is_dir())
                                 if (home / "profiles").is_dir() else [])
        kept = {name: {"files": [], "backups": None, "had": []} for name in names}
    for profile, info in kept.items():
        folder = profile_folder(home, profile)
        if not folder.is_dir():
            continue
        def before(name: str) -> Optional[bytes]:
            path = (snapshots / profile / name) if snapshots else None
            return path.read_bytes() if path is not None and name in info.get("files", []) and path.is_file() else None
        _put_back_config(folder / "config.yaml", before("config.yaml"), runner)
        _put_back_soul(folder / "SOUL.md", before("SOUL.md"), runner)
        _put_back_env(folder / ".env", before(".env"), runner)
        for rel in OUR_FILES:
            target = folder / rel
            if rel in info.get("had", []) or not (target.exists() or target.is_symlink()):
                continue
            runner.log.append(f"remove {target}")
            if not runner.dry:
                shutil.rmtree(target) if target.is_dir() and not target.is_symlink() else target.unlink()
        backups = folder / "backups" / "config"
        if info.get("backups") is not None and backups.is_dir():   # Hermes' backups of our `config set`s
            for extra in sorted(set(p.name for p in backups.iterdir()) - set(info["backups"])):
                runner.log.append(f"remove {backups / extra}")
                if not runner.dry:
                    (backups / extra).unlink()
            for empty in (backups, backups.parent):
                if not runner.dry and not info["backups"] and empty.is_dir() and not any(empty.iterdir()):
                    empty.rmdir()
        for leftover in folder.glob(".*.bipagents-tmp"):
            if not runner.dry:
                leftover.unlink(missing_ok=True)


def apply_changes(runner: Runner, changes: List[Tuple[str, str, str, Optional[str]]], record: Path,
                  gateway: str, patched: Optional[List[str]] = None, declined: Optional[List[str]] = None,
                  profiles: Optional[List[str]] = None) -> None:
    """Writes them through Hermes' own `config set`, after noting what to put back: the settings, the patches
    applied to its code, how the gateway ran, the fixes the person refused (not offered again by updates), and each
    shown profile's files as they were (`profiles`, with the main one: snapshot)."""
    runner.log.append(f"write {record}")
    if not runner.dry:
        record.parent.mkdir(parents=True, exist_ok=True)
        shown = list(dict.fromkeys([HUB_PROFILE] + list(profiles or [])))
        kept = snapshot(hermes_home(), shown, record.parent / SNAPSHOT)
        record.write_text(json.dumps({"preexisting": True, "gateway": gateway, "patched": patched or [],
                                      "declined": sorted(declined or []), "snapshot": kept, "restore": [
            {"profile": p, "key": k, "before": before} for p, k, _, before in changes]}, indent=2) + "\n",
                          encoding="utf-8")
        record.chmod(0o600)   # the values before may include a key
    for profile, key, value, _ in changes:
        if key == "API_SERVER_KEY":   # a key: in the profile's .env, never on a command line (audit 11/11, BV-6)
            write_env(runner, profile, key, value)
            continue
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
        elif item["key"] == "API_SERVER_KEY":   # the key of before: in the .env too, never on a command line (BV-6)
            write_env(runner, item["profile"], item["key"], item["before"])
        else:
            runner.run([hermes_bin()] + profile + ["config", "set", item["key"], item["before"]], check=False)
    for patch in reversed(data.get("patched", [])):
        runner.run(["git", "-C", str(hermes_checkout()), "apply", "--reverse", patch], check=False)
    if data.get("preexisting"):
        clean_profiles(runner, hermes_home(), data, record.parent / SNAPSHOT)
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


def bridge_writes(shown: List[str]) -> Optional[str]:
    """What the bridge itself writes, once it runs, in the shown profiles other than the main one: only its tools'
    entry (profile_files.rs hands_off, Alex 12/11 « mains propres »), said before « Ready to go? »."""
    others = [profile_label(p) for p in shown if p != HUB_PROFILE]
    if not others:
        return None
    return (f"{', '.join(others)}: one entry in their config.yaml, the app's tools (mcp_servers.bipagents). Their "
            "personality,\n    thinking level, tools and scheduled jobs stay as they are.")


def profile_label(name: str) -> str:
    return re.sub(r"[-_]+", " ", name).strip().title() if name != HUB_PROFILE else "Hermes"
