#!/usr/bin/env python3
"""BipAgents: the guichet's orders for the places of this server (guichet/bipguichet/orders.py).

Run as root every two minutes by bipagents-seats.timer, set up at first boot. Its call to the guichet, with the token
its first boot gave it, is the server's heartbeat (guichet/bipguichet/watch.py): it says, for each place, whether its
bridge's door answers here, whether its Hermes runs (or sleeps), the BipAgents version it runs, when its last backup worked
and whether its encrypted space waits for its key ("locked" and why),
and for the server its memory, swap, disk and load, and for each place the memory it holds now and its highest since the
last beat ("places") -- yes or no, a version, a time and numbers, nothing of anyone. The
guichet answers with each place's AI credit state ("credit": ok, low, full, stopped; written where the place's bridge
reads it, which pauses its routines, browser and background tasks while the month's credit is used) and its orders, and the agent does one only when the guichet's Ed25519 key signed it (checked with
openssl against /etc/bipagents/guichet.pub), for this server, not expired, not done before. There are three orders:
install place n again for its next person -- its account bip<n> (bip for place 1), everything in it and its
services go, then installer/cloud/add-seat.sh sets it up again with the new pairing secret; bring place n to a
given BipAgents version (the guichet's update waves, or their way back), which its own updater does; and let place n
pair again with a new secret (its person recovered their account without any of their devices). The other places
are not touched, and nothing is ever read or sent back: only "done".

With --bootstrap, run once by the server's first-boot script (the guichet's guichet-rs/src/bootstrap.rs), before any
place is installed: the script, which the provider keeps and serves to anyone on the server through its metadata
service, holds no secret, only a one-time token. With it root fetches from the guichet, over HTTPS and from this
server's own address, each place's setup (its pairing secret, its backups' token, its key service grant) and the seat
agent's own token; the guichet then forgets the one-time token (pool.rs: bootstrap). The setups go to
/etc/bipagents/cloud.json (place 1) and cloud<n>.json (root's, 0600), which add-seat.sh and the installer read then
shred. A place installed again later gets its setup in a signed order (above), never through the first-boot script.

With --rebooting, run by bipagents-reboot-notice.service as the server shuts down (the night's reboot after a kernel
update, user-data.yaml), it only tells the guichet the server is rebooting when a reboot is under way: the guichet
then waits a quarter of an hour before taking its silence, or its places not answering yet, for an outage. The beat
says how long the server has been up for the same reason.

It also keeps each place's memory soft-capped (MemoryHigh on its user slice, never a hard limit: below), samples
each place's memory every 15 s between beats (--sample, bipagents-memory.timer, which it sets up), and sets up zram
on a server that has none.

ASCII only, like user-data.yaml and add-seat.sh (installer/tests/test_cloud_init.py checks it).
"""
import base64
import contextlib
import json
import os
import pwd
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

CONF = "/etc/bipagents/guichet.json"
PUBLIC_KEY = "/etc/bipagents/guichet.pub"
DONE = "/var/lib/bipagents/orders-done"
LIB = "/usr/local/lib/bipagents"
MAX_SEATS = 8
BACKUP_STATE = ".local/state/bipagents/backup.json"
ASLEEP = ".local/state/bipagents/asleep"   # its bridge put its Hermes to sleep (bridge-rs/src/sleep.rs)
SETUP = ".config/bipagents/setup.json"     # its bridge's pairing secret while it waits for an app (cloud_setup.py)
LOCKED = ".local/share/bipagents/space/locked.json"   # its encrypted space not open yet, and why (space.rs)
CREDIT = ".local/state/bipagents/credit.json"   # its plan's AI credit as the guichet says it (bridge-rs/src/credit.rs)
CREDIT_STATES = ("ok", "low", "full", "stopped")
PAIR_CHARS = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"


def log(text):
    print(text, flush=True)


def verified(body, signature, public_key=PUBLIC_KEY):
    """The guichet's signature over the order's exact text."""
    try:
        raw = base64.b64decode(signature, validate=True)
    except (ValueError, TypeError):
        return False
    with tempfile.TemporaryDirectory() as folder:
        body_path, sig_path = os.path.join(folder, "body"), os.path.join(folder, "sig")
        with open(body_path, "wb") as fh:
            fh.write(body.encode("utf-8"))
        with open(sig_path, "wb") as fh:
            fh.write(raw)
        result = subprocess.run(["openssl", "pkeyutl", "-verify", "-pubin", "-inkey", public_key, "-rawin",
                                 "-in", body_path, "-sigfile", sig_path], capture_output=True)
    return result.returncode == 0


def accepted(order, server, done, now):
    """The order's content when it is one this server must do, else None (with why in the log)."""
    try:
        body = json.loads(order["body"])
        number = int(body["seat"])
        ok = (body.get("server") == server and int(body["order"]) == int(order["id"]) and 1 <= number <= MAX_SEATS
              and int(body["expires"]) > now)
        if body.get("action") == "reset":
            setup = body["setup"]
            ok = ok and isinstance(setup, dict) and isinstance(setup.get("pair"), str) \
                and int(setup.get("port", 0)) == 442 + number
        elif body.get("action") == "update":
            commit = body["commit"]
            ok = ok and isinstance(commit, str) and len(commit) == 40 and all(c in "0123456789abcdef" for c in commit)
        elif body.get("action") == "reopen":
            pair = body["pair"]
            ok = ok and isinstance(pair, str) and 40 <= len(pair) <= 64 and all(c in PAIR_CHARS for c in pair)
        elif body.get("action") in MOVE_ACTIONS:
            ok = ok and move_valid(body)
        elif body.get("action") == "remove":
            ok = ok and remove_valid(body)
        else:
            ok = False
    except (KeyError, TypeError, ValueError):
        ok = False
    if not ok:
        log("order %s refused: not for this server, expired or malformed" % order.get("id"))
        return None
    if int(order["id"]) in done:
        return None
    return body


