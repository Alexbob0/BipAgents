#!/usr/bin/env python3
"""BipAgents: the guichet's orders for the places of this server (guichet/bipguichet/orders.py).

Run as root every two minutes by bipagents-seats.timer, set up at first boot. Its call to the guichet, with the token
its first boot gave it, is the server's heartbeat (guichet/bipguichet/watch.py): it says, for each place, whether its
bridge's door answers here, whether its Hermes runs, the BipAgents version it runs and when its last backup worked,
and for the server its memory, swap, disk and load -- yes or no, a version, a time and numbers, nothing of anyone. The
guichet answers with its orders, and the agent does one only when the guichet's Ed25519 key signed it (checked with
openssl against /etc/bipagents/guichet.pub), for this server, not expired, not done before. The one order there is:
install place n again for its next person -- its account bip<n> (bip for place 1), everything in it and its
services go, then installer/cloud/add-seat.sh sets it up again with the new pairing secret. The other places are
not touched, and nothing is ever read or sent back: only "done".

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
        setup = body["setup"]
        ok = (body.get("action") == "reset" and body.get("server") == server and int(body["order"]) == int(order["id"])
              and 1 <= number <= MAX_SEATS and int(body["expires"]) > now and isinstance(setup, dict)
              and isinstance(setup.get("pair"), str) and int(setup.get("port", 0)) == 442 + number)
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


def vitals(meminfo="/proc/meminfo", root="/"):
    """The server's own numbers: memory, swap and disk in MB and GB, the load."""
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
            "load": [round(load[0], 2), round(load[1], 2)]}


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
                              "version": version(entry.pw_dir), "backup": last_backup(user)}
    return seats


def read_done():
    try:
        with open(DONE) as fh:
            return {int(line) for line in fh if line.strip().isdigit()}
    except OSError:
        return set()


def main():
    with open(CONF) as fh:
        conf = json.load(fh)
    try:
        orders = call(conf, "POST", "/v1/server/beat", {"seats": health(), "server": vitals()}).get("orders") or []
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
    sys.exit(main())
