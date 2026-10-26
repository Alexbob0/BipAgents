#!/usr/bin/env python3
"""BipAgents: a planned move of one place to another server of the offer, while its agents keep working (2026-10-26).

Run as root by the seat agent (seat-agent.py) on the guichet's orders: `move_in` on the target server, `move_out` on
the source server, both with the same one-time key (32 random bytes the guichet made for this move). From that key
both servers derive two Ed25519 keys: the target's sshd for this move only (its host key) and the source's (its
client key). The target lets that client key in, from the source's address only, on the move's port, for one
command: this script's `serve`, which allows rsync writing under the target place's home and the move's few steps.
The data goes from server to server inside that SSH channel, never through the guichet or any machine of Kerfy's.

    place-move.py in <order file>      (target) the listener: its sshd, its firewall rule, its keys
    place-move.py out <order file>     (source, a unit of its own: bipagents-move-<id>) the whole move:
        1. copy while running: rsync of the place's folders (installer/bipinstall/placemove.py says which, and what
           to leave out) again and again until what changed between two copies is small
        2. a quiet moment: in the guichet's window (or the next half hour), when the bridge (GET /v1/place/move) has
           no turn running; at the latest five minutes before the window ends
        3. drain: the bridge refuses the apps' writes (503 "moving": the apps keep the person's sends), pauses the
           background tasks at their next step, queues new ones, pauses the routines due in the window
        4. switch: the place stopped, its last changes copied (checksums: nothing missed), the target adjusts the
           settings to its own door and ports (as a restore does), starts it with the same keys, checks its bridge,
           each agent's gateway and the routines, and resumes the tasks and routines (those whose time passed run
           now). Any failure: the target stops its copy, the source starts again as it was.
        5. the source's copy is kept, stopped, seven days, then its account goes (`cleanup`)
    place-move.py serve <id>           (target, the forced command of the move's SSH key)
    place-move.py cancel <id>          either side: the move called off (the source resumes if it had stopped)
    place-move.py close <id>           (target) the listener gone: sshd stopped, firewall rule and keys removed
    place-move.py cleanup              each beat: listeners past their time closed, source copies past 7 days removed
    place-move.py states               what the beat reports: each move's phase and numbers, nothing of anyone

State: /var/lib/bipagents/moves/<id>/ (root only): order.json (with the key), state.json (what the beat says).
ASCII only, like seat-agent.py and user-data.yaml (installer/tests/test_cloud_init.py checks it).
"""
import base64
import hashlib
import hmac
import json
import os
import pwd
import re
import shlex
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

LIB = "/usr/local/lib/bipagents"
MOVES = "/var/lib/bipagents/moves"
MAX_SEATS = 8
KEEP_SECONDS = 7 * 86400
SMALL_DELTA = 32 * 2 ** 20      # bytes changed between two copies: small enough to switch
MAX_ROUNDS = 6                  # copies before the quiet moment, at most
ROUND_EVERY = 120               # seconds between copies while waiting for the window
DRAIN_WAIT = 150                # seconds the running turns and tasks get to finish or pause
AUTO_WINDOW = 1800              # no window from the guichet: the next half hour
LATEST_BEFORE_END = 300         # the switch starts at the latest this long before the window ends
SUB_RANGE = 65536
MOVED_AWAY = ".config/bipagents/moved-away.json"
ID = re.compile(r"^[a-z0-9][a-z0-9-]{5,62}$")
PKCS8_ED25519 = bytes.fromhex("302e020100300506032b657004220420")
STATE_FIELDS = ("id", "role", "seat", "phase", "round", "copied_bytes", "delta_bytes", "files", "started", "updated",
                "drain_seconds", "downtime_seconds", "error", "kept_until", "result", "verified")


def log(text):
    print("%s %s" % (time.strftime("%H:%M:%S"), text), flush=True)


def account(number):
    return "bip" if number == 1 else "bip%d" % number


class MoveError(Exception):
    """Safe to report (no key, nothing of the person)."""


# -- the one-time key and what both sides derive from it ----------------------------------------------------------------

def secret_of(key):
    raw = base64.urlsafe_b64decode(key + "=" * (-len(key) % 4))
    if len(raw) < 32:
        raise MoveError("the move's key is too short")
    return raw


