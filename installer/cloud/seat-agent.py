#!/usr/bin/env python3
"""BipAgents: the guichet's orders for the places of this server (guichet/bipguichet/orders.py).

Run as root every two minutes by bipagents-seats.timer, set up at first boot. Its call to the guichet, with the token
its first boot gave it, is the server's heartbeat (guichet/bipguichet/watch.py): it says, for each place, whether its
bridge's door answers here, whether its Hermes runs (or sleeps), the BipAgents version it runs, when its last backup worked
and whether its encrypted space waits for its key ("locked" and why),
and for the server its memory, swap, disk and load -- yes or no, a version, a time and numbers, nothing of anyone. The
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


def main(argv=()):
    with open(CONF) as fh:
        conf = json.load(fh)
    if "--rebooting" in argv:
        return rebooting(conf)
    try:
        beat = {"seats": health(), "server": vitals()}
        add_moves(beat)
        orders = call(conf, "POST", "/v1/server/beat", beat).get("orders") or []
    except (OSError, ValueError) as exc:
        log("guichet unreachable: %s" % exc)
        return 0
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
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
