"""Bridge configuration (TOML).

Default path: ``~/.config/hermes-ios/bridge.toml``, overridable with ``BRIDGE_CONFIG``.
Secrets may be given inline (``bridge_key``, ``hermes_key``, ``ntfy_token``) or through a file
(``bridge_key_file``, ``hermes_key_file``, ``ntfy_token_file``), e.g. a systemd credential.
"""
from __future__ import annotations

import glob
import logging
import os
import shlex
import stat
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

try:  # Python 3.11+
    import tomllib  # type: ignore[import-not-found]
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]

log = logging.getLogger("bipbridge.config")

DEFAULT_CONFIG_PATH = "~/.config/hermes-ios/bridge.toml"
DEFAULT_DATA_DIR = "~/.local/share/hermes-bridge"
PLACEHOLDER_PREFIXES = ("CHANGE-ME", "CHANGEME", "<")


class ConfigError(ValueError):
    pass


@dataclass
class AgentConfig:
    name: str
    display_name: str
    hermes_url: str
    hermes_key: str
    ntfy_topic: Optional[str] = None
    ntfy_token: Optional[str] = None
    upload_dir_host: Optional[str] = None
    upload_dir_container: Optional[str] = None
    voice: Optional[str] = None
    # The agent's Hermes address as the app reaches it on the tailnet (`tailscale serve`), for the pairing QR code.
    public_url: Optional[str] = None
    # The agent has its own space (container, sandbox): it may hold credentials from the vault. Agents of the
    # shared hub never do.
    isolated: bool = False
    # How to run Python in the agent's Hermes (e.g. `podman exec -i hermes-vie /opt/hermes/venv/bin/python`)
    # and its HERMES_HOME as that Python sees it: the vault writes the agent's items there.
    python: List[str] = field(default_factory=list)
    hermes_home: Optional[str] = None
    hermes_root: Optional[str] = None   # Hermes' code folder, when that Python needs it (`hermes_bootstrap`)
    # Its own space (spaces.py): the HERMES_HOME of the gateway that serves it alone.
    space: Optional[str] = None
    sandboxed: bool = False   # its tools run in a sandbox container (or its space is a container of its own)
    # A viewer page of its browser set up by hand (e.g. a noVNC on the tailnet): the app opens it as « Bureau ».
    desktop_url: Optional[str] = None
    # Its browser's VNC server set up by hand (ws://…/websockify or tcp://host:port) and the file holding its VNC
    # password: the bridge logs in itself and relays the desktop to the app (Face ID there), see desktop.py.
    desktop_vnc: Optional[str] = None
    desktop_vnc_password_file: Optional[str] = None
    # Set up by hand, its administrator mounts the shared folders (shares.py) when it starts: `"external"`, and the
    # command to run after the person changes them (e.g. restarting its container once it is idle).
    shares: str = ""
    shares_apply: Optional[str] = None
    # Set up by hand: the HERMES_HOME of the gateway serving it, on this machine (its profile is profiles/<name>).
    # The app may then change its personality, language and thinking — files the bridge writes there; its tools,
    # its container and its deletion stay with its administrator.
    host_home: Optional[str] = None


@dataclass
class ApnsConfig:
    team_id: str = ""
    key_id: str = ""
    p8_path: str = ""
    bundle_id: str = "io.github.bipagents"
    environment: str = "production"  # default for devices registered without one

    @property
    def enabled(self) -> bool:
        return bool(self.team_id and self.key_id and self.p8_path and self.bundle_id)


@dataclass
class Limits:
    upload_max_bytes: int = 50 * 1024 * 1024
    upload_retention_days: int = 30
    upload_file_mode: int = 0o640
    upload_dir_mode: int = 0o750
    sentence_max_chars: int = 180
    tts_max_chars: int = 1000
    tts_cache_entries: int = 256
    watch_max_seconds: int = 7200
    purge_interval_seconds: int = 6 * 3600