def seed(key, label):
    """32 bytes for one of the move's Ed25519 keys ("host" or "client"), the same on both servers."""
    return hmac.new(secret_of(key), ("bipagents-move/" + label).encode(), hashlib.sha256).digest()


def ed25519_public(private_seed):
    """The public key of an Ed25519 seed (openssl does the curve)."""
    pem = "-----BEGIN PRIVATE KEY-----\n%s\n-----END PRIVATE KEY-----\n" % base64.b64encode(
        PKCS8_ED25519 + private_seed).decode()
    done = subprocess.run(["openssl", "pkey", "-pubout", "-outform", "DER"], input=pem.encode(), capture_output=True)
    if done.returncode != 0 or len(done.stdout) < 32:
        raise MoveError("openssl could not read the move's key")
    return done.stdout[-32:]


def _s(data):
    return struct.pack(">I", len(data)) + data


def openssh_public(public):
    return "ssh-ed25519 " + base64.b64encode(_s(b"ssh-ed25519") + _s(public)).decode()


def openssh_private(private_seed, public, comment="bipagents-move"):
    """OpenSSH's own format (openssh-key-v1), unencrypted: what ssh and sshd read."""
    check = os.urandom(4)
    inner = (check + check + _s(b"ssh-ed25519") + _s(public) + _s(private_seed + public) + _s(comment.encode()))
    pad = 1
    while len(inner) % 8:
        inner += bytes([pad])
        pad += 1
    body = (b"openssh-key-v1\0" + _s(b"none") + _s(b"none") + _s(b"") + struct.pack(">I", 1)
            + _s(_s(b"ssh-ed25519") + _s(public)) + _s(inner))
    text = base64.b64encode(body).decode()
    lines = [text[i:i + 70] for i in range(0, len(text), 70)]
    return "-----BEGIN OPENSSH PRIVATE KEY-----\n" + "\n".join(lines) + "\n-----END OPENSSH PRIVATE KEY-----\n"


def keypair(key, label):
    private_seed = seed(key, label)
    public = ed25519_public(private_seed)
    return openssh_private(private_seed, public), openssh_public(public)


# -- the move's folder and state ----------------------------------------------------------------------------------------

def folder(move_id, root=MOVES):
    if not isinstance(move_id, str) or not ID.match(move_id):
        raise MoveError("not a move id")
    return os.path.join(root, move_id)


