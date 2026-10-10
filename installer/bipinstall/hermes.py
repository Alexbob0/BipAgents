"""Hermes for one person: the official install, one multiplex gateway (the default profile listens, every agent is a
named profile reached under /p/<agent>/ with its own key), the model, and each agent's personality.

Everything goes through Hermes' own commands (`hermes config set`, `hermes profile create`, `hermes gateway
install`), so an update of Hermes keeps working. Commands are collected by a Runner: the real one runs them, the
dry one prints them (and tests read them)."""
from __future__ import annotations

import os
import secrets
import shlex
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, Iterable, List, Optional, Sequence

import yaml  # PyYAML, installed with the installer

INSTALL_URL = "https://hermes-agent.nousresearch.com/install.sh"
# The Hermes version BipAgents is tested with: its patches (hermes/patches/) apply cleanly there. Override with
# BIPAGENTS_HERMES_COMMIT (empty = Hermes' own default, without guarantee for the patches).
HERMES_COMMIT = os.environ.get("BIPAGENTS_HERMES_COMMIT", "517b5e10f")
HUB_PROFILE = "default"


@dataclass
class Model:
    provider: str            # "openrouter" | "openai-api" | "anthropic" | "custom" | "lmstudio"
    name: str                # the model id
    base_url: Optional[str] = None
    api_key: Optional[str] = None
    vision: bool = False

    @property
    def key_env(self) -> Optional[str]:
        return {"openrouter": "OPENROUTER_API_KEY", "openai-api": "OPENAI_API_KEY",
                "anthropic": "ANTHROPIC_API_KEY", "lmstudio": "LM_API_KEY"}.get(self.provider,
                                                                                   "CUSTOM_MODEL_API_KEY" if self.api_key else None)


@dataclass
class Runner:
    dry: bool = False
    env: Dict[str, str] = field(default_factory=dict)
    log: List[str] = field(default_factory=list)

    def run(self, args: Sequence[str], *, input_text: Optional[str] = None, shell: bool = False, check: bool = True) -> str:
        shown = args if isinstance(args, str) else " ".join(shlex.quote(a) for a in args)
        self.log.append(shown)
        if self.dry:
            return ""
        result = subprocess.run(args, input=input_text, text=True, capture_output=True, shell=shell,
                                env={**os.environ, **self.env}, check=False)
        if result.returncode != 0 and check:
            raise RuntimeError(f"{shown} failed ({result.returncode}): {result.stderr.strip()[-600:]}")
        return result.stdout


def hermes_bin() -> str:
    found = Path.home() / ".local" / "bin" / "hermes"
    return str(found)


def hermes_home() -> Path:
    return Path(os.environ.get("HERMES_HOME", Path.home() / ".hermes"))


def profile_dir(name: str) -> Path:
    return hermes_home() if name == HUB_PROFILE else hermes_home() / "profiles" / name


def new_key() -> str:
    return secrets.token_urlsafe(32)


def install(runner: Runner, *, browser: bool) -> None:
    """The official installer, without questions; Hermes' own browser only when this machine can afford it."""
    if Path(hermes_bin()).exists():
        return
    flags = ["--non-interactive", "--skip-computer-use"] + ([] if browser else ["--skip-browser"])
    if HERMES_COMMIT:
        flags += ["--commit", HERMES_COMMIT]
    runner.run(f"curl -fsSL {INSTALL_URL} | bash -s -- {' '.join(flags)}", shell=True)


def hermes_checkout() -> Path:
    return Path(os.environ.get("HERMES_INSTALL_DIR", hermes_home() / "hermes-agent"))


def apply_patches(runner: Runner, patches: Path) -> List[str]:
    """BipAgents' Hermes patches (questions from the agent in the app, photos kept for follow-up questions), applied
    once, before the gateway first starts. Returns the patches that could not be applied (the app then works without
    those features)."""
    checkout = hermes_checkout()
    skipped: List[str] = []
    for patch in sorted(patches.glob("*.patch")):
        applied = ["git", "-C", str(checkout), "apply", "--reverse", "--check", str(patch)]
        if not runner.dry and subprocess.run(applied, capture_output=True).returncode == 0:
            continue   # already there (a second run of the installer)
        try:
            runner.run(["git", "-C", str(checkout), "apply", "--check", str(patch)])
            runner.run(["git", "-C", str(checkout), "apply", str(patch)])
        except RuntimeError:
            skipped.append(patch.name)
    return skipped


