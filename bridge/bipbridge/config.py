"""Bridge configuration (TOML).

Default path: ``~/.config/hermes-ios/bridge.toml``, overridable with ``BRIDGE_CONFIG``.
Secrets may be given inline (``bridge_key``, ``hermes_key``, ``ntfy_token``) or through a file
(``bridge_key_file``, ``hermes_key_file``, ``ntfy_token_file``), e.g. a systemd credential.
"""
from __future__ import annotations

import logging
import os
import stat
from dataclasses import dataclass, field
from typing import Any, Dict, Optional

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