@dataclass
class OutboxConfig:
    db_path: str = DEFAULT_DATA_DIR + "/bridge.db"
    audio_dir: str = DEFAULT_DATA_DIR + "/audio"
    retention_days: int = 90
    audio_max_chars: int = 2000
    audio_wait_seconds: float = 8.0


@dataclass
class HermesInstall:
    """The person's own Hermes, managed by this bridge (an install made by the installer): lets the app create
    agents. Empty `bin` = agents are not managed here (set up by hand, like aibox's containers)."""
    bin: str = ""
    home: str = ""
    hub_url: str = ""            # the multiplex gateway, local (http://127.0.0.1:8642)
    public_hub_url: str = ""     # the same on the tailnet, given to the app
    # Hermes in a container: its HERMES_HOME as the agents see it (`home` is the same folder on the host), and
    # `bin` a wrapper that runs `hermes` inside the container.
    container_home: str = ""
    # How to run Hermes' own Python, and its code folder (the vault writes the agents' items with it).
    python: List[str] = field(default_factory=list)
    root: str = ""
    # The agents cannot read this bridge's files (their tools run in a sandbox, or their Hermes in a container that does
    # not see them). Without it the bridge keeps no credentials: an agent with a terminal or the file tools, running
    # under the same account, could read the vault, its key and every agent's key in this file.
    isolated: bool = False

    @property
    def enabled(self) -> bool:
        return bool(self.bin and self.hub_url)


@dataclass
class SpacesConfig:
    """Agents' own spaces (spaces.py): where they live, their local ports, and how their gateway is started."""
    root: str = DEFAULT_DATA_DIR + "/spaces"
    first_port: int = 8660
    sandbox: str = "auto"            # "auto": podman/docker sandbox when found; "none": never
    start: Optional[str] = None      # own commands instead of systemd/launchd: "{agent}" and "{home}" filled in
    stop: Optional[str] = None
    # A space in a container of its own (start/stop commands): its HERMES_HOME as the agent sees it, for the
    # attachments' folder (e.g. /home/hermes/.hermes). Empty: the same path as on this host.
    container_home: str = ""


@dataclass
class BrowserConfig:
    """Each agent's visible browser (browsers.py): a real Chrome on a screen of its own, in a container of its own."""
    enabled: bool = False            # every agent this install creates gets one
    runtime: str = ""                # "podman" or "docker" (default: the one found)
    image: str = "bipagents-browser"
    first_port: int = 9600           # its DevTools and VNC ports, on 127.0.0.1 only
    root: str = DEFAULT_DATA_DIR + "/browsers"
    gpu: str = "auto"                # "auto": /dev/dri when there is one; "none": software rendering
    language: str = ""               # e.g. "fr-FR" (default: the bridge's locale)
    timezone: str = ""               # e.g. "Europe/Paris" (default: the bridge's)


@dataclass
class RelayConfig:
    """This bridge relays the notifications of other installs on the server, which have no APNs key of their own
    (another person's bridge): each install gets its own key, listed here as key -> install name."""
    clients: Dict[str, str] = field(default_factory=dict)
    per_hour: int = 600

    @property
    def enabled(self) -> bool:
        return bool(self.clients)


@dataclass
class LanConfig:
    """The local-network door (no Tailscale): the same bridge over HTTPS with a self-signed certificate the app pins,
    Hermes relayed under /hermes/<agent>/. Off by default: everything else only listens on 127.0.0.1."""
    enabled: bool = False
    host: str = "0.0.0.0"
    port: int = 8650
    address: Optional[str] = None   # advertised to the app; detected when empty
    cert_dir: str = ""


