"""Fake executables put first on the bridge's PATH (and given as `[hermes] bin`): `hermes`, `podman`, `docker`,
`systemctl`, `systemd-run`, `launchctl`, `ffmpeg`. Each one appends what it was run with to a JSON-lines file
(`argv`, `HERMES_HOME`, its stdin) and answers like the real one would for what the bridge asks:

- `hermes profile create <name>` makes the profile's folder (the hub's config.yaml copied in), `profile delete`
  removes it and leaves Hermes' tombstone, `config set <key> <value>` writes the profile's config.yaml (dotted keys)
  or its .env (upper-case keys); anything else succeeds silently.
- `ffmpeg … -i <in> … <out>` copies its input to its output.
- the others succeed silently.

Rules (a JSON file, read at each run) change that for one test: `{"cmd": "hermes", "prefix": [...], "exit": 1,
"stderr": "...", "stdout": "...", "times": 1}` — the first rule whose command and argument prefix match wins."""
from __future__ import annotations

import json
import stat
import sys
from pathlib import Path
from typing import Any, Dict, List

COMMANDS = ("hermes", "podman", "docker", "systemctl", "systemd-run", "launchctl", "ffmpeg", "fake-gateway")

# Each command is a shell script running the same Python file with its own name first (a shebang line cannot name an
# interpreter whose path has a space, as a virtualenv's may).
WRAPPER = """#!/bin/sh
exec "__PYTHON__" -I "__FAKE__" "$(basename "$0")" "$@"
"""

SCRIPT = r'''import fcntl, json, os, shutil, sys, time
from pathlib import Path

NAME = sys.argv[1]
ARGS = sys.argv[2:]
LOG = os.environ.get("BB_COMMANDS")
RULES = os.environ.get("BB_RULES")

# A connector's container (connectors_run.rs) talks on its stdin: never read here, it goes to the program run.
INTERACTIVE = NAME == "podman" and ARGS[:1] == ["run"] and "bipagents.role=connector" in ARGS
try:
    stdin = "" if INTERACTIVE or sys.stdin.isatty() else sys.stdin.read()
except Exception:
    stdin = ""

def exec_container(image):
    """`podman run … <image> <command…>` played on this machine: the command runs in the `-w` folder (mapped through
    the `-v` mounts), with only the variables `-e` names (`NAME=value`, or `NAME` taken from this environment) and
    PATH; paths under a mount point are mapped too. `python3` is this Python."""
    at = ARGS.index(image)
    options, command = ARGS[:at], ARGS[at + 1:]
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin")}
    mounts, workdir, i = {}, None, 0
    while i < len(options):
        if options[i] == "-e" and i + 1 < len(options):
            key, eq, value = options[i + 1].partition("=")
            env[key] = value if eq else os.environ.get(key, "")
            i += 2
        elif options[i] == "-v" and i + 1 < len(options):
            host, target = options[i + 1].split(":")[:2]
            mounts[target] = host
            i += 2
        elif options[i] == "-w" and i + 1 < len(options):
            workdir = options[i + 1]
            i += 2
        else:
            i += 1
    def mapped(path):
        for target, host in mounts.items():
            if path == target or path.startswith(target + "/"):
                return host + path[len(target):]
        return path
    env = {k: mapped(v) for k, v in env.items()}
    if workdir:
        os.chdir(mapped(workdir))
    if command and command[0] == "python3":
        command[0] = sys.executable
    os.execvpe(command[0], command, env)

def record(extra=None):
    if not LOG:
        return
    entry = {"cmd": NAME, "argv": ARGS, "hermes_home": os.environ.get("HERMES_HOME"), "stdin": stdin,
             "at": time.time()}
    entry.update(extra or {})
    with open(LOG, "a", encoding="utf-8") as fh:
        fcntl.flock(fh, fcntl.LOCK_EX)
        fh.write(json.dumps(entry) + "\n")

def rule():
    if not RULES or not os.path.exists(RULES):
        return None
    with open(RULES, "r+", encoding="utf-8") as fh:
        fcntl.flock(fh, fcntl.LOCK_EX)
        rules = json.load(fh)
        for item in rules:
            prefix = item.get("prefix") or []
            if item.get("cmd") == NAME and ARGS[:len(prefix)] == prefix and item.get("times", 1) != 0:
                if "times" in item:
                    item["times"] -= 1
                    fh.seek(0)
                    fh.truncate()
                    json.dump(rules, fh)
                return item
    return None

def scalar(value):
    if value[:1] in ("[", "{"):
        try:
            return json.loads(value)
        except ValueError:
            return value
    if value in ("true", "false"):
        return value == "true"
    try:
        return int(value)
    except ValueError:
        return value

def hermes():
    home = Path(os.environ.get("HERMES_HOME") or Path.home() / ".hermes")
    args = list(ARGS)
    profile = None
    if args[:1] == ["-p"] and len(args) > 1:
        profile, args = args[1], args[2:]
    if args[:2] == ["profile", "create"] and len(args) > 2:
        folder = home / "profiles" / args[2]
        folder.mkdir(parents=True, exist_ok=True)
        hub = home / "config.yaml"
        if hub.exists() and not (folder / "config.yaml").exists():
            shutil.copyfile(hub, folder / "config.yaml")
        return
    if args[:2] == ["profile", "delete"] and len(args) > 2:
        shutil.rmtree(home / "profiles" / args[2], ignore_errors=True)
        (home / "profiles" / ".deleted").mkdir(parents=True, exist_ok=True)
        (home / "profiles" / ".deleted" / args[2]).write_text("deleted")
        return
    if args[:2] == ["config", "set"] and len(args) > 3:
        folder = home / "profiles" / profile if profile else home
        folder.mkdir(parents=True, exist_ok=True)
        key, value = args[2], args[3]
        if key.replace("_", "").isalnum() and key.upper() == key:
            env = folder / ".env"
            lines = [l for l in (env.read_text().splitlines() if env.exists() else []) if l.split("=", 1)[0] != key]
            env.write_text("\n".join(lines + [f"{key}={value}"]) + "\n")
            return
        import yaml
        path = folder / "config.yaml"
        # One writer at a time (the bridge runs several `config set` at once at start: a lost write otherwise).
        lock = open(Path(LOG).with_name("config-set.lock") if LOG else folder / ".config-set.lock", "w")
        fcntl.flock(lock, fcntl.LOCK_EX)
        data = yaml.safe_load(path.read_text()) if path.exists() else None
        data = data if isinstance(data, dict) else {}
        node = data
        parts = key.split(".")
        for part in parts[:-1]:
            if not isinstance(node.get(part), dict):
                node[part] = {}
            node = node[part]
        node[parts[-1]] = scalar(value)
        path.write_text(yaml.safe_dump(data, allow_unicode=True, sort_keys=False))

def ffmpeg():
    if "-i" in ARGS and ARGS:
        source = ARGS[ARGS.index("-i") + 1]
        shutil.copyfile(source, ARGS[-1])

def gateway():
    """`fake-gateway <HERMES_HOME>`: a space's gateway (`[spaces] start`), on the port its .env gives, for a minute."""
    env = dict(l.split("=", 1) for l in Path(ARGS[0], ".env").read_text().splitlines() if "=" in l)
    port = int(env["API_SERVER_PORT"])
    if os.fork():
        return
    os.setsid()
    if os.fork():
        os._exit(0)
    devnull = os.open(os.devnull, os.O_RDWR)
    for fd in (0, 1, 2):   # the bridge waits for its command's output to end: the server keeps none of it
        os.dup2(devnull, fd)
    import http.server, threading
    class Handler(http.server.BaseHTTPRequestHandler):
        def answer(self, status=200):
            length = int(self.headers.get("content-length") or 0)
            if length:
                self.rfile.read(length)
            body = json.dumps({"runs": True, "object": "hermes.session", "session": {"id": "space-1"}}).encode()
            self.send_response(status)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def do_GET(self):
            self.answer()
        def do_POST(self):
            self.answer(201 if self.path.endswith("/api/sessions") else 200)
        def log_message(self, *args):
            pass
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
    threading.Timer(60, server.shutdown).start()
    server.serve_forever()
    os._exit(0)

found = rule()
record({"rule": bool(found)})
if found is not None and found.get("exec_image") and found["exec_image"] in ARGS:
    exec_container(found["exec_image"])
if found is not None:
    if found.get("stdout"):
        sys.stdout.write(found["stdout"])
    if found.get("stdout_file"):
        sys.stdout.flush()
        with open(found["stdout_file"], "rb") as fh:
            sys.stdout.buffer.write(fh.read())
    if found.get("stderr"):
        sys.stderr.write(found["stderr"])
    sys.exit(int(found.get("exit", 0)))
if NAME == "hermes":
    hermes()
elif NAME == "ffmpeg":
    ffmpeg()
elif NAME == "fake-gateway":
    gateway()
sys.exit(0)
'''


