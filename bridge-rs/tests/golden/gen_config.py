"""Golden fixtures for config.parse_config: TOML documents (as dicts) -> the Config (dataclasses.asdict) or the
ConfigError message. Run from bridge/ with the venv:

    cd bridge && ../path/to/venv/bin/python ../bridge-rs/tests/golden/gen_config.py

The account's home folder is written as $HOME (the Rust test puts its own back)."""
import dataclasses
import json
import os
import sys

sys.path.insert(0, os.getcwd())
from bipbridge.config import ConfigError, parse_config  # noqa: E402

KEY = "k" * 40
BASE = {"bridge_key": KEY}
CASES = [
    BASE,
    {**BASE, "port": "9000", "host": "0.0.0.0", "log_level": "DEBUG", "data_dir": "/srv/bip/data"},
    {**BASE, "kyutai": {"enabled": False}, "pocket": {"url": "http://127.0.0.1:8098/"}},
    {**BASE, "kyutai": {"url": ""}, "pocket": {"url": "http://p", "default_voice": "pocket:ours"}},
    {**BASE, "kyutai": {"enabled": False}},
    {**BASE, "pocket": {"url": "http://p"}, "drama": {"url": "http://127.0.0.1:8099/"}},
    {**BASE, "pocket": {"url": "http://p"}, "drama": {"url": "http://127.0.0.1:8099", "enabled": False}},
    {**BASE, "voice_server": {"url": "http://gpu:8880/", "model": "kokoro", "live": False, "voices": ["af_bella", 3],
                              "stream_rate": "24000", "first_audio_seconds": 2}},
    {**BASE, "voice_server": {"url": "http://gpu:8880", "enabled": False}},
    {**BASE, "kyutai": {"url": "http://gpu:8097/", "default_voice": 7, "timeout_seconds": "30"},
     "cron": {"watch": 0, "interval_seconds": 5, "window_hours": "1.5"},
     "ntfy": {"url": "http://ntfy/"}, "push": {"previews": "yes", "watch_followed_runs": False,
                                              "relay_url": "http://relay/", "relay_key": "rk" * 20,
                                              "service_url": "https://push.example/"},
     "model": {"health_url": "http://model/v1/models", "health_key": "mk"},
     "public_url": "https://box.ts.net/", "peer_url": "http://host.containers.internal:8643/peer/", "cloud": 1,
     "setup": {"file": "/srv/setup.json", "progress": "/srv/progress.json"}},
    {**BASE, "apns": {"team_id": "T", "key_id": "K", "p8_path": "/keys/a.p8", "environment": "sandbox",
                      "base_url": "http://127.0.0.1:1/", "sandbox_url": ""},
     "fcm": {"service_account": "/keys/fcm.json", "base_url": "http://127.0.0.1:2/", "token_url": "http://127.0.0.1:3/t"}},
    {**BASE, "apns": {"environment": "dev"}},
    {**BASE, "outbox": {"db_path": "/srv/db/bridge.db", "audio_dir": "/srv/audio", "retention_days": "30",
                        "audio_max_chars": 100, "audio_wait_seconds": 2}},
    {**BASE, "limits": {"upload_max_mb": 1.5, "upload_retention_days": 7, "upload_file_mode": "0644",
                        "upload_dir_mode": 0o700, "sentence_max_chars": "200", "tts_max_chars": 500,
                        "tts_cache_entries": 0, "watch_max_minutes": 30, "purge_interval_hours": 0.5}},
    {**BASE, "agents": {
        "Vie": {"hermes_url": "http://127.0.0.1:8644/", "hermes_key": " key-vie ", "ntfy_topic": "t",
                "ntfy_token": "tk", "upload_dir_host": "/srv/up/vie/", "upload_dir_container": "/home/h/up/",
                "voice": "pocket:colibri", "server_voice": "server:fr/x", "category": "wellness", "color": "#FF0000",
                "public_url": "https://box:8644/", "isolated": True, "python": "podman exec -i hermes-vie python",
                "hermes_home": "/home/hermes/.hermes/", "hermes_root": "/opt/hermes/", "space": "/srv/spaces/vie",
                "sandboxed": 1, "desktop_url": "https://box/vnc", "desktop_vnc": "tcp://127.0.0.1:5900",
                "desktop_vnc_password_file": "/srv/vnc.pw", "shares": "external", "shares_apply": "systemctl restart x",
                "host_home": "/srv/hermes"},
        " Wellness ": {"display_name": "Bien-être", "hermes_url": "http://w", "hermes_key": "kw",
                       "python": ["/usr/bin/python3", "-I"]},
        "mON-coach": {"hermes_url": "http://c", "hermes_key": "kc", "display_name": ""},
    }},
    {**BASE, "agents": {"a": {"hermes_key": "k"}}},
    {**BASE, "agents": {"a": {"hermes_url": "http://h"}}},
    {**BASE, "agents": {"a": {"hermes_url": "http://h", "hermes_key": "k", "upload_dir_host": "/tmp/x"}}},
    {**BASE, "agents": {"a": "not a table"}},
    {**BASE, "agents": {"a": {"hermes_url": "http://h", "hermes_key": "CHANGE-ME-key"}}},
    {"bridge_key": "CHANGE-ME-run-python-m-bipbridge-genkey"},
    {"bridge_key": "<your key>"},
    {},
    {"bridge_key": "short"},
    {**BASE, "relay": {"clients": {"chi": "c" * 40, "empty": "  "}, "per_hour": "60"}},
    {**BASE, "relay": {"clients": {"chi": "short"}}},
    {**BASE, "hermes": {"bin": "/opt/hermes/bin/hermes", "home": "/srv/no-such-hermes-home", "hub_url": "http://127.0.0.1:8642/",
                        "public_hub_url": "https://box:8642/", "container_home": "/home/hermes/.hermes/",
                        "python": "podman exec -i hermes python", "root": "/opt/hermes", "isolated": True}},
    {**BASE, "spaces": {"root": "/srv/spaces", "first_port": "8700", "sandbox": "none", "start": "x {agent}",
                        "stop": "y {home}", "container_home": "/home/h/"}},
    {**BASE, "spaces": {"sandbox": "always"}},
    {**BASE, "browser": {"enabled": True, "runtime": "podman", "image": "img", "first_port": 9700, "ports": 1,
                         "root": "/srv/browsers", "gpu": "none", "language": "fr-FR", "timezone": "Europe/Paris",
                         "idle_minutes": -3}},
    {**BASE, "browser": {"runtime": "lxc"}},
    {**BASE, "lan": {"enabled": True, "host": "192.168.1.2", "port": "8651", "address": " 192.168.1.177 ",
                     "cert_dir": "/srv/lan"}},
    {**BASE, "media": {"roots": {"/home/hermes/media": "/srv/media", "/x": "/y/"}},
     "pronunciations": {"fr": {"Garmin": "Garmine", "n": 5}, "en": "not a table"}},
]

home = os.path.expanduser("~")


def scrub(value):
    if isinstance(value, str):
        return value.replace(home, "$HOME")
    if isinstance(value, list):
        return [scrub(v) for v in value]
    if isinstance(value, dict):
        return {k: scrub(v) for k, v in value.items()}
    return value


out = []
for case in CASES:
    try:
        result = {"config": scrub(dataclasses.asdict(parse_config(case)))}
    except ConfigError as exc:
        result = {"error": str(exc)}
    out.append({"input": case, **result})

path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "config.json")
with open(path, "w", encoding="utf-8") as fh:
    json.dump(out, fh, ensure_ascii=False, indent=1)
    fh.write("\n")
print(f"{len(out)} cases -> {path}")
