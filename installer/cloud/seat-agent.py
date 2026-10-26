#!/usr/bin/env python3
"""BipAgents: the guichet's orders for the places of this server (guichet/bipguichet/orders.py).

Run as root every two minutes by bipagents-seats.timer, set up at first boot. Its call to the guichet, with the token
its first boot gave it, is the server's heartbeat (guichet/bipguichet/watch.py): it says, for each place, whether its
bridge's door answers here, whether its Hermes runs (or sleeps), the BipAgents version it runs, when its last backup worked
and whether its encrypted space waits for its key ("locked" and why),
and for the server its memory, swap, disk and load, and for each place the memory it holds now and its highest since the
last beat ("places") -- yes or no, a version, a time and numbers, nothing of anyone. The
guichet answers with its orders, and the agent does one only when the guichet's Ed25519 key signed it (checked with
openssl against /etc/bipagents/guichet.pub), for this server, not expired, not done before. There are three orders:
install place n again for its next person -- its account bip<n> (bip for place 1), everything in it and its
services go, then installer/cloud/add-seat.sh sets it up again with the new pairing secret; bring place n to a
given BipAgents version (the guichet's update waves, or their way back), which its own updater does; and let place n
pair again with a new secret (its person recovered their account without any of their devices). The other places
are not touched, and nothing is ever read or sent back: only "done".

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
import json
import os
import pwd
import shutil
import socket
import subprocess
import sys
import tempfile
import time
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


def update(number, commit):
    """Place n brought to BipAgents `commit` by its own updater, as its account (what changed restarts)."""
    user = account(number)
    try:
        uid = pwd.getpwnam(user).pw_uid
    except KeyError:
        return
    log("place %d (%s): BipAgents %s" % (number, user, commit[:12]))
    with open("/var/log/bipagents-update%d.log" % number, "a") as out:
        subprocess.run(["sudo", "-iu", user, "env", "XDG_RUNTIME_DIR=/run/user/%d" % uid,
                        "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/%d/bus" % uid, "BIPAGENTS_TO=" + commit,
                        "bash", "BipAgents/install.sh", "update", "--auto"], stdout=out, stderr=out, timeout=3600)


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
    if runner("modprobe", "zram") != 0 or runner("apt-get", "install", "-y", "systemd-zram-generator") != 0:
        log("zram not set up yet (tried again in an hour)")
        return False
    with open(conf, "w") as fh:
        fh.write(ZRAM_CONF)
    runner("systemctl", "daemon-reload")
    runner("systemctl", "restart", "systemd-zram-setup@zram0.service")
    on = zram_on(swaps)
    log("zram %s" % ("on" if on else "not on yet"))
    return on


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
    with open(CONF) as fh:
        conf = json.load(fh)
    if "--rebooting" in argv:
        return rebooting(conf)
    server = vitals()
    memory_settled(server)
    try:
        answer = call(conf, "POST", "/v1/server/beat", {"seats": health(), "server": server,
                                                         "places": places_memory()})
        orders = answer.get("orders") or []
    except (OSError, ValueError) as exc:
        log("guichet unreachable: %s" % exc)
        zram()
        return 0
    try:
        keep_settings(answer)
    except OSError as exc:
        log("settings not kept: %s" % exc)
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