def write_private(path, text):
    os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
    fd = os.open(path + ".tmp", os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as fh:
        fh.write(text)
    os.replace(path + ".tmp", path)


def read_json(path):
    try:
        with open(path) as fh:
            data = json.load(fh)
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def save(move_id, root=MOVES, **fields):
    path = os.path.join(folder(move_id, root), "state.json")
    state = read_json(path)
    state.update(fields)
    state["id"] = move_id
    state["updated"] = int(time.time())
    write_private(path, json.dumps(state))
    return state


def states(root=MOVES):
    """Each move's phase and numbers for the beat (times, counts, bytes, an error's words: nothing of anyone)."""
    found = []
    try:
        names = sorted(os.listdir(root))
    except OSError:
        return found
    for name in names:
        state = read_json(os.path.join(root, name, "state.json"))
        if state:
            found.append({k: state[k] for k in STATE_FIELDS if k in state})
    return found


def order_of(path):
    order = read_json(path)
    move = order.get("move") or {}
    if not isinstance(move, dict) or not ID.match(str(move.get("id", ""))) or not move.get("key"):
        raise MoveError("the order holds no move")
    return order, move


# -- the place's own account --------------------------------------------------------------------------------------------

def place(number):
    entry = pwd.getpwnam(account(number))
    return entry


def as_user(entry, args, timeout=900):
    """`bipinstall <args>` as the place's account (its installer, its systemd user manager); the last JSON line."""
    home = entry.pw_dir
    command = ["sudo", "-u", entry.pw_name, "env", "HOME=" + home, "USER=" + entry.pw_name,
               "XDG_RUNTIME_DIR=/run/user/%d" % entry.pw_uid,
               "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/%d/bus" % entry.pw_uid,
               "PATH=%s/.local/bin:/usr/local/bin:/usr/bin:/bin" % home,
               "PYTHONPATH=%s/BipAgents/installer" % home,
               "%s/BipAgents/installer/.venv/bin/python" % home, "-m", "bipinstall"] + list(args)
    done = subprocess.run(command, capture_output=True, text=True, timeout=timeout, cwd=home)
    for line in reversed(done.stdout.strip().splitlines()):
        if line.startswith("{"):
            try:
                result = json.loads(line)
            except ValueError:
                continue
            if done.returncode != 0 or result.get("error"):
                raise MoveError("%s: %s" % (" ".join(args), str(result.get("error"))[:200]))
            return result
    raise MoveError("%s gave no answer (%d): %s" % (" ".join(args), done.returncode, done.stderr.strip()[-200:]))


def bridge_settings(home):
    import tomllib
    with open(os.path.join(home, ".config", "bipagents", "bridge.toml"), "rb") as fh:
        return tomllib.load(fh)


def bridge(home, method, path, body=None, timeout=30):
    """A call to the place's own bridge on this server, with its key (root reads it from the place's settings)."""
    conf = bridge_settings(home)
    host = conf.get("host") or "127.0.0.1"
    host = "127.0.0.1" if host in ("0.0.0.0", "::") else host
    url = "http://%s:%d%s" % (host, int(conf.get("port", 8643)), path)
    data = json.dumps(body).encode() if body is not None else (b"{}" if method == "POST" else None)
    request = urllib.request.Request(url, data=data, method=method, headers={
        "Authorization": "Bearer " + str(conf.get("bridge_key", "")), "Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.loads(response.read().decode() or "{}")


def gateways_answer(home, timeout=180):
    """Each agent's Hermes gateway answers on this server (GET /v1/capabilities with the agent's own key)."""
    deadline = time.time() + timeout
    waiting = None
    while time.time() < deadline:
        waiting = []
        for name, agent in (bridge_settings(home).get("agents") or {}).items():
            if not isinstance(agent, dict) or not agent.get("hermes_url"):
                continue
            request = urllib.request.Request(str(agent["hermes_url"]).rstrip("/") + "/v1/capabilities", headers={
                "Authorization": "Bearer " + str(agent.get("hermes_key", ""))})
            try:
                with urllib.request.urlopen(request, timeout=10):
                    pass
            except (OSError, ValueError):
                waiting.append(name)
        if not waiting:
            return []
        time.sleep(3)
    return waiting


def subids(user, path="/etc/subuid"):
    """The first of the account's subordinate ids (its agents' sandboxes), else None."""
    try:
        with open(path) as fh:
            for line in fh:
                parts = line.strip().split(":")
                if len(parts) == 3 and parts[0] == user:
                    return int(parts[1])
    except (OSError, ValueError):
        pass
    return None


def shifted(value, own_from, own_to, sub_from, sub_to):
    """An owner of the source's files as the target's: the place's own id, its sandboxes' range moved along."""
    if value == own_from:
        return own_to
    if sub_from is not None and sub_to is not None and sub_from <= value < sub_from + SUB_RANGE:
        return value - sub_from + sub_to
    return value


def shift_owners(paths, src, dst):
    """Every file under `paths` given to the target's ids (rsync kept the source's numbers). Returns how many."""
    changed = 0
    for top in paths:
        if not os.path.lexists(top):
            continue
        walk = [(top, [], [])] if not os.path.isdir(top) or os.path.islink(top) else os.walk(top)
        for here, dirs, names in walk:
            for path in [here] + [os.path.join(here, n) for n in dirs + names]:
                try:
                    st = os.lstat(path)
                except OSError:
                    continue
                uid = shifted(st.st_uid, src["uid"], dst["uid"], src.get("sub_uid"), dst.get("sub_uid"))
                gid = shifted(st.st_gid, src["gid"], dst["gid"], src.get("sub_gid"), dst.get("sub_gid"))
                if (uid, gid) != (st.st_uid, st.st_gid):
                    os.lchown(path, uid, gid)
                    changed += 1
    return changed


def rehome(path, source_home, target_home):
    """Where a folder of the source place goes on the target: the same place under the target's home."""
    rel = os.path.relpath(path, source_home)
    if rel == "." or rel.startswith(".."):
        raise MoveError("a folder of the place is outside its home: %s" % os.path.basename(path))
    return os.path.join(target_home, rel)


# -- the target: its listener and the commands it accepts ---------------------------------------------------------------

def sshd_config(base, port):
    return "\n".join([
        "Port %d" % port, "ListenAddress 0.0.0.0", "HostKey %s/host_key" % base, "PidFile %s/sshd.pid" % base,
        "AuthorizedKeysFile %s/authorized_keys" % base, "PermitRootLogin forced-commands-only",
        "AllowUsers root", "PasswordAuthentication no", "KbdInteractiveAuthentication no", "PubkeyAuthentication yes",
        "UsePAM yes", "AllowTcpForwarding no", "AllowAgentForwarding no", "X11Forwarding no", "PermitTTY no",
        "PermitUserEnvironment no", "MaxAuthTries 2", "LoginGraceTime 30", "ClientAliveInterval 30", "LogLevel INFO",
        ""])


def authorized_line(public, source, move_id, script):
    return 'restrict,from="%s",command="/usr/bin/python3 %s serve %s" %s\n' % (source, script, move_id, public)


def listen(order_path, root=MOVES, script=None, run=subprocess.run):
    """`move_in`: the move's sshd on its port, for the source's address only, with the move's keys."""
    order, move = order_of(order_path)
    move_id = move["id"]
    base = folder(move_id, root)
    seat = int(order["seat"])
    source = str(move["source"]["address"])
    port = int(move.get("port") or 2222)
    if not 1024 <= port <= 65535 or port in range(443, 443 + MAX_SEATS):
        raise MoveError("the move's port is not one this server may open")
    host_private, _ = keypair(move["key"], "host")
    _, client_public = keypair(move["key"], "client")
    write_private(os.path.join(base, "host_key"), host_private)
    write_private(os.path.join(base, "authorized_keys"),
                  authorized_line(client_public, source, move_id, script or os.path.join(LIB, "place-move.py")))
    write_private(os.path.join(base, "sshd_config"), sshd_config(base, port))
    os.makedirs("/run/sshd", mode=0o755, exist_ok=True)
    save(move_id, root, role="in", seat=seat, phase="listening", started=int(time.time()), source=source, port=port,
         expires=int(move.get("expires") or time.time() + 6 * 3600))
    run(["ufw", "allow", "from", source, "to", "any", "port", str(port), "proto", "tcp"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    run(["systemd-run", "--unit", "bipagents-move-%s-sshd" % move_id, "--collect", "/usr/sbin/sshd", "-D", "-e",
         "-f", os.path.join(base, "sshd_config")], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    log("move %s: listening on %d for %s (place %d)" % (move_id, port, source, seat))
    return 0


def close(move_id, root=MOVES, run=subprocess.run):
    base = folder(move_id, root)
    state = read_json(os.path.join(base, "state.json"))
    run(["systemctl", "stop", "bipagents-move-%s-sshd" % move_id], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if state.get("source") and state.get("port"):
        run(["ufw", "delete", "allow", "from", str(state["source"]), "to", "any", "port", str(state["port"]), "proto",
             "tcp"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for name in ("host_key", "authorized_keys", "order.json"):
        try:
            os.unlink(os.path.join(base, name))
        except OSError:
            pass
    if state.get("phase") not in ("done", "failed", "cancelled"):
        save(move_id, root, phase="closed")
    return 0


def allowed_rsync(words, home):
    """rsync as the receiving end only, writing under the target place's home (or its move-in folder)."""
    if len(words) < 3 or words[0] != "rsync" or "--server" not in words or "--sender" in words:
        return False
    if any(w.startswith(("--rsync-path", "--log-file", "--write-devices", "--remove-source", "--temp-dir",
                         "--partial-dir", "--backup-dir", "--copy-dest", "--link-dest", "--compare-dest",
                         "--files-from", "--include-from", "--exclude-from", "--filter")) for w in words[1:]):
        return False
    dest = os.path.normpath(words[-1])
    home = os.path.normpath(home)
    return dest.startswith(home + "/") and ".." not in words[-1].split("/")


def serve(move_id, command=None, root=MOVES):
    """The forced command of the move's SSH key on the target: what the source asks, checked."""
    command = command if command is not None else os.environ.get("SSH_ORIGINAL_COMMAND", "")
    state = read_json(os.path.join(folder(move_id, root), "state.json"))
    if state.get("role") != "in" or state.get("phase") in ("closed", "cancelled"):
        print(json.dumps({"error": "no move here"}))
        return 1
    entry = place(int(state["seat"]))
    words = shlex.split(command)
    if not words:
        print(json.dumps({"error": "nothing asked"}))
        return 1
    if words[0] == "rsync":
        if not allowed_rsync(words, entry.pw_dir):
            sys.stderr.write("refused\n")
            return 1
        os.makedirs(os.path.dirname(os.path.normpath(words[-1])), exist_ok=True)
        os.execvp("rsync", words)
    try:
        if words[0] == "info":
            out = {"home": entry.pw_dir, "user": entry.pw_name, "uid": entry.pw_uid, "gid": entry.pw_gid,
                   "sub_uid": subids(entry.pw_name), "sub_gid": subids(entry.pw_name, "/etc/subgid"),
                   "seat": int(state["seat"])}
        elif words[0] == "prepare":
            save(move_id, root, phase="receiving")
            out = as_user(entry, ["move-in", "prepare"], timeout=300)
        elif words[0] == "finish" and len(words) == 2:
            asked = json.loads(base64.b64decode(words[1]).decode())
            out = finish(move_id, entry, asked, root)
        elif words[0] == "abort":
            out = abort(move_id, entry, root)
        else:
            out = {"error": "not a move step"}
    except (MoveError, OSError, ValueError, KeyError, subprocess.SubprocessError) as exc:
        save(move_id, root, phase="failed", error=str(exc)[:300])
        out = {"error": str(exc)[:300]}
    print(json.dumps(out), flush=True)
    return 0 if "error" not in out else 1


def finish(move_id, entry, asked, root=MOVES):
    """The target's switch: owners shifted, settings adjusted, started, checked, tasks and routines resumed."""
    save(move_id, root, phase="starting")
    home = entry.pw_dir
    dst = {"uid": entry.pw_uid, "gid": entry.pw_gid, "sub_uid": subids(entry.pw_name),
           "sub_gid": subids(entry.pw_name, "/etc/subgid")}
    paths = [p for p in asked.get("paths", []) if os.path.normpath(p).startswith(home + "/")]
    shifted_count = shift_owners(paths + [os.path.join(home, ".local", "state", "bipagents", "move-in")],
                                 asked["source"], dst)
    out = as_user(entry, ["move-in", "finish"], timeout=900)
    deadline = time.time() + 120
    while True:
        try:
            bridge(home, "GET", "/v1/place/move", timeout=10)
            break
        except (OSError, ValueError) as exc:
            if time.time() > deadline:
                raise MoveError("the bridge did not start: %s" % type(exc).__name__)
            time.sleep(1)
    waiting = gateways_answer(home)
    if waiting:
        raise MoveError("%d agent(s) did not answer" % len(waiting))
    resumed = bridge(home, "POST", "/v1/place/move/resume", {}, timeout=120)
    status = bridge(home, "GET", "/v1/place/move", timeout=60)
    enabled = [r for r in status.get("routines", []) if r.get("enabled") and r.get("next_run_at")]
    if len(enabled) < int(asked.get("routines", 0)):
        raise MoveError("%d of %d routines scheduled" % (len(enabled), int(asked.get("routines", 0))))
    verified = {"bridge": True, "agents": True, "routines": len(enabled),
                "tasks_resumed": len(resumed.get("resumed_jobs", [])), "caught_up": len(resumed.get("caught_up", [])),
                "owners_shifted": shifted_count, "volumes": len(out.get("volumes", []))}
    save(move_id, root, phase="done", verified=verified, result="moved")
    # The listener goes a minute later (this very command runs inside it).
    subprocess.run(["systemd-run", "--on-active=60", "--collect", "/usr/bin/python3",
                    os.path.join(LIB, "place-move.py"), "close", move_id],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return {"ok": True, "verified": verified}


def abort(move_id, entry, root=MOVES):
    """The move failed or was called off: the target's copy stopped (its account is reset by the guichet later)."""
    try:
        as_user(entry, ["move-out", "stop"], timeout=300)
    except MoveError:
        pass
    save(move_id, root, phase="cancelled", result="aborted")
    return {"ok": True}


# -- the source: the move itself ------------------------------------------------------------------------------------------

class Channel:
    """The source's end of the move's SSH channel."""

    def __init__(self, base, address, port, key):
        self.address, self.port = address, port
        client_private, _ = keypair(key, "client")
        _, host_public = keypair(key, "host")
        self.key_file = os.path.join(base, "client_key")
        self.known = os.path.join(base, "known_hosts")
        write_private(self.key_file, client_private)
        write_private(self.known, "[%s]:%d %s\n" % (address, port, host_public))

    def ssh_args(self):
        return ["ssh", "-p", str(self.port), "-i", self.key_file, "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes",
                "-o", "UserKnownHostsFile=" + self.known, "-o", "StrictHostKeyChecking=yes",
                "-o", "GlobalKnownHostsFile=/dev/null", "-o", "ConnectTimeout=15", "-o", "ServerAliveInterval=15",
                "-o", "LogLevel=ERROR"]

    def ask(self, *words, timeout=900):
        done = subprocess.run(self.ssh_args() + ["root@" + self.address, " ".join(words)], capture_output=True,
                              text=True, timeout=timeout)
        for line in reversed(done.stdout.strip().splitlines()):
            if line.startswith("{"):
                result = json.loads(line)
                if result.get("error"):
                    raise MoveError("target: " + str(result["error"])[:300])
                return result
        raise MoveError("the target did not answer %s (%d): %s" % (words[0], done.returncode,
                                                                   done.stderr.strip()[-200:]))

    def rsync(self, source, dest, rules=None, checksum=False, timeout=6 * 3600):
        """One copy of a folder (or a file) to the target; (bytes sent of changed files, files changed)."""
        args = ["rsync", "-aH", "--numeric-ids", "--delete", "--partial", "--stats", "-z",
                "-e", " ".join(shlex.quote(a) for a in self.ssh_args())]
        if checksum:
            args.append("--checksum")
        rules_file = None
        if rules:
            fd, rules_file = tempfile.mkstemp(prefix="rules-", dir=os.path.dirname(self.key_file))
            with os.fdopen(fd, "w") as fh:
                fh.write("\n".join(rules) + "\n")
            args.append("--filter=merge " + rules_file)
        is_dir = os.path.isdir(source) and not os.path.islink(source)
        args += [source.rstrip("/") + ("/" if is_dir else ""), "root@%s:%s" % (self.address,
                                                                            dest.rstrip("/") + ("/" if is_dir else ""))]
        try:
            done = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
        finally:
            if rules_file:
                os.unlink(rules_file)
        if done.returncode not in (0, 24):   # 24: files vanished while copied (a running place)
            raise MoveError("rsync of %s failed (%d): %s" % (os.path.basename(source.rstrip("/")), done.returncode,
                                                             done.stderr.strip()[-300:]))
        return stats(done.stdout)


def stats(output):
    def number(label):
        found = re.search(r"^%s:\s*([\d,]+)" % re.escape(label), output, re.M)
        return int(found.group(1).replace(",", "")) if found else 0
    return number("Total transferred file size"), number("Number of regular files transferred")


def copy_round(channel, plan, target_home, checksum=False, with_stage=False):
    sent = files = 0
    for name, path in sorted(plan["roots"].items()):
        if not os.path.lexists(path):
            continue
        dest = rehome(path, plan["home"], target_home)
        size, count = channel.rsync(path, dest, plan["rules"].get(name), checksum=checksum)
        sent += size
        files += count
    if with_stage:
        dest = os.path.join(target_home, ".local", "state", "bipagents", "move-in", "stage")
        size, count = channel.rsync(plan["stage"], dest, checksum=True)
        sent += size
        files += count
    return sent, files


def quiet_moment(home, start, end, keep_copying, now=time.time, sleep=time.sleep):
    """Waits for the window, then for a moment with no turn running (at the latest LATEST_BEFORE_END before its
    end). `keep_copying` is called every ROUND_EVERY meanwhile. Returns the bridge's status then."""
    last_round = now()
    while True:
        status = {}
        if now() >= start:
            try:
                status = bridge(home, "GET", "/v1/place/move", timeout=60)
            except (OSError, ValueError):
                status = {}
            if int(status.get("turns_running", 1)) == 0 or now() >= end - LATEST_BEFORE_END:
                return status
        if now() - last_round >= ROUND_EVERY:
            keep_copying()
            last_round = now()
        sleep(5)


def drained(home, timeout=DRAIN_WAIT, now=time.time, sleep=time.sleep):
    """The running turns ended and the running tasks paused (or the time is up). Returns the last status."""
    deadline = now() + timeout
    status = {}
    while now() < deadline:
        status = bridge(home, "GET", "/v1/place/move", timeout=60)
        if int(status.get("turns_running", 0)) == 0 and not status.get("jobs_running"):
            return status
        sleep(2)
    return status


def move_out(order_path, root=MOVES):
    order, move = order_of(order_path)
    move_id = move["id"]
    base = folder(move_id, root)
    seat = int(order["seat"])
    entry = place(seat)
    home = entry.pw_dir
    now = time.time()
    window = move.get("window") or {}
    start = float(window.get("start") or now)
    end = float(window.get("end") or start + AUTO_WINDOW)
    back_in = int(move.get("back_in") or 60)
    save(move_id, root, role="out", seat=seat, phase="connecting", started=int(now), round=0)
    target = move["target"]
    channel = Channel(base, str(target["address"]), int(target.get("port") or 2222), move["key"])
    deadline = min(end, now + 3600)
    while True:
        try:
            info = channel.ask("info", timeout=60)
            break
        except (MoveError, subprocess.SubprocessError, ValueError) as exc:
            if time.time() > deadline:
                raise MoveError("the target never answered: %s" % str(exc)[:200])
            time.sleep(15)
    plan = as_user(entry, ["move-out", "plan"], timeout=300)
    for path in plan["roots"].values():
        rehome(path, plan["home"], info["home"])   # all under the home, or stop now
    channel.ask("prepare", timeout=300)
    save(move_id, root, phase="copying")
    # 1. Copies while the agents work, until little changes between two.
    total, rounds, delta = 0, 0, None
    while rounds < MAX_ROUNDS:
        rounds += 1
        sent, files = copy_round(channel, plan, info["home"])
        total += sent
        delta = sent
        save(move_id, root, round=rounds, copied_bytes=total, delta_bytes=delta, files=files)
        log("move %s: copy %d, %d bytes in %d files" % (move_id, rounds, sent, files))
        if rounds > 1 and sent <= SMALL_DELTA:
            break

    def keep_copying():
        sent, files = copy_round(channel, plan, info["home"])
        save(move_id, root, round=read_json(os.path.join(base, "state.json")).get("round", 0) + 1,
             delta_bytes=sent, files=files)
    # 2. The quiet moment.
    save(move_id, root, phase="waiting")
    before = quiet_moment(home, start, end, keep_copying)
    routines = len([r for r in before.get("routines", []) if r.get("enabled") and r.get("next_run_at")])
    # 3. Drain.
    t_drain = time.time()
    save(move_id, root, phase="draining")
    bridge(home, "POST", "/v1/place/move/drain", {"until": time.time() + 900, "back_in": back_in}, timeout=120)
    copy_round(channel, plan, info["home"])
    drained(home)
    bridge(home, "POST", "/v1/place/move/drain", {"switching": True, "back_in": back_in}, timeout=120)
    # 4. Switch.
    t_switch = time.time()
    save(move_id, root, phase="switching", drain_seconds=round(t_switch - t_drain, 1))
    stopped = False
    try:
        as_user(entry, ["move-out", "stop"], timeout=300)
        stopped = True
        copy_round(channel, plan, info["home"], checksum=True, with_stage=True)
        source = {"uid": entry.pw_uid, "gid": entry.pw_gid, "sub_uid": subids(entry.pw_name),
                  "sub_gid": subids(entry.pw_name, "/etc/subgid")}
        paths = [rehome(p, plan["home"], info["home"]) for p in plan["roots"].values()]
        asked = base64.b64encode(json.dumps({"source": source, "paths": paths, "routines": routines,
                                             "back_in": back_in}).encode()).decode()
        result = channel.ask("finish", asked, timeout=1200)
    except (MoveError, OSError, ValueError, subprocess.SubprocessError) as exc:
        rollback(channel, entry, stopped)
        save(move_id, root, phase="failed", error=str(exc)[:300], result="rolled back")
        raise
    downtime = round(time.time() - t_switch, 1)
    write_private(os.path.join(home, MOVED_AWAY), json.dumps({"move": move_id, "at": int(time.time())}))
    save(move_id, root, phase="done", result="moved", downtime_seconds=downtime, verified=result.get("verified"),
         kept_until=int(time.time() + KEEP_SECONDS))
    for name in ("client_key", "known_hosts", "order.json"):
        try:
            os.unlink(os.path.join(base, name))
        except OSError:
            pass
    log("move %s: done, %.1f s without the agents" % (move_id, downtime))
    return 0


def rollback(channel, entry, stopped):
    """The switch failed: the target's copy stopped, the source as it was (its services, its tasks, its routines)."""
    try:
        channel.ask("abort", timeout=300)
    except (MoveError, OSError, ValueError, subprocess.SubprocessError):
        pass
    if stopped:
        try:
            as_user(entry, ["resume"], timeout=300)
        except MoveError:
            pass   # `bipinstall resume` prints words, not JSON: it ran
    deadline = time.time() + 120
    while time.time() < deadline:
        try:
            bridge(entry.pw_dir, "POST", "/v1/place/move/resume", {}, timeout=120)
            return
        except (OSError, ValueError):
            time.sleep(2)


def cancel(move_id, root=MOVES):
    base = folder(move_id, root)
    state = read_json(os.path.join(base, "state.json"))
    subprocess.run(["systemctl", "stop", "bipagents-move-%s" % move_id], stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL)
    if state.get("role") == "in":
        return close(move_id, root)
    if state.get("role") == "out" and state.get("phase") not in ("done", "failed"):
        entry = place(int(state["seat"]))
        if state.get("phase") in ("draining", "switching"):
            try:
                as_user(entry, ["resume"], timeout=300)
            except MoveError:
                pass
            try:
                bridge(entry.pw_dir, "POST", "/v1/place/move/resume", {}, timeout=120)
            except (OSError, ValueError):
                pass
        save(move_id, root, phase="cancelled")
    return 0


def cleanup(root=MOVES, now=None, wipe=None):
    """Listeners past their time closed; source copies kept 7 days, then their account removed (only when it still
    holds that move's mark: a seat installed again for someone else since is never touched)."""
    now = time.time() if now is None else now
    removed = []
    for state in states(root):
        move_id = state.get("id")
        full = read_json(os.path.join(root, str(move_id), "state.json"))
        if full.get("role") == "in" and full.get("phase") not in ("closed", "cancelled") \
                and now > float(full.get("expires") or 0):
            close(move_id, root)
        if full.get("role") == "out" and full.get("phase") == "done" and now > float(full.get("kept_until") or now + 1):
            try:
                entry = place(int(full["seat"]))
                mark = read_json(os.path.join(entry.pw_dir, MOVED_AWAY))
            except (KeyError, ValueError):
                mark = {}
            if mark.get("move") == move_id and wipe:
                wipe(entry.pw_name)
                removed.append(entry.pw_name)
            save(move_id, root, phase="removed")
    return removed


def main(argv):
    if not argv:
        print(__doc__)
        return 2
    what, rest = argv[0], argv[1:]
    try:
        if what == "in" and rest:
            return listen(rest[0])
        if what == "out" and rest:
            try:
                return move_out(rest[0])
            except MoveError as exc:
                log("move failed: %s" % exc)
                return 1
        if what == "serve" and rest:
            return serve(rest[0])
        if what == "cancel" and rest:
            return cancel(rest[0])
        if what == "close" and rest:
            return close(rest[0])
        if what == "states":
            print(json.dumps(states()))
            return 0
    except MoveError as exc:
        log("refused: %s" % exc)
        return 1
    print("usage: place-move.py in|out <order file> | serve|cancel|close <id> | states", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
