#!/usr/bin/env python3
"""BipAgents: the guichet's orders for the places of this server (guichet/bipguichet/orders.py).

Run as root every two minutes by bipagents-seats.timer, set up at first boot. It asks the guichet for its orders with
the token its first boot gave it, and does an order only when the guichet's Ed25519 key signed it (checked with
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


def call(conf, method, path):
    request = urllib.request.Request(conf["url"].rstrip("/") + path, method=method, data=b"" if method == "POST" else None,
                                     headers={"Authorization": "Bearer " + conf["token"]})
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.loads(response.read().decode("utf-8") or "{}")


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
        orders = call(conf, "GET", "/v1/server/orders").get("orders") or []
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