def install(folder: Path) -> Path:
    """The fake executables in `folder` (made once per test session)."""
    folder.mkdir(parents=True, exist_ok=True)
    fake = folder / "fake_command.py"
    fake.write_text(SCRIPT, encoding="utf-8")
    text = WRAPPER.replace("__PYTHON__", sys.executable).replace("__FAKE__", str(fake))
    for name in COMMANDS:
        path = folder / name
        path.write_text(text, encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return folder


class Commands:
    """What the fake executables of one bridge were run with, and the rules they follow."""

    def __init__(self, log: Path, rules: Path):
        self.log, self.rules_path = log, rules
        self.rules_path.write_text("[]", encoding="utf-8")

    def all(self) -> List[Dict[str, Any]]:
        if not self.log.exists():
            return []
        return [json.loads(line) for line in self.log.read_text(encoding="utf-8").splitlines() if line.strip()]

    def of(self, cmd: str) -> List[List[str]]:
        return [entry["argv"] for entry in self.all() if entry["cmd"] == cmd]

    def entries(self, cmd: str) -> List[Dict[str, Any]]:
        return [entry for entry in self.all() if entry["cmd"] == cmd]

    def clear(self) -> None:
        if self.log.exists():
            self.log.unlink()

    def rule(self, cmd: str, prefix: List[str], exit: int = 0, stdout: str = "", stderr: str = "",
             times: int = -1, stdout_file: str = "", exec_image: str = "") -> None:
        rules = json.loads(self.rules_path.read_text(encoding="utf-8"))
        item: Dict[str, Any] = {"cmd": cmd, "prefix": prefix, "exit": exit, "stdout": stdout, "stderr": stderr}
        if stdout_file:
            item["stdout_file"] = stdout_file   # bytes (a tar stream for `podman cp … -`)
        if exec_image:
            item["exec_image"] = exec_image     # `podman run … <image> <command>`: the command run here
            item["prefix"] = prefix
        if times >= 0:
            item["times"] = times
        rules.append(item)
        self.rules_path.write_text(json.dumps(rules), encoding="utf-8")

    def clear_rules(self) -> None:
        self.rules_path.write_text("[]", encoding="utf-8")