def account(number):
    return "bip" if number == 1 else "bip%d" % number


def run(*args):
    subprocess.run(list(args), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def wipe(user):
    """The account, its services, its sandboxes and every file of it."""
    try:
        uid = pwd.getpwnam(user).pw_uid
    except KeyError:
        return
    run("loginctl", "disable-linger", user)
    run("systemctl", "stop", "user@%d.service" % uid)
    run("pkill", "-KILL", "-u", user)
    time.sleep(2)
    run("userdel", "--remove", "--force", user)
    for path in ("/etc/subuid", "/etc/subgid"):
        try:
            with open(path) as fh:
                lines = [line for line in fh if not line.startswith(user + ":")]
            with open(path, "w") as fh:
                fh.writelines(lines)
        except OSError:
            pass
    run("rm", "-rf", "/home/" + user)


def reset(number, setup):
    user = account(number)
    log("place %d (%s): installed again" % (number, user))
    wipe(user)
    path = "/etc/bipagents/cloud%d.json" % number
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as fh:
        json.dump(setup, fh)
    subprocess.run(["bash", os.path.join(LIB, "add-seat.sh"), str(number), path])


SHARED_SETTING = "/etc/bipagents/shared-hermes.json"
SHARED_REPO = os.path.join(LIB, "repo")   # root's own BipAgents, never a place's (an agent could change those)
PUBLIC_REPO = "https://github.com/Alexbob0/BipAgents.git"


def shared_on():
    """The server's places share one read-only Hermes (installer/bipinstall/shared_hermes.py): on unless root wrote
    {"enabled": false} there; the first round writes the setting."""
    try:
        with open(SHARED_SETTING) as fh:
            return json.load(fh).get("enabled") is not False
    except OSError:
        os.makedirs(os.path.dirname(SHARED_SETTING), exist_ok=True)
        with open(SHARED_SETTING, "w") as fh:
            json.dump({"enabled": True, "root": "/opt/bipagents/hermes"}, fh)
        os.chmod(SHARED_SETTING, 0o644)
        return True
    except ValueError:
        return False


def prepare_shared(commit):
    """The shared Hermes that BipAgents `commit` pins, built by root before a place moves to it (its own update
    then points at it). Built from root's own checkout at that commit. A failure leaves the place on its current
    Hermes: its update says the version isn't ready and it keeps the one it has. Usually fetched ready-made (its
    release), not built. The agents' images of that version are pulled into the server's store too
    (installer/bipinstall/shared_images.py), with root's nightly pull set up the first time. `commit` "main": the
    latest (a place installed without a pinned version, add-seat.sh)."""
    if not os.path.isdir(os.path.join(SHARED_REPO, ".git")):
        subprocess.run(["git", "clone", "--quiet", "--filter=blob:none", PUBLIC_REPO, SHARED_REPO], check=True,
                       timeout=600)
    subprocess.run(["git", "-C", SHARED_REPO, "fetch", "--quiet", "origin"], check=True, timeout=600)
    subprocess.run(["git", "-C", SHARED_REPO, "-c", "advice.detachedHead=false", "checkout", "--quiet", "--force",
                    "origin/main" if commit == "main" else commit], check=True, timeout=120)
    env = dict(os.environ, PYTHONPATH=os.path.join(SHARED_REPO, "installer"))
    if shared_on():
        with open("/var/log/bipagents-shared-hermes.log", "a") as out:
            done = subprocess.run(["python3", "-m", "bipinstall.shared_hermes", "build"], env=env, stdout=out,
                                  stderr=out, timeout=3600, cwd=SHARED_REPO)
        log("shared Hermes for %s: %s" % (commit[:12], "ready" if done.returncode == 0 else "not built"))
    # --timer the first time: the nightly pull, and the places' views of the store mounted again at each boot.
    timer = [] if os.path.exists("/etc/systemd/system/bipagents-image-views.service") else ["--timer"]
    with open("/var/log/bipagents-images.log", "a") as out:
        done = subprocess.run(["python3", "-m", "bipinstall.shared_images", "pull"] + timer, env=env, stdout=out,
                              stderr=out, timeout=3600, cwd=SHARED_REPO)
    log("shared images for %s: %s" % (commit[:12], "ready" if done.returncode == 0 else "not all pulled"))


def shared_gc():
    """The shared versions no place points at and no process runs any more, deleted."""
    if not shared_on() or not os.path.isdir(os.path.join(SHARED_REPO, "installer")):
        return
    env = dict(os.environ, PYTHONPATH=os.path.join(SHARED_REPO, "installer"))
    subprocess.run(["python3", "-m", "bipinstall.shared_hermes", "gc"], env=env, capture_output=True,
                   timeout=600, cwd=SHARED_REPO)


def update(number, commit):
    """Place n brought to BipAgents `commit` by its own updater, as its account (what changed restarts)."""
    user = account(number)
    try:
        uid = pwd.getpwnam(user).pw_uid
    except KeyError:
        return
    try:
        prepare_shared(commit)
    except (OSError, subprocess.SubprocessError) as exc:
        log("shared Hermes for %s not built: %s" % (commit[:12], exc))
    log("place %d (%s): BipAgents %s" % (number, user, commit[:12]))
    with open("/var/log/bipagents-update%d.log" % number, "a") as out:
        subprocess.run(["sudo", "-iu", user, "env", "XDG_RUNTIME_DIR=/run/user/%d" % uid,
                        "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/%d/bus" % uid, "BIPAGENTS_TO=" + commit,
                        "bash", "BipAgents/install.sh", "update", "--auto"], stdout=out, stderr=out, timeout=3600)
    shared_gc()


def reopen(number, pair, home=None):
    """Place n pairs again with `pair` (its person recovered their account without any of their devices): its bridge
    reads the secret, gives the device that proves it a new key and drops the others. Nothing else is touched."""
    user = account(number)
    try:
        entry = pwd.getpwnam(user)
    except KeyError:
        return
    log("place %d (%s): pairing reopened" % (number, user))
    path = os.path.join(home or entry.pw_dir, SETUP)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    fd = os.open(path + ".tmp", os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as fh:
        json.dump({"pair": pair, "reopen": True}, fh)
    os.chown(path + ".tmp", entry.pw_uid, entry.pw_gid)
    os.replace(path + ".tmp", path)


@contextlib.contextmanager
def ipv4_only():
    """Calls made in IPv4 inside it: the guichet accepts the one-time token only from the server's own IPv4 address
    (the one its provider gave it), and a server with IPv6 would otherwise call from that."""
    real = socket.getaddrinfo

    def only_v4(host, port, family=0, type=0, proto=0, flags=0):
        return real(host, port, socket.AF_INET, type, proto, flags)
    socket.getaddrinfo = only_v4
    try:
        yield
    finally:
        socket.getaddrinfo = real


def setup_path(number, folder="/etc/bipagents"):
    """Where a place's setup waits for its install: place 1's for user-data.yaml's own install, the others' for
    add-seat.sh."""
    return os.path.join(folder, "cloud.json" if number == 1 else "cloud%d.json" % number)


def good_setup(number, setup):
    """A place's setup as the guichet gives it: a pairing secret and the door of its number."""
    if not isinstance(setup, dict) or setup.get("v") != 1:
        return False
    pair = setup.get("pair")
    if not (isinstance(pair, str) and 40 <= len(pair) <= 64 and all(c in PAIR_CHARS for c in pair)):
        return False
    try:
        return int(setup.get("port") or 443) == 442 + number
    except (TypeError, ValueError):
        return False


def write_private(path, value):
    """`value` as JSON at `path`, root's only (0600), replaced in one step."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as fh:
        json.dump(value, fh)
    os.chmod(tmp, 0o600)
    os.replace(tmp, path)


def bootstrap(conf_path=CONF, folder="/etc/bipagents", tries=20, wait=15, caller=None):
    """--bootstrap: the one-time token of the first-boot script exchanged at the guichet for each place's setup and the
    seat agent's own token (see above). 0 when done (or done before), 1 when the guichet refused or never answered
    (the script stops there; the guichet installs the server again once its install time is over)."""
    with open(conf_path) as fh:
        conf = json.load(fh)
    once = conf.get("bootstrap")
    if not once:
        return 0 if conf.get("token") else 1
    caller = caller or call
    answer = None
    for attempt in range(tries):
        try:
            with ipv4_only():
                answer = caller(dict(conf, token=once), "POST", "/v1/server/bootstrap")
            break
        except urllib.error.HTTPError as exc:
            # 401 too: the guichet may not have recorded this install yet (its round still going), so asked again;
            # a token used before, too late or not from this server stays refused all along.
            log("bootstrap: the guichet answered %d, again in %d s" % (exc.code, wait))
        except (OSError, ValueError) as exc:
            log("bootstrap: guichet unreachable (%s), again in %d s" % (exc, wait))
        if attempt + 1 < tries:
            time.sleep(wait)
    if not isinstance(answer, dict):
        return 1
    token, seats = answer.get("token"), answer.get("seats")
    try:
        numbers = sorted(int(n) for n in seats) if isinstance(seats, dict) else []
    except (TypeError, ValueError):
        numbers = []
    if answer.get("server") != conf.get("server") or not isinstance(token, str) or len(token) < 32 or not numbers \
            or numbers != list(range(1, len(numbers) + 1)) or len(numbers) > MAX_SEATS \
            or not all(good_setup(n, seats[str(n)]) for n in numbers):
        log("bootstrap: the guichet's answer is not this server's places")
        return 1
    for number in numbers:
        write_private(setup_path(number, folder), seats[str(number)])
    write_private(conf_path, {"url": conf["url"], "token": token, "server": conf["server"]})
    log("bootstrap: %d places' setups and the seat agent's token fetched" % len(numbers))
    return 0


METADATA_BLOCK = "/etc/bipagents/metadata-block.nft"


def metadata_block(rules=METADATA_BLOCK, runner=None):
    """The metadata service for root only (user-data.yaml: bipagents-metadata-block.service), loaded again when its
    table went (a flushed ruleset)."""
    if not os.path.exists(rules):
        return
    runner = runner or quietly
    if runner("nft", "list", "table", "inet", "bipagents_metadata") != 0:
        log("metadata block %s" % ("loaded again" if runner("nft", "-f", rules) == 0 else "not loaded"))


def call(conf, method, path, body=None):
    data = json.dumps(body).encode("utf-8") if body is not None else (b"" if method == "POST" else None)
    headers = {"Authorization": "Bearer " + conf["token"]}
    if body is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(conf["url"].rstrip("/") + path, method=method, data=data, headers=headers)
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.loads(response.read().decode("utf-8") or "{}")


def door_open(port):
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=5):
            return True
    except OSError:
        return False


def last_backup(user):
    """The time of the place's last backup that worked (installer/bipinstall/backup.py writes it), else None."""
    try:
        home = pwd.getpwnam(user).pw_dir
        with open(os.path.join(home, BACKUP_STATE)) as fh:
            value = json.load(fh).get("last_ok")
        return float(value) if isinstance(value, (int, float)) else None
    except (KeyError, OSError, ValueError, AttributeError):
        return None


def locked(home):
    """Why the place's encrypted space is not open ("key service unreachable"...), else None: a place waiting
    for its key is locked, not down (bridge-rs/src/space.rs writes the file outside the encrypted folder)."""
    try:
        with open(os.path.join(home, LOCKED)) as fh:
            reason = json.load(fh).get("reason")
        return str(reason)[:120] if reason else "locked"
    except (OSError, ValueError, AttributeError):
        return None


def running(uid, words, proc="/proc"):
    """A process of this account whose command line holds all `words` (what runs, never what it does)."""
    for pid in os.listdir(proc):
        if not pid.isdigit():
            continue
        try:
            if os.stat(os.path.join(proc, pid)).st_uid != uid:
                continue
            with open(os.path.join(proc, pid, "cmdline"), "rb") as fh:
                command = fh.read().replace(b"\0", b" ").decode("utf-8", "replace")
        except OSError:
            continue
        if all(word in command for word in words):
            return True
    return False


def version(home):
    """The BipAgents commit the place runs (its repository's HEAD, read without git), else None."""
    git = os.path.join(home, "BipAgents", ".git")
    try:
        with open(os.path.join(git, "HEAD")) as fh:
            head = fh.read().strip()
        if not head.startswith("ref: "):
            return head[:12] or None
        ref = head[5:]
        if os.path.exists(os.path.join(git, ref)):
            with open(os.path.join(git, ref)) as fh:
                return fh.read().strip()[:12] or None
        with open(os.path.join(git, "packed-refs")) as fh:
            for line in fh:
                if line.strip().endswith(" " + ref):
                    return line.split()[0][:12]
    except OSError:
        pass
    return None


def uptime(path="/proc/uptime"):
    """Seconds since the server started, else None."""
    try:
        with open(path) as fh:
            return int(float(fh.read().split()[0]))
    except (OSError, ValueError, IndexError):
        return None


def vitals(meminfo="/proc/meminfo", root="/", uptime_path="/proc/uptime"):
    """The server's own numbers: memory, swap and disk in MB and GB, the load, how long it has been up."""
    info = {}
    try:
        with open(meminfo) as fh:
            for line in fh:
                key, _, value = line.partition(":")
                if value.split():
                    info[key] = int(value.split()[0]) // 1024
    except (OSError, ValueError):
        pass
    disk = shutil.disk_usage(root)
    load = os.getloadavg()
    return {"memory_mb": info.get("MemTotal"), "available_mb": info.get("MemAvailable"),
            "swap_mb": info.get("SwapTotal"), "swap_used_mb": info.get("SwapTotal", 0) - info.get("SwapFree", 0),
            "disk_gb": round(disk.total / 1e9, 1), "disk_free_gb": round(disk.free / 1e9, 1),
            "load": [round(load[0], 2), round(load[1], 2)], "uptime": uptime(uptime_path)}


def health():
    """Each place of this server: its door answers, its Hermes runs, its version, its last backup."""
    seats = {}
    for number in range(1, MAX_SEATS + 1):
        user = account(number)
        try:
            entry = pwd.getpwnam(user)
        except KeyError:
            continue
        seats[str(number)] = {"up": door_open(442 + number), "hermes": running(entry.pw_uid, ("hermes", "gateway")),
                              "asleep": os.path.exists(os.path.join(entry.pw_dir, ASLEEP)),
                              "version": version(entry.pw_dir), "backup": last_backup(user),
                              "locked": locked(entry.pw_dir)}
    return seats


# -- memory: each place's soft ceiling, compressed swap, and what each place holds ------------------------------------
#
# A place is one account; systemd keeps all its processes in user-<uid>.slice (its Hermes, its bridge, its rootless
# podman containers, the agents' browsers among them). Its MemoryHigh (never MemoryMax: nothing is ever killed or
# refused) is a share of the server's memory, 35 % unless the guichet sends another (`memory_high_share` in the beat's
# answer): above it the kernel reclaims that place's own memory first (into zram) and slows its allocations, so a place
# opening four or five browsers at once slows itself, not its neighbours. Set with systemctl set-property: a drop-in
# in /etc/systemd/system.control that survives reboots.

CGROUP_USERS = "/sys/fs/cgroup/user.slice"
SETTINGS = "/var/lib/bipagents/settings.json"   # what the guichet's beat answer set (`settings`)
SAMPLES = "/run/bipagents/memory.json"           # each place's highest memory since the last beat (--sample)
ZRAM_TRIED = "/var/lib/bipagents/zram-tried"
MEMORY_HIGH_SHARE = 0.35
MB = 1024 * 1024
# The same text as user-data.yaml's (installer/tests/test_cloud_init.py): half the memory, zstd, before the disk swap.
ZRAM_CONF = "[zram0]\nzram-size = ram / 2\ncompression-algorithm = zstd\nswap-priority = 100\n"
SAMPLER_SERVICE = """[Unit]
Description=BipAgents: each place's memory, sampled between the guichet's beats

[Service]
Type=oneshot
ExecStart=/usr/bin/python3 %s/seat-agent.py --sample
"""
SAMPLER_TIMER = """[Unit]
Description=BipAgents: each place's memory every 15 s

[Timer]
OnBootSec=30s
OnUnitActiveSec=15s
AccuracySec=1s

[Install]
WantedBy=timers.target
"""


def read_json(path):
    try:
        with open(path) as fh:
            value = json.load(fh)
        return value if isinstance(value, dict) else {}
    except (OSError, ValueError):
        return {}


def write_json(path, value):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path + ".tmp", "w") as fh:
        json.dump(value, fh)
    os.replace(path + ".tmp", path)


def good_share(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and 0.1 <= value <= 1.0


def keep_settings(answer, path=None):
    """The settings in the guichet's answer, kept when they are ones this agent knows and within reason."""
    settings = answer.get("settings") if isinstance(answer, dict) else None
    if not isinstance(settings, dict) or not good_share(settings.get("memory_high_share")):
        return
    path = path or SETTINGS
    kept = read_json(path)
    if kept.get("memory_high_share") != float(settings["memory_high_share"]):
        kept["memory_high_share"] = float(settings["memory_high_share"])
        write_json(path, kept)


def write_credit(path, value, uid, gid, home):
    """`value` written at `path` for the place's account: never through a link, never outside its home."""
    folder = os.path.dirname(path)
    if not os.path.isdir(folder):
        os.makedirs(folder, mode=0o755)
        os.chown(folder, uid, gid)
    real_home = os.path.realpath(home)
    if not os.path.realpath(folder).startswith(real_home + os.sep):
        raise OSError("the credit's folder leads out of the place's home")
    tmp = path + ".tmp"
    try:
        os.unlink(tmp)
    except FileNotFoundError:
        pass
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o644)
    with os.fdopen(fd, "w") as fh:
        json.dump(value, fh)
        os.fchown(fh.fileno(), uid, gid)
    os.replace(tmp, path)


def keep_credit(answer, homes=None):
    """Each place's AI credit state in the guichet's answer, written where its bridge reads it (credit.rs); a place the
    answer leaves out (no running subscription) has its file removed: ok. An older guichet's answer changes nothing.
    `homes`: number -> (home, uid, gid), the places' accounts by default."""
    credit = answer.get("credit") if isinstance(answer, dict) else None
    if not isinstance(credit, dict):
        return
    if homes is None:
        homes = {}
        for number in range(1, MAX_SEATS + 1):
            try:
                entry = pwd.getpwnam(account(number))
            except KeyError:
                continue
            homes[number] = (entry.pw_dir, entry.pw_uid, entry.pw_gid)
    for number, (home, uid, gid) in sorted(homes.items()):
        path = os.path.join(home, CREDIT)
        given = credit.get(str(number))
        try:
            if not isinstance(given, dict) or given.get("state") not in CREDIT_STATES:
                if os.path.lexists(path):
                    os.unlink(path)
                continue
            value = {"state": given["state"]}
            resumes = given.get("resumes")
            if isinstance(resumes, (int, float)) and not isinstance(resumes, bool):
                value["resumes"] = float(resumes)
            if not os.path.islink(path) and read_json(path) == value:
                continue
            write_credit(path, value, uid, gid, home)
        except OSError as exc:
            log("place %d: credit state not written: %s" % (number, exc))


def places():
    """Each place of this server: its number (as text) and its account's uid."""
    found = {}
    for number in range(1, MAX_SEATS + 1):
        try:
            found[str(number)] = pwd.getpwnam(account(number)).pw_uid
        except KeyError:
            continue
    return found


def slice_file(uid, name, root=CGROUP_USERS):
    """A number of the place's slice (memory.current...), else None (its services not started, "max")."""
    try:
        with open(os.path.join(root, "user-%d.slice" % uid, name)) as fh:
            value = fh.read().split()[0]
        return None if value == "max" else int(value)
    except (OSError, ValueError, IndexError):
        return None


def ceiling_mb(memory_mb, settings):
    share = settings.get("memory_high_share")
    return int(memory_mb * (share if good_share(share) else MEMORY_HIGH_SHARE))


def ceiling(memory_mb, settings, root=CGROUP_USERS, setter=None):
    """Each running place's slice gets its MemoryHigh when it has another one (a place started, the share changed)."""
    if not memory_mb:
        return
    high = ceiling_mb(memory_mb, settings)
    for number, uid in places().items():
        if not os.path.isdir(os.path.join(root, "user-%d.slice" % uid)):
            continue
        if slice_file(uid, "memory.high", root) == high * MB:
            continue
        log("place %s: memory ceiling %d MB" % (number, high))
        (setter or run)("systemctl", "set-property", "user-%d.slice" % uid, "MemoryHigh=%dM" % high)


def sample(root=CGROUP_USERS, path=SAMPLES):
    """--sample (every 15 s, bipagents-memory.timer): each place's highest memory.current since the last beat."""
    samples = read_json(path)
    for number, uid in places().items():
        current = slice_file(uid, "memory.current", root)
        if current is None:
            continue
        entry = samples.get(number) if isinstance(samples.get(number), dict) else {}
        entry["max"] = max(int(entry.get("max") or 0), current)
        samples[number] = entry
    write_json(path, samples)
    return 0


def places_memory(root=CGROUP_USERS, path=SAMPLES):
    """For the beat: each place's memory now and its peak since the last beat, in MB. The peak is the highest of the
    samples taken between beats, now, and the kernel's memory.peak when it rose since the last beat (it never goes
    down: a new high since then is exact)."""
    samples = read_json(path)
    out, kept = {}, {}
    for number, uid in places().items():
        current = slice_file(uid, "memory.current", root) or 0
        entry = samples.get(number) if isinstance(samples.get(number), dict) else {}
        peak = max(current, int(entry.get("max") or 0))
        lifetime, before = slice_file(uid, "memory.peak", root), entry.get("lifetime")
        if lifetime is not None and isinstance(before, int) and lifetime > before:
            peak = max(peak, lifetime)
        out[number] = {"mem_mb": current // MB, "mem_peak_mb": peak // MB}
        kept[number] = {"max": 0, "lifetime": lifetime}
    try:
        write_json(path, kept)
    except OSError:
        pass
    return out


BUSY = ".local/state/bipagents/busy"                 # a run goes on (bridge-rs/src/runs.rs, rewritten every minute)
BUSY_FRESH = 180
BRIDGE_DATA = ".local/share/hermes-bridge"           # its bridge's data_dir: time.json, sleep.json
HERMES_HOME = ".hermes"


def iso(seconds):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(seconds))


def next_routine(home):
    """The next run of the place's routines (Hermes' cron jobs of every agent), as Unix seconds, else None."""
    import glob
    from datetime import datetime, timezone
    soonest = None
    files = glob.glob(os.path.join(home, HERMES_HOME, "profiles", "*", "cron", "jobs.json"))
    for path in files + [os.path.join(home, HERMES_HOME, "cron", "jobs.json")]:
        try:
            with open(path) as fh:
                data = json.load(fh)
        except (OSError, ValueError):
            continue
        jobs = data if isinstance(data, list) else data.get("jobs") if isinstance(data, dict) else None
        for job in jobs if isinstance(jobs, list) else []:
            if not isinstance(job, dict) or job.get("enabled") is False \
                    or job.get("state") in ("paused", "completed", "deleted"):
                continue
            try:
                at = datetime.fromisoformat(str(job.get("next_run_at")).replace("Z", "+00:00"))
            except ValueError:
                continue
            seconds = (at if at.tzinfo else at.replace(tzinfo=timezone.utc)).timestamp()
            soonest = seconds if soonest is None else min(soonest, seconds)
    return soonest


def activity(home, now=None):
    """What the place's bridge says of it in its files (nothing asked of it): `busy` (a turn, a background task or a
    Live call going on), `tz` (the person's zone the apps gave), `last_active_at` (its last request from an app, to ten
    minutes), `next_routine_at` (its routines' next run). What is not known is left out."""
    now = time.time() if now is None else now
    out = {}
    try:
        out["busy"] = now - os.stat(os.path.join(home, BUSY)).st_mtime < BUSY_FRESH
    except OSError:
        out["busy"] = False
    person = read_json(os.path.join(home, BRIDGE_DATA, "time.json"))
    zone = person.get("time_zone") or person.get("device")
    if isinstance(zone, str) and 0 < len(zone) <= 64 and all(c.isalnum() or c in "/_+-" for c in zone):
        out["tz"] = zone
    last = read_json(os.path.join(home, BRIDGE_DATA, "sleep.json")).get("last")
    if isinstance(last, (int, float)) and not isinstance(last, bool) and last > 0:
        out["last_active_at"] = iso(last)
    soonest = next_routine(home)
    if soonest is not None:
        out["next_routine_at"] = iso(soonest)
    return out


def places_activity(memory):
    """The beat's places: each one's memory and what its bridge's files say."""
    for number, entry in memory.items():
        try:
            home = pwd.getpwnam(account(int(number))).pw_dir
        except KeyError:
            continue
        try:
            entry.update(activity(home))
        except (OSError, ValueError, TypeError, AttributeError):
            pass
    return memory


def sampler(lib=LIB, units="/etc/systemd/system", runner=None):
    """bipagents-memory.timer, set up once (servers installed before 2026-10-26 included)."""
    wanted = {"bipagents-memory.service": SAMPLER_SERVICE % lib, "bipagents-memory.timer": SAMPLER_TIMER}
    changed = False
    for name, text in wanted.items():
        path = os.path.join(units, name)
        try:
            with open(path) as fh:
                if fh.read() == text:
                    continue
        except OSError:
            pass
        with open(path, "w") as fh:
            fh.write(text)
        changed = True
    if changed:
        (runner or run)("systemctl", "daemon-reload")
        (runner or run)("systemctl", "enable", "--now", "bipagents-memory.timer")


def zram_on(swaps="/proc/swaps"):
    try:
        with open(swaps) as fh:
            return any(line.startswith("/dev/zram") for line in fh)
    except OSError:
        return False


def quietly(*args):
    try:
        return subprocess.run(list(args), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=900,
                              env=dict(os.environ, DEBIAN_FRONTEND="noninteractive")).returncode
    except (OSError, subprocess.SubprocessError):
        return 1


def zram(swaps="/proc/swaps", tried=ZRAM_TRIED, conf="/etc/systemd/zram-generator.conf", runner=quietly):
    """Compressed swap in memory (zram, half the memory, ahead of the disk swap) on a server that has none yet
    (installed before 2026-10-26, or its first boot could not): as user-data.yaml does, tried again an hour later
    when it did not work (apt busy...)."""
    if zram_on(swaps):
        return True
    try:
        if time.time() - os.stat(tried).st_mtime < 3600:
            return False
    except OSError:
        pass
    write_json(tried, {"at": int(time.time())})
    # Ubuntu's cloud kernels keep zram in linux-modules-extra (DigitalOcean, 2026-10-26), for the kernel running now:
    # after a kernel update and the night's reboot it is set up again here.
    extra = "linux-modules-extra-" + os.uname().release
    loaded = runner("modprobe", "zram") == 0 or (runner("apt-get", "install", "-y", extra) == 0
                                                  and runner("modprobe", "zram") == 0)
    if not loaded or runner("apt-get", "install", "-y", "systemd-zram-generator") != 0:
        log("zram not set up yet (tried again in an hour)")
        return False
    with open(conf, "w") as fh:
        fh.write(ZRAM_CONF)
    runner("systemctl", "daemon-reload")
    runner("systemctl", "restart", "systemd-zram-setup@zram0.service")   # the device, with these settings
    runner("systemctl", "start", "dev-zram0.swap")                         # in use as swap (waited for)
    on = zram_on(swaps)
    log("zram %s" % ("on" if on else "not on yet"))
    return on


# -- planned moves (place-move.py, 2026-10-26): kept apart from the rest -----------------------------------------------

MOVE_ACTIONS = ("move_out", "move_in", "move_cancel")
MOVES = "/var/lib/bipagents/moves"
MOVE_ID_CHARS = "abcdefghijklmnopqrstuvwxyz0123456789-"


def move_valid(body):
    """A move order's own fields: its id, and for move_out / move_in the one-time key and the other server."""
    move = body.get("move")
    if not isinstance(move, dict):
        return False
    move_id = move.get("id")
    if not (isinstance(move_id, str) and 6 <= len(move_id) <= 63 and all(c in MOVE_ID_CHARS for c in move_id)
            and move_id[0] != "-"):
        return False
    if body["action"] == "move_cancel":
        return True
    key = move.get("key")
    if not (isinstance(key, str) and 43 <= len(key) <= 88 and all(c in PAIR_CHARS for c in key.rstrip("="))):
        return False
    other = move.get("target") if body["action"] == "move_out" else move.get("source")
    if not (isinstance(other, dict) and isinstance(other.get("address"), str) and _address(other["address"])):
        return False
    port = (other.get("port") if body["action"] == "move_out" else move.get("port")) or 2222
    window = move.get("window")
    return isinstance(port, int) and 1024 <= port <= 65535 and (window is None or (
        isinstance(window, dict) and all(isinstance(window.get(k), (int, float)) for k in ("start", "end"))))


def _address(text):
    import ipaddress
    try:
        ipaddress.ip_address(text)
        return True
    except ValueError:
        return False


def _place_move():
    import importlib.util
    spec = importlib.util.spec_from_file_location("place_move", os.path.join(LIB, "place-move.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def move(body, moves=MOVES, lib=LIB, run=subprocess.run):
    """The order to place-move.py: the target's listener at once, the source's move in a unit of its own (it lasts
    from minutes to hours), a cancel either side."""
    move_id = body["move"]["id"]
    script = os.path.join(lib, "place-move.py")
    if body["action"] == "move_cancel":
        run(["/usr/bin/python3", script, "cancel", move_id])
        return
    folder = os.path.join(moves, move_id)
    os.makedirs(folder, mode=0o700, exist_ok=True)
    path = os.path.join(folder, "order.json")
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as fh:
        json.dump(body, fh)
    log("place %s: %s %s" % (body["seat"], body["action"], move_id))
    if body["action"] == "move_in":
        run(["/usr/bin/python3", script, "in", path])
    else:
        run(["systemd-run", "--unit", "bipagents-move-%s" % move_id, "--collect", "--property=Nice=5",
             "/usr/bin/python3", script, "out", path])


def remove_valid(body):
    """`remove`: the place left behind by a move, named by that move's id, or marked removed by the guichet."""
    move = body.get("move")
    if isinstance(move, dict):
        return move_valid({"action": "move_cancel", "move": move})
    return body.get("removed") is True


def moved_out(number, move_id, moves=MOVES):
    """This seat's move_out with that id completed here."""
    try:
        with open(os.path.join(moves, move_id, "state.json")) as fh:
            state = json.load(fh)
    except (OSError, ValueError):
        return False
    return (state.get("role") == "out" and int(state.get("seat", 0)) == number
            and state.get("phase") in ("done", "removed"))


def remove(number, body, moves=MOVES, wipe_account=None, run=subprocess.run):
    """The place a move left on this server goes (the guichet's pool::vacate): its services and account, its door's
    firewall rule, its setup file. Only after its move_out completed here, and only while the account still holds
    that move's mark (a seat installed again since is someone else's), or when the guichet marks it removed. Done
    again, nothing more happens."""
    user = account(number)
    move = body.get("move")
    if isinstance(move, dict):
        move_id = move["id"]
        if not moved_out(number, move_id, moves):
            log("place %d: remove refused, its move %s did not complete here" % (number, move_id))
            return False
        try:
            home = pwd.getpwnam(user).pw_dir
            with open(os.path.join(home, ".config", "bipagents", "moved-away.json")) as fh:
                mark = json.load(fh).get("move")
        except KeyError:
            mark = move_id   # the account is already gone
        except (OSError, ValueError, AttributeError):
            mark = None
        if mark != move_id:
            log("place %d: remove refused, the account is not the one moved by %s" % (number, move_id))
            return False
    elif body.get("removed") is not True:
        return False
    log("place %d (%s): removed" % (number, user))
    (wipe_account or wipe)(user)
    run(["ufw", "delete", "allow", "%d/tcp" % (442 + number)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        os.unlink("/etc/bipagents/cloud%d.json" % number)
    except OSError:
        pass
    if isinstance(move, dict):
        path = os.path.join(moves, move["id"], "state.json")
        try:
            with open(path) as fh:
                state = json.load(fh)
            state["phase"] = "removed"
            with open(path, "w") as fh:
                json.dump(state, fh)
        except (OSError, ValueError):
            pass
    return True


def add_moves(beat, lib=LIB):
    """The moves' phases and numbers in the beat (`moves`), and the moves' leftovers cleared (listeners past their
    time, source copies past their seven days)."""
    if not os.path.exists(os.path.join(lib, "place-move.py")):
        return
    try:
        module = _place_move()
        module.cleanup(wipe=wipe)
        found = module.states()
    except Exception as exc:   # never the beat's loss
        log("moves not read: %s" % exc)
        return
    if found:
        beat["moves"] = found


def read_done():
    try:
        with open(DONE) as fh:
            return {int(line) for line in fh if line.strip().isdigit()}
    except OSError:
        return set()


def reboot_queued():
    """The server is going down to start again (not to stop): systemd has a job for reboot.target."""
    try:
        jobs = subprocess.run(["systemctl", "list-jobs", "--no-legend", "--no-pager"], capture_output=True,
                              text=True, timeout=10).stdout
    except (OSError, subprocess.SubprocessError):
        return False
    return any(line.split()[1:2] == ["reboot.target"] for line in jobs.splitlines())


def rebooting(conf):
    """As the server shuts down: the guichet told when it is a reboot, nothing else (it is a planned one)."""
    if not reboot_queued():
        return 0
    try:
        call(conf, "POST", "/v1/server/rebooting")
        log("the guichet knows this server is rebooting")
    except (OSError, ValueError) as exc:
        log("the guichet was not told of the reboot: %s" % exc)
    return 0


def memory_settled(server):
    """Each place's memory ceiling and the sampler between beats, whatever the guichet answers."""
    try:
        ceiling(server.get("memory_mb"), read_json(SETTINGS))
        sampler()
    except OSError as exc:
        log("memory settings not applied: %s" % exc)


def main(argv=()):
    if "--sample" in argv:
        return sample()
    if "--ceiling" in argv:   # add-seat.sh, as soon as a place's account starts
        ceiling(vitals().get("memory_mb"), read_json(SETTINGS))
        return 0
    if "--prepare" in argv:   # add-seat.sh, before a place's install: Hermes and the images of its version, shared
        at = list(argv).index("--prepare")
        commit = argv[at + 1] if len(argv) > at + 1 else "main"
        try:
            prepare_shared(commit)
        except (OSError, subprocess.SubprocessError) as exc:
            log("shared Hermes and images for %s not ready: %s" % (commit[:12], exc))
        return 0
    if "--bootstrap" in argv:   # the first-boot script, before any place is installed
        return bootstrap()
    with open(CONF) as fh:
        conf = json.load(fh)
    if not conf.get("token"):   # its first boot never got as far as its own token: nothing to tell the guichet
        log("no token for the guichet yet (first boot not finished)")
        return 0
    if "--rebooting" in argv:
        return rebooting(conf)
    metadata_block()
    server = vitals()
    memory_settled(server)
    try:
        beat = {"seats": health(), "server": server, "places": places_activity(places_memory())}
        add_moves(beat)
        answer = call(conf, "POST", "/v1/server/beat", beat)
        orders = answer.get("orders") or []
    except (OSError, ValueError) as exc:
        log("guichet unreachable: %s" % exc)
        zram()
        return 0
    try:
        keep_settings(answer)
    except OSError as exc:
        log("settings not kept: %s" % exc)
    keep_credit(answer)
    memory_settled(server)
    done = read_done()
    for order in orders:
        if int(order.get("id", 0)) in done:
            body = None
        elif not verified(str(order.get("body", "")), str(order.get("signature", ""))):
            log("order %s refused: not signed by the guichet" % order.get("id"))
            continue
        else:
            body = accepted(order, conf["server"], done, time.time())
            if body is None:
                continue
            if body["action"] == "update":
                try:
                    update(int(body["seat"]), body["commit"])
                except (OSError, subprocess.SubprocessError) as exc:
                    log("update of place %s stopped: %s" % (body["seat"], exc))
            elif body["action"] in MOVE_ACTIONS:
                try:
                    move(body)
                except (OSError, ValueError, subprocess.SubprocessError) as exc:
                    log("move order %s stopped: %s" % (order.get("id"), exc))
            elif body["action"] == "remove":
                try:
                    remove(int(body["seat"]), body)
                except OSError as exc:
                    log("removing place %s stopped: %s" % (body["seat"], exc))
            elif body["action"] == "reopen":
                try:
                    reopen(int(body["seat"]), body["pair"])
                except OSError as exc:
                    log("reopening place %s stopped: %s" % (body["seat"], exc))
            else:
                reset(int(body["seat"]), body["setup"])
            os.makedirs(os.path.dirname(DONE), exist_ok=True)
            with open(DONE, "a") as fh:
                fh.write("%d\n" % int(order["id"]))
        try:
            call(conf, "POST", "/v1/server/orders/%d/done" % int(order["id"]))
        except (OSError, ValueError) as exc:
            log("done not told yet (next round): %s" % exc)
    zram()   # last: installing it may take a minute, once
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
