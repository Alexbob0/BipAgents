"""The bridge under test, as a separate process: its config written as TOML in a folder of its own, started with
`BRIDGE_CMD` (default: the Rust bridge's release build, `bridge-rs/target/release/bipbridge serve`, or the one in
`$CARGO_TARGET_DIR/release/`), run from bridge/, waited for on `/health`, talked to over HTTP only, stopped with SIGTERM.

Its environment is small and fixed: `HOME` is the test's own folder (nothing of this machine's), `PATH` starts with
the fake executables, `BRIDGE_CONFIG` names the config, `TZ=UTC`."""
from __future__ import annotations

import json
import os
import shlex
import signal
import sqlite3
import subprocess
import time
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

import httpx

from . import toml_out
from .common import BRIDGE_KEY, HERMES_KEY, NTFY_TOKEN
from .fakebin import Commands
from .servers import free_port

BRIDGE_DIR = Path(__file__).resolve().parents[2]


def built_bridge() -> Path:
    """The Rust bridge's release build: `$CARGO_TARGET_DIR/release/bipbridge` when that is set, else bridge-rs/'s."""
    target = os.environ.get("CARGO_TARGET_DIR", "").strip()
    folder = Path(target) if target else BRIDGE_DIR.parent / "bridge-rs" / "target"
    return folder / "release" / "bipbridge"


def bridge_command() -> List[str]:
    """`BRIDGE_CMD` split (it ends with `serve`), or the Rust bridge's release build with `serve`."""
    raw = os.environ.get("BRIDGE_CMD", "").strip()
    if raw:
        return shlex.split(raw)
    built = built_bridge()
    if not built.is_file():
        raise RuntimeError(f"no bridge to test: build it (`cargo build --release` in bridge-rs/, {built}) or set "
                           "BRIDGE_CMD")
    return [str(built), "serve"]


def cli_command() -> List[str]:
    """The same program without `serve`: the CLI's other subcommands go after it."""
    command = bridge_command()
    return command[:-1] if command and command[-1] == "serve" else command


# The outbox database as the Python bridge created it (once bridge/bipbridge/store.py): the bridge must open such a
# file as it is, so tests create some before the bridge starts.
SCHEMA = """
CREATE TABLE IF NOT EXISTS devices (
    token TEXT PRIMARY KEY,
    environment TEXT NOT NULL,
    agent_ids TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS outbox (
    id TEXT PRIMARY KEY,
    agent TEXT NOT NULL,
    ntfy_id TEXT,
    title TEXT,
    text TEXT NOT NULL,
    created_at TEXT NOT NULL,
    sent_at TEXT,
    session_id TEXT,
    audio_path TEXT,
    UNIQUE (agent, ntfy_id)
);
CREATE INDEX IF NOT EXISTS outbox_created_at ON outbox (created_at);
CREATE TABLE IF NOT EXISTS cron_jobs (
    agent TEXT NOT NULL,
    job TEXT NOT NULL,
    name TEXT,
    notify INTEGER NOT NULL DEFAULT 1,
    last_seen TEXT,
    PRIMARY KEY (agent, job)
);
CREATE TABLE IF NOT EXISTS watched_runs (
    agent TEXT NOT NULL,
    run_id TEXT NOT NULL,
    session_id TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (agent, run_id)
);
CREATE TABLE IF NOT EXISTS ntfy_cursor (
    agent TEXT PRIMARY KEY,
    last_id TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
"""


def default_config(root: Path, fakes: Any, p8_path: str) -> Dict[str, Any]:
    """tests/conftest.py's config_dict, pointed at the fakes."""
    return {
        "bridge_key": BRIDGE_KEY,
        "host": "127.0.0.1",
        "port": free_port(),
        "data_dir": str(root / "data"),
        "kyutai": {"url": fakes.kyutai.url, "default_voice": "5476"},
        "pocket": {"url": fakes.pocket.url},
        "ntfy": {"url": fakes.ntfy.url},
        "apns": {"team_id": "TEAM123456", "key_id": "TESTKEY123", "p8_path": p8_path,
                 "bundle_id": "io.github.bipagents", "environment": "sandbox",
                 "base_url": fakes.apns.url + "/production", "sandbox_url": fakes.apns.url + "/sandbox"},
        "outbox": {"audio_wait_seconds": 2},
        "limits": {"upload_max_mb": 1, "purge_interval_hours": 1000},
        "cron": {"watch": False},
        # The weather and route tools' services: never the real ones.
        "weather": {"met_url": fakes.maps.url + "/weatherapi/locationforecast/2.0/complete",
                    "geocoder_url": fakes.maps.url + "/search"},
        "maps": {"routes_url": fakes.maps.url + "/directions/v2:computeRoutes",
                 "tile_url": fakes.maps.url + "/osm/{z}/{x}/{y}.png", "vector_url": fakes.maps.url + "/vector"},
        # Never isolated by default: `auto` would isolate the sandboxes on Linux with podman (CI); test_sandbox says on.
        "sandbox": {"isolate": "off"},
        "agents": {
            "Wellness": {
                "display_name": "Wellness", "hermes_url": fakes.hermes.url, "hermes_key": HERMES_KEY,
                "ntfy_topic": "hermes-wellness-out", "ntfy_token": NTFY_TOKEN,
                "upload_dir_host": str(root / "uploads" / "wellness"),
                "upload_dir_container": "/home/hermes/.hermes/profiles/wellness/uploads",
                "voice": "4193",
            },
        },
    }


