"""Read-only data between agents: « Vie peut lire ce que Wellness partage ».

Each agent whose tools run in a sandbox (Hermes' `docker` terminal backend) can have a folder it shares,
`<data>/shares/<agent>/`, seen in its sandbox as `/partage` (it writes there what others may read). A link
reader ← source mounts the source's folder in the reader's sandbox, read-only, as `/lecture/<source>`. Nothing else
of either agent is visible to the other: not its memory, conversations, keys or other files.

The mounts are Hermes' own `terminal.docker_volumes` of each profile: entries the person added by hand are kept.
`<data>/shares.json` holds the links. The sandboxes pick the new mounts up when they start again (they are recreated
when their mounts change).
"""
from __future__ import annotations

import json
import logging
import os
import shutil
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Set, Tuple

import yaml

from .config import AgentConfig, Config
from .logs import fields

log = logging.getLogger("bipbridge.shares")

SKILL = (Path(__file__).with_name("skills") / "shared-data" / "SKILL.md").read_text(encoding="utf-8")

def shares_root(config: Config) -> Path:
    return Path(config.outbox.db_path).parent / "shares"


def store_path(config: Config) -> Path:
    return Path(config.outbox.db_path).parent / "shares.json"


def load(config: Config) -> List[Tuple[str, str]]:
    try:
        data = json.loads(store_path(config).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return []
    return [tuple(x) for x in data.get("reads", []) if isinstance(x, list) and len(x) == 2]   # type: ignore[misc]


def save(config: Config, reads: List[Tuple[str, str]]) -> None:
    path = store_path(config)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"reads": [list(x) for x in sorted(set(reads))]}) + "\n", encoding="utf-8")


def profile_dir(config: Config, agent: AgentConfig) -> Optional[Path]:
    """The agent's Hermes profile, when this install manages it (its hub, or its own space)."""
    hub = config.hermes.hub_url
    if not config.hermes.enabled or not (agent.space or agent.hermes_url.startswith(f"{hub}/p/")):
        return None
    home = agent.space or config.hermes.home or str(Path.home() / ".hermes")
    return Path(home) / "profiles" / agent.name


def _profile_config(folder: Path) -> Dict[str, Any]:
    try:
        data = yaml.safe_load((folder / "config.yaml").read_text(encoding="utf-8"))
    except (OSError, yaml.YAMLError):
        return {}
    return data if isinstance(data, dict) else {}


def why_not(config: Config, agent: AgentConfig) -> Optional[str]:
    """None when the agent can share and read; otherwise why not: `not_managed` (set up by hand: its
    administrator mounts folders), `no_sandbox` (its tools run on this machine, where a read-only mount means
    nothing). An agent set up by hand with `shares = "external"` takes part: its administrator mounts what
    `mounts()` lists when it starts."""
    if agent.shares == "external":
        return None
    folder = profile_dir(config, agent)
    if folder is None:
        return "not_managed"
    if terminal_backend(folder) != "docker":
        return "no_sandbox"
    return None


def _env_value(path: Path, key: str) -> Optional[str]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return None
    for line in lines:
        name, sep, value = line.strip().partition("=")
        if sep and name.strip() == key:
            return value.strip().strip("'\"") or None
    return None


def terminal_backend(folder: Path) -> Optional[str]:
    """Where the agent's tools run, as its gateway sees it: the profile's own setting (config.yaml, then its .env's
    TERMINAL_ENV), else what it gets from the gateway's home (`hermes profile create` without --clone copies none of
    it: such a profile runs in the hub's sandbox through the hub's environment)."""
    home = folder.parent.parent
    for config_dir in (folder, home):
        terminal = _profile_config(config_dir).get("terminal")
        if isinstance(terminal, dict) and terminal.get("backend"):
            return str(terminal["backend"])
        env = _env_value(config_dir / ".env", "TERMINAL_ENV")
        if env:
            return env
    return None