@dataclass
class Config:
    bridge_key: str
    host: str = "127.0.0.1"
    port: int = 8643
    log_level: str = "info"
    # Kyutai TTS 1.6B (GPU): the human voice and the podcast. None = off (Pocket only, e.g. on a Mac mini or a VPS).
    kyutai_url: Optional[str] = "http://127.0.0.1:8097"
    kyutai_timeout: float = 120.0
    default_voice: str = "5476"
    # Kyutai Pocket TTS (CPU) for the Bips' voices, used by voices named "pocket:<name>". None = off.
    pocket_url: Optional[str] = None
    # Scheduled-task replies read from Hermes sessions into the outbox (see cronwatch.py).
    cron_watch: bool = True
    cron_interval_seconds: float = 30.0
    cron_window_hours: float = 12.0
    # Files the agents point to with « MEDIA:<path> » lines: agent-side prefix -> host directory.
    media_roots: Dict[str, str] = field(default_factory=dict)
    ntfy_url: str = "http://127.0.0.1:8645"
    push_previews: bool = False
    watch_followed_runs: bool = True
    apns: ApnsConfig = field(default_factory=ApnsConfig)
    outbox: OutboxConfig = field(default_factory=OutboxConfig)
    limits: Limits = field(default_factory=Limits)
    agents: Dict[str, AgentConfig] = field(default_factory=dict)
    lan: LanConfig = field(default_factory=LanConfig)
    relay: RelayConfig = field(default_factory=RelayConfig)
    hermes: HermesInstall = field(default_factory=HermesInstall)
    spaces: "SpacesConfig" = field(default_factory=lambda: SpacesConfig())
    browser: BrowserConfig = field(default_factory=BrowserConfig)
    # Notifications through another bridge's relay instead of APNs (an install without the .p8 key).
    push_relay_url: Optional[str] = None
    push_relay_key: Optional[str] = None
    # Optional check that the model answers (`GET <url>`, e.g. its /v1/models), for « model unreachable ».
    model_health_url: Optional[str] = None
    model_health_key: Optional[str] = None
    # The bridge's address on the tailnet (`tailscale serve`), for the pairing QR code.
    public_url: Optional[str] = None
    # The bridge as the agents reach it, for messages between agents (peers.py). Default: http://127.0.0.1:<port>/peer;
    # e.g. http://host.containers.internal:8643/peer for agents in containers.
    peer_url: Optional[str] = None
    source_path: Optional[str] = None

    def agent(self, name: Optional[str]) -> Optional[AgentConfig]:
        if not name:
            return None
        return self.agents.get(str(name).strip().lower())


def _expand(path: str) -> str:
    return os.path.abspath(os.path.expanduser(os.path.expandvars(path)))


def _secret(table: Dict[str, Any], key: str, where: str, required: bool) -> Optional[str]:
    value = table.get(key)
    file_key = key + "_file"
    if not value and table.get(file_key):
        path = _expand(str(table[file_key]))
        try:
            with open(path, "r", encoding="utf-8") as fh:
                value = fh.read().strip()
        except OSError as exc:
            raise ConfigError(f"{where}.{file_key}: cannot read {path}: {exc}") from exc
    if value is not None:
        value = str(value).strip()
    if required and not value:
        raise ConfigError(f"{where}.{key} (or {file_key}) is required")
    if value and value.upper().startswith(PLACEHOLDER_PREFIXES):
        raise ConfigError(f"{where}.{key} still holds a placeholder value")
    return value or None


def _octal(value: Any, default: int) -> int:
    if value is None:
        return default
    if isinstance(value, int):
        return value
    return int(str(value), 8)


def check_permissions(path: str) -> None:
    try:
        mode = stat.S_IMODE(os.stat(path).st_mode)
    except OSError:
        return
    if mode & 0o077:
        log.warning("config file is readable by group/others; run chmod 600",
                    extra={"fields": {"path": path, "mode": oct(mode)}})


def _command(value: Any) -> List[str]:
    """`"podman exec -i x python"` or `["podman", "exec", ...]`."""
    if not value:
        return []
    return [str(v) for v in value] if isinstance(value, list) else shlex.split(str(value))