class BridgeFailed(RuntimeError):
    pass


class Bridge:
    """One bridge process and its folder: `config` (a dict, written as TOML at start), `data` (its data dir),
    `home` (its HOME), `commands` (what its fake executables were run with)."""

    def __init__(self, root: Path, config: Dict[str, Any], fakebin: Path, name: str = "bridge"):
        self.root = root
        self.name = name
        self.config = config
        self.fakebin = fakebin
        self.home = root / "home"
        self.home.mkdir(parents=True, exist_ok=True)
        self.config_path = root / "bridge.toml"
        self.log_path = root / f"{name}.log"
        self.commands = Commands(root / "commands.jsonl", root / "rules.json")
        self.process: Optional[subprocess.Popen] = None
        self._http: Optional[httpx.Client] = None
        self.extra_toml = ""     # appended to the written config as it is (comments, odd layouts)
        self.env_extra: Dict[str, str] = {}
        self.command: List[str] = bridge_command()   # another implementation's for the switch test (test_switch.py)

    # -- where things are ------------------------------------------------------------------------------------------
    @property
    def port(self) -> int:
        return int(self.config["port"])

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    @property
    def data(self) -> Path:
        return Path(self.config.get("data_dir") or self.root / "data")

    @property
    def db_path(self) -> Path:
        custom = (self.config.get("outbox") or {}).get("db_path")
        return Path(custom) if custom else self.data / "bridge.db"

    @property
    def state_dir(self) -> Path:
        """Where the bridge keeps its files next to its database (console.json, peers.json, vault/…)."""
        return self.db_path.parent

    def env(self) -> Dict[str, str]:
        env = {
            "PATH": f"{self.fakebin}:/usr/bin:/bin:/usr/sbin:/sbin",
            "HOME": str(self.home),
            "BRIDGE_CONFIG": str(self.config_path),
            "BB_COMMANDS": str(self.commands.log),
            "BB_RULES": str(self.commands.rules_path),
            "TZ": "UTC",
            "LANG": "en_US.UTF-8",
            "PYTHONUNBUFFERED": "1",
            "PYTHONDONTWRITEBYTECODE": "1",
            "NO_PROXY": "*",
        }
        env.update(self.env_extra)
        return env

    def write_config(self) -> None:
        text = toml_out.dumps(self.config)
        if self.extra_toml:
            text += "\n" + self.extra_toml
        self.config_path.write_text(text, encoding="utf-8")
        os.chmod(self.config_path, 0o600)

    # -- life ----------------------------------------------------------------------------------------------------
    def start(self, wait: bool = True, write: bool = True) -> "Bridge":
        if write:
            self.write_config()
        log = open(self.log_path, "ab")
        self._log_start = log.tell()
        self.process = subprocess.Popen(self.command, cwd=str(BRIDGE_DIR), env=self.env(), stdin=subprocess.DEVNULL,
                                        stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        log.close()
        if wait:
            self.wait_ready()
        return self

    def wait_ready(self, timeout: float = 30.0) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.process is not None and self.process.poll() is not None:
                raise BridgeFailed(f"{self.name} exited with {self.process.returncode}:\n{self.log_tail()}")
            try:
                if httpx.get(f"{self.url}/health", timeout=2.0).status_code == 200:
                    return
            except httpx.HTTPError:
                pass
            time.sleep(0.05)
        raise BridgeFailed(f"{self.name} not ready after {timeout} s:\n{self.log_tail()}")

    def stop(self) -> Optional[int]:
        if self._http is not None:
            self._http.close()
            self._http = None
        if self.process is None or self.process.poll() is not None:
            return self.process.returncode if self.process else None
        self.process.send_signal(signal.SIGTERM)
        try:
            self.process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            os.killpg(self.process.pid, signal.SIGKILL)
            self.process.wait(timeout=5)
        return self.process.returncode

    def restart(self, write: bool = True) -> "Bridge":
        self.stop()
        return self.start(write=write)

    @property
    def running(self) -> bool:
        return self.process is not None and self.process.poll() is None

    # -- what it said ----------------------------------------------------------------------------------------------
    def log_tail(self, lines: int = 60) -> str:
        try:
            return "\n".join(self.log_path.read_text(encoding="utf-8", errors="replace").splitlines()[-lines:])
        except OSError:
            return ""

    def logs(self, every_run: bool = False) -> List[Dict[str, Any]]:
        """Its JSON log lines since it last started (`every_run`: since the first start); anything else it printed is
        left out."""
        out = []
        try:
            raw = self.log_path.read_bytes()
        except OSError:
            return out
        text = (raw if every_run else raw[getattr(self, "_log_start", 0):]).decode("utf-8", errors="replace")
        for line in text.splitlines():
            line = line.strip()
            if line.startswith("{"):
                try:
                    out.append(json.loads(line))
                except ValueError:
                    continue
        return out

    def logged(self, msg: str) -> List[Dict[str, Any]]:
        return [entry for entry in self.logs() if entry.get("msg") == msg]

    # -- the CLI ---------------------------------------------------------------------------------------------------
    def cli(self, *args: str, timeout: float = 60.0) -> subprocess.CompletedProcess:
        """Another subcommand of the same program (`console`, `qr`, `check`…), with this bridge's environment (its
        config as last written: `write_config()` first for a bridge never started)."""
        base = self.command[:-1] if self.command and self.command[-1] == "serve" else self.command
        return subprocess.run(base + list(args), cwd=str(BRIDGE_DIR), env=self.env(),
                              stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=timeout)

    # -- HTTP ------------------------------------------------------------------------------------------------------
    @property
    def http(self) -> httpx.Client:
        if self._http is None:
            self._http = httpx.Client(base_url=self.url, timeout=60.0, follow_redirects=False)
        return self._http

    def request(self, method: str, path: str, **kw: Any) -> httpx.Response:
        return self.http.request(method, path, **kw)

    def get(self, path: str, **kw: Any) -> httpx.Response:
        return self.http.get(path, **kw)

    def post(self, path: str, **kw: Any) -> httpx.Response:
        return self.http.post(path, **kw)

    def put(self, path: str, **kw: Any) -> httpx.Response:
        return self.http.put(path, **kw)

    def patch(self, path: str, **kw: Any) -> httpx.Response:
        return self.http.patch(path, **kw)

    def delete(self, path: str, **kw: Any) -> httpx.Response:
        return self.http.delete(path, **kw)

    def stream(self, method: str, path: str, **kw: Any):
        return self.http.stream(method, path, **kw)

    def ws_url(self, path: str) -> str:
        return f"ws://127.0.0.1:{self.port}{path}"

    # -- its files -------------------------------------------------------------------------------------------------
    def db(self) -> sqlite3.Connection:
        return sqlite3.connect(str(self.db_path))

    def seed_db(self, statements: List[tuple]) -> None:
        """Rows in its database before it starts (created with the Python bridge's schema)."""
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        with sqlite3.connect(str(self.db_path)) as db:
            db.executescript(SCHEMA)
            for sql, params in statements:
                db.execute(sql, params)

    def toml_text(self) -> str:
        return self.config_path.read_text(encoding="utf-8")


class Bridges:
    """Makes the bridges of one test (most need one; households need two) and stops them at its end."""

    def __init__(self, tmp_path: Path, fakes: Any, fakebin: Path, p8_path: str):
        self.tmp_path, self.fakes, self.fakebin, self.p8_path = tmp_path, fakes, fakebin, p8_path
        self.made: List[Bridge] = []

    def new(self, name: str = "bridge", change: Optional[Callable[[Dict[str, Any]], Any]] = None) -> Bridge:
        root = self.tmp_path / name
        root.mkdir(parents=True, exist_ok=True)
        config = default_config(root, self.fakes, self.p8_path)
        if change is not None:
            change(config)
        bridge = Bridge(root, config, self.fakebin, name)
        self.made.append(bridge)
        return bridge

    def stop(self) -> None:
        for bridge in self.made:
            try:
                bridge.stop()
            except Exception:  # noqa: BLE001 - teardown goes on
                pass