def _config(runner: Runner, profile: str, key: str, value: str) -> None:
    args = [hermes_bin()] + ([] if profile == HUB_PROFILE else ["-p", profile]) + ["config", "set", key, value]
    runner.run(args)


def set_model(runner: Runner, profile: str, model: Model) -> None:
    _config(runner, profile, "model.provider", model.provider)
    _config(runner, profile, "model.default", model.name)
    if model.base_url:
        _config(runner, profile, "model.base_url", model.base_url)
    if model.provider in ("custom",) and model.api_key:
        _config(runner, profile, "model.key_env", model.key_env or "CUSTOM_MODEL_API_KEY")
    if model.api_key and model.key_env:
        _config(runner, profile, model.key_env, model.api_key)   # UPPER_SNAKE: written to the profile's .env
    _config(runner, profile, "model.supports_vision", "true" if model.vision else "false")


def setup_hub(runner: Runner, port: int, model: Model) -> str:
    """The default profile: the one listener (127.0.0.1, published on the tailnet by `tailscale serve`)."""
    key = new_key()
    _config(runner, HUB_PROFILE, "API_SERVER_ENABLED", "true")
    _config(runner, HUB_PROFILE, "API_SERVER_HOST", "127.0.0.1")
    _config(runner, HUB_PROFILE, "API_SERVER_PORT", str(port))
    _config(runner, HUB_PROFILE, "API_SERVER_KEY", key)
    _config(runner, HUB_PROFILE, "gateway.multiplex_profiles", "true")
    set_model(runner, HUB_PROFILE, model)
    return key


def create_agent(runner: Runner, name: str, *, description: str, soul: str, model: Model) -> str:
    """A named profile served by the hub under /p/<name>/, with its own key, model and personality. Picked up by a
    running gateway within 30 s: no restart."""
    runner.run([hermes_bin(), "profile", "create", name, "--no-alias", "--description", description])
    key = new_key()
    _config(runner, name, "API_SERVER_KEY", key)       # its own key; only the hub enables the api_server
    set_model(runner, name, model)
    if not runner.dry:
        folder = profile_dir(name)
        (folder / "SOUL.md").write_text(soul, encoding="utf-8")
        mark_as_bot(folder)
    return key


def mark_as_bot(folder: Path) -> None:
    """Bot Mode: the agents of an install can message each other (`message_agent`) in their « Bot Chat »."""
    path = folder / "profile.yaml"
    data = yaml.safe_load(path.read_text(encoding="utf-8")) if path.exists() else None
    data = data if isinstance(data, dict) else {}
    data.setdefault("ui_meta", {}).setdefault("hermes-bots", {})
    path.write_text(yaml.safe_dump(data, allow_unicode=True, sort_keys=False), encoding="utf-8")


def install_service(runner: Runner) -> None:
    """launchd (macOS) or a systemd user unit (Linux), written by Hermes itself; started now and at login."""
    runner.run([hermes_bin(), "gateway", "install", "--start-now", "--start-on-login"])


def profile_name(display_name: str, taken: Iterable[str] = ()) -> str:
    """« Léa » → « lea », « Mon coach » → « mon-coach »: the profile (and bridge) id of a name the person chose."""
    import unicodedata
    plain = unicodedata.normalize("NFKD", display_name).encode("ascii", "ignore").decode().lower()
    slug = "-".join("".join(c if c.isalnum() else " " for c in plain).split())[:32] or "agent"
    if slug == HUB_PROFILE:
        slug = "agent-default"
    name, n = slug, 2
    while name in set(taken):
        name, n = f"{slug}-{n}", n + 1
    return name


def agent_url(base: str, name: str) -> str:
    return f"{base.rstrip('/')}/p/{name}"