def parse_config(data: Dict[str, Any], source_path: Optional[str] = None) -> Config:
    bridge_key = _secret(data, "bridge_key", "bridge", required=True)
    assert bridge_key is not None
    if len(bridge_key) < 32:
        log.warning("bridge_key is shorter than 32 characters")
    data_dir = _expand(str(data.get("data_dir", DEFAULT_DATA_DIR)))

    kyutai = data.get("kyutai", {}) or {}
    pocket = data.get("pocket", {}) or {}
    cron = data.get("cron", {}) or {}
    ntfy = data.get("ntfy", {}) or {}
    push = data.get("push", {}) or {}
    apns_t = data.get("apns", {}) or {}
    outbox_t = data.get("outbox", {}) or {}
    # `[kyutai] enabled = false` (or `url = ""`) runs on Pocket alone; the default voice is then a Bip's.
    kyutai_url = str(kyutai.get("url", "http://127.0.0.1:8097")).rstrip("/") if kyutai.get("enabled", True) else ""
    pocket_url = str(pocket["url"]).rstrip("/") if pocket.get("url") else None
    if not kyutai_url and not pocket_url:
        raise ConfigError("no TTS engine: set [pocket] url, or [kyutai] url")
    default_voice = str(kyutai.get("default_voice", "5476")) if kyutai_url \
        else str(pocket.get("default_voice", "pocket:colibri"))
    limits_t = data.get("limits", {}) or {}

    apns = ApnsConfig(
        team_id=str(apns_t.get("team_id", "") or ""),
        key_id=str(apns_t.get("key_id", "") or ""),
        p8_path=_expand(str(apns_t["p8_path"])) if apns_t.get("p8_path") else "",
        bundle_id=str(apns_t.get("bundle_id", "io.github.bipagents")),
        environment=str(apns_t.get("environment", "production")),
    )
    if apns.environment not in ("sandbox", "production"):
        raise ConfigError("apns.environment must be 'sandbox' or 'production'")

    outbox = OutboxConfig(
        db_path=_expand(str(outbox_t.get("db_path", os.path.join(data_dir, "bridge.db")))),
        audio_dir=_expand(str(outbox_t.get("audio_dir", os.path.join(data_dir, "audio")))),
        retention_days=int(outbox_t.get("retention_days", 90)),
        audio_max_chars=int(outbox_t.get("audio_max_chars", 2000)),
        audio_wait_seconds=float(outbox_t.get("audio_wait_seconds", 8.0)),
    )

    limits = Limits(
        upload_max_bytes=int(float(limits_t.get("upload_max_mb", 50)) * 1024 * 1024),
        upload_retention_days=int(limits_t.get("upload_retention_days", 30)),
        upload_file_mode=_octal(limits_t.get("upload_file_mode"), 0o640),
        upload_dir_mode=_octal(limits_t.get("upload_dir_mode"), 0o750),
        sentence_max_chars=int(limits_t.get("sentence_max_chars", 180)),
        tts_max_chars=int(limits_t.get("tts_max_chars", 1000)),
        tts_cache_entries=int(limits_t.get("tts_cache_entries", 256)),
        watch_max_seconds=int(float(limits_t.get("watch_max_minutes", 120)) * 60),
        purge_interval_seconds=int(float(limits_t.get("purge_interval_hours", 6)) * 3600),
    )

    agents: Dict[str, AgentConfig] = {}
    for raw_name, table in (data.get("agents", {}) or {}).items():
        name = str(raw_name).strip().lower()
        where = f"agents.{name}"
        if not isinstance(table, dict):
            raise ConfigError(f"{where} must be a table")
        hermes_url = str(table.get("hermes_url", "")).rstrip("/")
        if not hermes_url:
            raise ConfigError(f"{where}.hermes_url is required")
        up_host = table.get("upload_dir_host")
        up_cont = table.get("upload_dir_container")
        if bool(up_host) != bool(up_cont):
            raise ConfigError(f"{where}: upload_dir_host and upload_dir_container go together")
        agents[name] = AgentConfig(
            name=name,
            display_name=str(table.get("display_name") or name.capitalize()),
            hermes_url=hermes_url,
            hermes_key=_secret(table, "hermes_key", where, required=True) or "",
            ntfy_topic=str(table["ntfy_topic"]) if table.get("ntfy_topic") else None,
            ntfy_token=_secret(table, "ntfy_token", where, required=False),
            upload_dir_host=_expand(str(up_host)) if up_host else None,
            upload_dir_container=str(up_cont).rstrip("/") if up_cont else None,
            voice=str(table["voice"]) if table.get("voice") else None,
            public_url=str(table["public_url"]).rstrip("/") if table.get("public_url") else None,
            isolated=bool(table.get("isolated", False)),
            python=_command(table.get("python")),
            hermes_home=str(table["hermes_home"]).rstrip("/") if table.get("hermes_home") else None,
            hermes_root=str(table["hermes_root"]).rstrip("/") if table.get("hermes_root") else None,
            space=_expand(str(table["space"])) if table.get("space") else None,
            sandboxed=bool(table.get("sandboxed", False)),
            desktop_url=str(table["desktop_url"]) if table.get("desktop_url") else None,
            desktop_vnc=str(table["desktop_vnc"]) if table.get("desktop_vnc") else None,
            desktop_vnc_password_file=_expand(str(table["desktop_vnc_password_file"]))
            if table.get("desktop_vnc_password_file") else None,
            shares=str(table.get("shares", "")),
            shares_apply=str(table["shares_apply"]) if table.get("shares_apply") else None,
            host_home=_expand(str(table["host_home"])) if table.get("host_home") else None,
        )

    relay_t = data.get("relay", {}) or {}
    clients: Dict[str, str] = {}
    for name, key in (relay_t.get("clients", {}) or {}).items():  # [relay.clients] chi = "key"
        if str(key).strip():
            clients[str(key).strip()] = str(name)
    if relay_t.get("clients_file"):  # or one « name key » per line
        path = _expand(str(relay_t["clients_file"]))
        try:
            with open(path, "r", encoding="utf-8") as fh:
                for line in fh:
                    parts = line.split()
                    if len(parts) == 2 and not line.lstrip().startswith("#"):
                        clients[parts[1]] = parts[0]
        except OSError as exc:
            raise ConfigError(f"relay.clients_file: cannot read {path}: {exc}") from exc
    if any(len(key) < 32 for key in clients):
        raise ConfigError("relay: client keys must be at least 32 characters (python -m bipbridge genkey)")
    relay = RelayConfig(clients=clients, per_hour=int(relay_t.get("per_hour", 600)))
    push_relay_url = str(push["relay_url"]).rstrip("/") if push.get("relay_url") else None
    push_relay_key = _secret(push, "relay_key", "push", required=bool(push_relay_url))
    model_t = data.get("model", {}) or {}
    hermes_t = data.get("hermes", {}) or {}
    hermes_install = HermesInstall(
        bin=_expand(str(hermes_t["bin"])) if hermes_t.get("bin") else "",
        home=_expand(str(hermes_t["home"])) if hermes_t.get("home") else "",
        hub_url=str(hermes_t.get("hub_url", "")).rstrip("/"),
        public_hub_url=str(hermes_t.get("public_hub_url", "")).rstrip("/"),
        container_home=str(hermes_t.get("container_home", "")).rstrip("/"),
        python=_command(hermes_t.get("python")),
        root=_expand(str(hermes_t["root"])) if hermes_t.get("root") else "",
        isolated=bool(hermes_t.get("isolated", False)),
    )
    if hermes_install.home and not hermes_install.python:
        # Hermes' official install: its own Python under <home>/tools, its code in <home>/hermes-agent.
        found = sorted(glob.glob(os.path.join(hermes_install.home, "tools", "python-*", "bin", "python3")))
        code = os.path.join(hermes_install.home, "hermes-agent")
        if found and os.path.isdir(code):
            hermes_install.python = [found[-1], "-I"]
            hermes_install.root = hermes_install.root or code
    spaces_t = data.get("spaces", {}) or {}
    spaces = SpacesConfig(
        root=_expand(str(spaces_t.get("root", os.path.join(data_dir, "spaces")))),
        first_port=int(spaces_t.get("first_port", 8660)),
        sandbox=str(spaces_t.get("sandbox", "auto")),
        start=str(spaces_t["start"]) if spaces_t.get("start") else None,
        stop=str(spaces_t["stop"]) if spaces_t.get("stop") else None,
        container_home=str(spaces_t.get("container_home", "")).rstrip("/"),
    )
    if spaces.sandbox not in ("auto", "none"):
        raise ConfigError("spaces.sandbox must be 'auto' or 'none'")
    browser_t = data.get("browser", {}) or {}
    browser = BrowserConfig(
        enabled=bool(browser_t.get("enabled", False)),
        runtime=str(browser_t.get("runtime", "")),
        image=str(browser_t.get("image", "bipagents-browser")),
        first_port=int(browser_t.get("first_port", 9600)),
        root=_expand(str(browser_t.get("root", os.path.join(data_dir, "browsers")))),
        gpu=str(browser_t.get("gpu", "auto")),
        language=str(browser_t.get("language", "")),
        timezone=str(browser_t.get("timezone", "")),
    )
    if browser.runtime not in ("", "podman", "docker") or browser.gpu not in ("auto", "none"):
        raise ConfigError("browser.runtime must be 'podman' or 'docker', browser.gpu 'auto' or 'none'")

    lan_t = data.get("lan", {}) or {}
    lan = LanConfig(
        enabled=bool(lan_t.get("enabled", False)),
        host=str(lan_t.get("host", "0.0.0.0")),
        port=int(lan_t.get("port", 8650)),
        address=str(lan_t["address"]).strip() if lan_t.get("address") else None,
        cert_dir=_expand(str(lan_t.get("cert_dir", os.path.join(data_dir, "lan")))),
    )

    return Config(
        bridge_key=bridge_key,
        host=str(data.get("host", "127.0.0.1")),
        port=int(data.get("port", 8643)),
        log_level=str(data.get("log_level", "info")).lower(),
        kyutai_url=kyutai_url or None,
        kyutai_timeout=float(kyutai.get("timeout_seconds", 120)),
        default_voice=default_voice,
        pocket_url=pocket_url,
        cron_watch=bool(cron.get("watch", True)),
        cron_interval_seconds=float(cron.get("interval_seconds", 30)),
        cron_window_hours=float(cron.get("window_hours", 12)),
        media_roots={str(k): _expand(str(v)) for k, v in ((data.get("media", {}) or {}).get("roots", {}) or {}).items()},
        ntfy_url=str(ntfy.get("url", "http://127.0.0.1:8645")).rstrip("/"),
        push_previews=bool(push.get("previews", False)),
        watch_followed_runs=bool(push.get("watch_followed_runs", True)),
        apns=apns,
        outbox=outbox,
        limits=limits,
        agents=agents,
        lan=lan,
        relay=relay,
        hermes=hermes_install,
        spaces=spaces,
        browser=browser,
        push_relay_url=push_relay_url,
        push_relay_key=push_relay_key,
        model_health_url=str(model_t["health_url"]) if model_t.get("health_url") else None,
        model_health_key=_secret(model_t, "health_key", "model", required=False),
        public_url=str(data["public_url"]).rstrip("/") if data.get("public_url") else None,
        peer_url=str(data["peer_url"]).rstrip("/") if data.get("peer_url") else None,
        source_path=source_path,
    )


def config_path() -> str:
    return _expand(os.environ.get("BRIDGE_CONFIG") or DEFAULT_CONFIG_PATH)


def load_config(path: Optional[str] = None) -> Config:
    path = _expand(path) if path else config_path()
    try:
        with open(path, "rb") as fh:
            data = tomllib.load(fh)
    except FileNotFoundError as exc:
        raise ConfigError(f"config file not found: {path}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise ConfigError(f"invalid TOML in {path}: {exc}") from exc
    check_permissions(path)
    return parse_config(data, source_path=path)