def volumes_for(config: Config, agent: str, reads: List[Tuple[str, str]], current: List[str]) -> List[str]:
    """The agent's docker_volumes: the person's own entries, then its shared folder (when another agent reads it)
    and what it may read."""
    root = str(shares_root(config))
    kept = [v for v in current if not str(v).startswith(root + "/")]
    ours: List[str] = []
    if any(source == agent for _, source in reads):
        # `z`: a label every container may read (SELinux, as on Fedora: without it the sandbox can't open the files).
        ours.append(f"{root}/{agent}:/partage:z")
    for reader, source in sorted(reads):
        if reader == agent:
            ours.append(f"{root}/{source}:/lecture/{source}:ro,z")
    return kept + ours


async def apply(config: Config, wanted: List[Tuple[str, str]], run: Callable[..., Any]) -> List[Tuple[str, str]]:
    """The person's read links, set up in every agent concerned (now or before). Raises ValueError for an unknown
    agent, or one that cannot share (see `why_not`)."""
    for reader, source in wanted:
        if reader == source:
            raise ValueError("an agent always reads its own files")
        for name in (reader, source):
            agent = config.agent(name)
            if agent is None:
                raise ValueError(f"unknown agent {name}")
            reason = why_not(config, agent)
            if reason:
                raise ValueError(f"{name}: {reason}")
    before = load(config)
    concerned: Set[str] = {n for pair in before + wanted for n in pair}
    for source in sorted({s for _, s in wanted}):
        share = shares_root(config) / source
        share.mkdir(parents=True, exist_ok=True)
        # Open to the sandboxes' own user: in a rootless sandbox this account is « root », the agent another user,
        # which a 700 folder shuts out (its own agent can't write there, its readers can't read). Nothing else
        # reaches it: the person's home is closed to other accounts, and only these sandboxes mount it.
        os.chmod(share, 0o777)
    changed_external = []
    for name in sorted(concerned):
        agent = config.agent(name)
        if agent is not None and agent.shares == "external":
            if mounts(config, name, before) != mounts(config, name, wanted):
                changed_external.append(agent)
            continue
        folder = profile_dir(config, agent) if agent else None
        if agent is None or folder is None or not folder.is_dir():
            continue
        current = (_profile_config(folder).get("terminal") or {}).get("docker_volumes") or []
        volumes = volumes_for(config, name, wanted, [str(v) for v in current] if isinstance(current, list) else [])
        home = str(folder.parent.parent)
        await run([config.hermes.bin, "-p", name, "config", "set", "terminal.docker_volumes", json.dumps(volumes)],
                  {"HERMES_HOME": home})
        skill = folder / "skills" / "shared-data" / "SKILL.md"
        if any(name in pair for pair in wanted):
            skill.parent.mkdir(parents=True, exist_ok=True)
            skill.write_text(SKILL, encoding="utf-8")
        else:
            shutil.rmtree(skill.parent, ignore_errors=True)
    save(config, wanted)
    for agent in changed_external:
        # Its administrator's command (e.g. restart its container when it is idle): what it mounts changed.
        if agent.shares_apply:
            try:
                await run(["sh", "-c", agent.shares_apply.format(agent=agent.name)], {})
            except RuntimeError as exc:
                log.warning("shares not applied", extra=fields(agent=agent.name, error=str(exc)[-200:]))
    log.info("agent shares set", extra=fields(reads=len(wanted)))
    return sorted(set(wanted))


def mounts(config: Config, agent: str, reads: Optional[List[Tuple[str, str]]] = None) -> List[str]:
    """What an agent set up by hand mounts (`podman run -v …`): its shared folder when another agent reads it, and
    what it may read, read-only. `python -m bipbridge shares-mounts --agent <name>` prints them."""
    reads = load(config) if reads is None else reads
    return volumes_for(config, agent, reads, [])


async def forget(config: Config, agent: str, run: Callable[..., Any]) -> None:
    """A deleted agent (already out of the config): the others stop reading it, and its shared folder goes."""
    reads = load(config)
    if any(agent in pair for pair in reads):
        await apply(config, [pair for pair in reads if agent not in pair], run)
    shutil.rmtree(shares_root(config) / agent, ignore_errors=True)
