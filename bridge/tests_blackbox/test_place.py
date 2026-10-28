"""A place of the all-included offer (`cloud = true`): the person's SSH keys (`/v1/ssh`), its backups' key and state
(`/v1/backup`), one's own Hermes sent in (`/v1/import…`), a place nobody uses put to sleep and woken by the next
request; and the site icons kept by the bridge (`/v1/favicon`). tests/test_ssh_keys.py, test_place_backup.py,
test_place_import.py, test_ai_transparency.py and test_sleep.py, over HTTP; HOME is the bridge's own folder."""
import base64
import hashlib
import hmac
import json
import os
import pwd
import secrets
import ssl
import threading
import time

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import ec, ed25519

from bbkit.common import AUTH, REMOTE
from bbkit.servers import free_port, wait_until

ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
IMPORT_CONTEXT = "bipagents-import-v1"
UPLOAD_CONTEXT = "bipagents-import-upload-v1"


def cloud(bridges, lan=False, start=True):
    def change(c):
        c.update(cloud=True, public_url="https://203.0.113.7:8701")
        if lan:
            c["lan"] = {"enabled": True, "address": "203.0.113.7", "host": "127.0.0.1", "port": free_port()}
    bridge = bridges.new(change=change)
    return bridge.start() if start else bridge


# -- SSH keys --------------------------------------------------------------------------------------------------------

def openssh(public_key, comment):
    line = public_key.public_bytes(serialization.Encoding.OpenSSH, serialization.PublicFormat.OpenSSH).decode()
    blob = base64.b64decode(line.split()[1])
    return f"{line} {comment}", "SHA256:" + base64.b64encode(hashlib.sha256(blob).digest()).decode().rstrip("=")


def test_the_person_adds_lists_and_removes_their_keys_on_a_server_of_the_offer(bridges):
    own = bridges.new("own").start()
    assert own.get("/v1/ssh", headers=AUTH).json() == {"enabled": False, "keys": []}   # someone's own computer
    one, one_print = openssh(ed25519.Ed25519PrivateKey.generate().public_key(), "one@laptop")
    refused = own.post("/v1/ssh/keys", json={"key": one}, headers=AUTH)
    assert refused.status_code == 403 and refused.json() == {"detail": "only on a server of the all-included offer"}
    assert own.delete("/v1/ssh/keys", params={"fingerprint": one_print}, headers=AUTH).status_code == 403
    bridge = cloud(bridges, start=False)
    (bridge.home / ".ssh").mkdir(mode=0o755)
    (bridge.home / ".ssh" / "authorized_keys").write_text("# kept\n")
    bridge.start()
    assert bridge.get("/v1/ssh").status_code == 401
    added = bridge.post("/v1/ssh/keys", json={"key": one}, headers=AUTH).json()
    assert added == {"fingerprint": one_print, "type": "ssh-ed25519", "comment": "one@laptop"}
    two, two_print = openssh(ec.generate_private_key(ec.SECP256R1()).public_key(), "two@laptop")
    bridge.post("/v1/ssh/keys", json={"key": two + "\n"}, headers=AUTH)
    assert bridge.post("/v1/ssh/keys", json={"key": one}, headers=AUTH).json()["fingerprint"] == one_print   # once only
    status = bridge.get("/v1/ssh", headers=AUTH).json()
    assert status["enabled"] and status["login"] == f"ssh {pwd.getpwuid(os.getuid()).pw_name}@203.0.113.7"
    assert [(k["type"], k["comment"], k["fingerprint"]) for k in status["keys"]] == [
        ("ssh-ed25519", "one@laptop", one_print), ("ecdsa-sha2-nistp256", "two@laptop", two_print)]
    text = (bridge.home / ".ssh" / "authorized_keys").read_text()
    assert text.startswith("# kept\n") and text.count("\n") == 3
    assert oct((bridge.home / ".ssh").stat().st_mode & 0o777) == "0o700"
    assert oct((bridge.home / ".ssh" / "authorized_keys").stat().st_mode & 0o777) == "0o600"
    unnamed, _ = openssh(ed25519.Ed25519PrivateKey.generate().public_key(), "")
    assert bridge.post("/v1/ssh/keys", json={"key": unnamed.strip()}, headers=AUTH).json()["comment"] == "added from BipAgents"
    gone = bridge.delete("/v1/ssh/keys", params={"fingerprint": two_print}, headers=AUTH)
    assert gone.status_code == 200 and gone.json() == {"ok": True}
    assert "two@laptop" not in (bridge.home / ".ssh" / "authorized_keys").read_text()
    again = bridge.delete("/v1/ssh/keys", params={"fingerprint": two_print}, headers=AUTH)
    assert again.status_code == 404 and again.json() == {"detail": "no such key"}
    assert bridge.delete("/v1/ssh/keys", headers=AUTH).status_code == 422


def test_only_a_plain_public_key_goes_in(bridges):
    bridge = cloud(bridges)
    good = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl me"
    from cryptography.hazmat.primitives.asymmetric import rsa
    rsa_blob = openssh(rsa.generate_private_key(public_exponent=65537, key_size=2048).public_key(), "")[0].split()[1]
    assert bridge.post("/v1/ssh/keys", json={"key": good}, headers=AUTH).json()["type"] == "ssh-ed25519"
    for bad, detail in (('command="rm -rf ~" ' + good, "not a public SSH key (ssh-ed25519 AAAA… name)"),
                        ("ssh-dss AAAAB3NzaC1kc3MAAACBAP me", "not a public SSH key (ssh-ed25519 AAAA… name)"),
                        ("ssh-ed25519 " + rsa_blob + " me", "not a public SSH key (its type and its content differ)"),
                        ("ssh-ed25519 AAAAB3NzaC1yc2EAAAADAQABAAABAQC me", "not a public SSH key (its middle part)"),
                        ("ssh-ed25519 not-base64!-and-long-enough", "not a public SSH key (ssh-ed25519 AAAA… name)"),
                        (good + "\nssh-rsa AAAA second", "one public key at a time")):
        resp = bridge.post("/v1/ssh/keys", json={"key": bad}, headers=AUTH)
        assert resp.status_code == 400 and resp.json() == {"detail": detail}, bad
    assert bridge.post("/v1/ssh/keys", json={"key": "short"}, headers=AUTH).status_code == 422


# -- backups ----------------------------------------------------------------------------------------------------------

def test_the_app_gets_the_key_and_when_the_last_backup_worked(bridge):
    assert bridge.get("/v1/backup", headers=AUTH).json() == {"enabled": False}
    assert bridge.get("/v1/backup").status_code == 401
    assert bridge.post("/v1/backup/now", headers=AUTH).json() == {"started": False}
    settings = bridge.home / ".config" / "bipagents" / "backup.json"
    settings.parent.mkdir(parents=True)
    settings.write_text(json.dumps({"url": "https://g.test/v1/backup/", "token": "t", "password": "the-key"}))
    five_gb = 5 * 2**30
    assert bridge.get("/v1/backup", headers=AUTH).json() == {"enabled": True, "key": "the-key", "allowance_bytes": five_gb}
    state = bridge.home / ".local" / "state" / "bipagents" / "backup.json"
    state.parent.mkdir(parents=True)
    state.write_text(json.dumps({"last_ok": 1790000000.0, "snapshot": "abc", "last_error": None,
                                 "failed_at": 1789990000.0}))
    status = bridge.get("/v1/backup", headers=AUTH).json()
    assert status == {"enabled": True, "key": "the-key", "last_ok": 1790000000.0, "failed_at": 1789990000.0,
                      "allowance_bytes": five_gb}
    started = bridge.post("/v1/backup/now", headers=AUTH)
    assert started.json() == {"started": True}
    assert bridge.commands.of("systemctl") == [["--user", "start", "--no-block", "bipagents-backup.service"]]
    bridge.commands.rule("systemctl", ["--user", "start"], exit=1)
    assert bridge.post("/v1/backup/now", headers=AUTH).json() == {"started": False}


def test_a_place_with_its_5_gb_used_up_refuses_files_and_notes(bridge):
    settings = bridge.home / ".config" / "bipagents" / "backup.json"
    settings.parent.mkdir(parents=True)
    settings.write_text(json.dumps({"url": "https://g.test/v1/backup/", "token": "t", "password": "the-key"}))
    state = bridge.home / ".local" / "state" / "bipagents" / "backup.json"
    state.parent.mkdir(parents=True)
    state.write_text(json.dumps({"last_ok": 1790000000.0, "bytes": 1_288_490_189}))
    assert bridge.get("/v1/backup", headers=AUTH).json()["used_bytes"] == 1_288_490_189
    state.write_text(json.dumps({"last_ok": 1790000000.0, "bytes": 5 * 2**30}))
    full = bridge.post("/v1/files", headers=AUTH, data={"agent": "wellness"},
                       files={"file": ("a.pdf", b"%PDF-1.4", "application/pdf")})
    assert full.status_code == 507 and full.json() == {"detail": "no room left: the 5 GB of your agents' space are used up"}


# -- one's own Hermes, sent in -----------------------------------------------------------------------------------------

def mac(code, *parts):
    return hmac.new(code.encode(), "\n".join(parts).encode(), hashlib.sha256).hexdigest()


def test_a_computer_proves_the_code_then_sends_its_export_once(bridges):
    own = bridges.new("own").start()
    assert own.post("/v1/import", headers=AUTH).status_code == 404               # someone's own computer
    assert own.get("/v1/import/hello", params={"nonce": secrets.token_hex(16)}).status_code == 404
    bridge = cloud(bridges, lan=True)
    pem = (bridge.data / "lan" / "bridge-lan.pem").read_text()
    fingerprint = hashlib.sha256(ssl.PEM_cert_to_DER_cert(pem)).hexdigest()
    assert bridge.get("/v1/import", headers=AUTH).json() == {"state": "none"}
    assert bridge.get("/v1/import/hello", params={"nonce": secrets.token_hex(16)}).status_code == 404   # no code yet
    assert bridge.post("/v1/import").status_code == 401
    given = bridge.post("/v1/import", headers=AUTH).json()
    assert given["state"] == "waiting" and given["address"] == "203.0.113.7:8701"
    assert given["command"] == f"bipagents send 203.0.113.7:8701 {given['code']}"
    assert given["command_alone"] == ("curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh "
                                      f"| bash -s -- send 203.0.113.7:8701 {given['code']}")
    assert abs(given["expires"] - (time.time() + 3600)) < 60
    code = given["code"].replace("-", "")
    assert len(code) == 16 and set(code) <= set(ALPHABET) and len(given["code"]) == 19
    folder = bridge.home / ".local" / "state" / "bipagents" / "import"
    assert json.loads((folder / "code.json").read_text())["code"] == code
    assert oct((folder / "code.json").stat().st_mode & 0o777) == "0o600"

    nonce = secrets.token_hex(16)
    hello = bridge.get("/v1/import/hello", params={"nonce": nonce}).json()
    assert hello["fingerprint"] == fingerprint
    assert hello["proof"] == mac(code, IMPORT_CONTEXT, nonce, hello["fingerprint"], hello["challenge"])

    body = b"sealed export " * 1000
    wrong = bridge.put("/v1/import/archive", content=body, headers={
        "X-Import-Challenge": hello["challenge"], "X-Import-Proof": mac("0" * 16, UPLOAD_CONTEXT,
                                                                        hello["challenge"], str(len(body)))})
    assert wrong.status_code == 401 and wrong.json() == {"detail": "wrong proof"} and not bridge.commands.of("systemd-run")
    challenge = bridge.get("/v1/import/hello", params={"nonce": nonce}).json()["challenge"]
    sent = bridge.put("/v1/import/archive", content=body, headers={
        "X-Import-Challenge": challenge, "X-Import-Proof": mac(code, UPLOAD_CONTEXT, challenge, str(len(body)))})
    assert sent.json() == {"started": True}
    archive, secret = folder / "archive.bipagents", folder / "passphrase"
    assert archive.read_bytes() == body and secret.read_text() == code
    assert oct(archive.stat().st_mode & 0o777) == "0o600" and oct(secret.stat().st_mode & 0o777) == "0o600"
    assert wait_until(lambda: bridge.commands.of("systemd-run"))
    run = bridge.commands.of("systemd-run")[0]
    assert run[:4] == ["--user", "--unit", "bipagents-import", "--collect"]
    assert run[-7:] == ["-m", "bipinstall", "import-place", "--archive", str(archive), "--passphrase-file", str(secret)]
    status = bridge.get("/v1/import", headers=AUTH).json()
    assert status == {"state": "importing", "step": 0, "total": 4, "label": "Received"}
    # Used: no second sending with it, and nothing to say hello to.
    assert bridge.get("/v1/import/hello", params={"nonce": nonce}).status_code == 404
    again = bridge.put("/v1/import/archive", content=body, headers={
        "X-Import-Challenge": challenge, "X-Import-Proof": mac(code, UPLOAD_CONTEXT, challenge, str(len(body)))})
    assert again.status_code == 403 and again.json() == {"detail": "no code waits: ask the app for a new one"}
    busy = bridge.post("/v1/import", headers=AUTH)
    assert busy.status_code == 409 and busy.json() == {"detail": "an export is being received or put in"}
    assert bridge.delete("/v1/import", headers=AUTH).status_code == 409

    (folder / "progress.json").write_text(json.dumps({"step": 4, "total": 4, "label": "Ready", "done": True, "at": 1}))
    assert bridge.get("/v1/import", headers=AUTH).json() == {"state": "done", "step": 4, "total": 4, "label": "Ready"}
    assert bridge.delete("/v1/import", headers=AUTH).status_code == 204
    assert bridge.get("/v1/import", headers=AUTH).json() == {"state": "none"}
    (folder / "progress.json").write_text(json.dumps({"step": 2, "total": 4, "label": "Agents", "at": time.time() - 3 * 3600}))
    assert bridge.get("/v1/import", headers=AUTH).json() == {"state": "failed", "error": "the import stopped half-way",
                                                             "step": 2, "total": 4, "label": "Agents"}


def test_an_archive_needs_its_size_and_wrong_proofs_count(bridges):
    bridge = cloud(bridges, lan=True)
    bridge.post("/v1/import", headers=AUTH)
    remote = {"X-Forwarded-For": REMOTE}

    def chunks():
        yield b"x" * 10

    unsized = bridge.put("/v1/import/archive", content=chunks(), headers={"X-Import-Challenge": "c", "X-Import-Proof": "p"})
    assert unsized.status_code == 411 and unsized.json() == {"detail": "the export's size is needed"}
    for _ in range(20):
        bridge.put("/v1/import/archive", content=b"x", headers={**remote, "X-Import-Challenge": "c", "X-Import-Proof": "p"})
    blocked = bridge.get("/v1/import/hello", params={"nonce": "ab" * 16}, headers=remote)
    assert blocked.status_code == 429 and blocked.json() == {"detail": "too many wrong proofs, try again later"}
    assert bridge.logged("wrong key")[0]["what"] == "import proof"


# -- sleep ----------------------------------------------------------------------------------------------------------

def test_a_sleeping_place_wakes_its_hermes_for_the_next_request(bridges, fakes):
    bridge = cloud(bridges, start=False)
    bridge.data.mkdir(parents=True)
    (bridge.data / "sleep.json").write_text(json.dumps({"last": time.time() - 20 * 86400, "asleep": True}))
    marker = bridge.home / ".local" / "state" / "bipagents" / "asleep"
    marker.parent.mkdir(parents=True)
    marker.write_text("1")
    bridge.commands.rule("systemctl", ["--user", "list-units"],
                         stdout="hermes-gateway.service loaded active running Hermes Agent gateway\n"
                                "bipagents-bridge.service loaded active running BipAgents bridge\n")
    fakes.hermes.capabilities[""] = 502            # Hermes is stopped
    bridge.start()
    assert bridge.get("/health").status_code == 200 and not bridge.commands.of("systemctl")   # no key: nothing woken
    threading.Timer(1.5, lambda: fakes.hermes.capabilities.update({"": 200})).start()
    started = time.monotonic()
    resp = bridge.get("/v1/agents", headers=AUTH)
    assert resp.status_code == 200 and time.monotonic() - started >= 1.0   # it waited for Hermes
    assert bridge.commands.of("systemctl") == [["--user", "list-units", "--all", "--type=service", "--plain", "--no-legend"],
                                               ["--user", "start", "hermes-gateway.service"]]
    assert not marker.exists()
    assert json.loads((bridge.data / "sleep.json").read_text())["asleep"] is False
    assert bridge.logged("hermes woken")


def test_an_own_computer_never_sleeps(bridge):
    assert bridge.get("/v1/agents", headers=AUTH).status_code == 200
    assert not (bridge.data / "sleep.json").exists() and bridge.commands.of("systemctl") == []


# -- site icons -------------------------------------------------------------------------------------------------------

def test_site_icons_come_from_the_bridges_cache(bridges):
    """favicons.py keeps each icon 30 days (`<data>/favicons/<host>.icon`) and a site without one a day
    (`<host>.missing`); kept files are served without asking DuckDuckGo again."""
    bridge = bridges.new()
    cache = bridge.data / "favicons"
    cache.mkdir(parents=True)
    (cache / "example.com.icon").write_bytes(b"\x89PNG\r\n\x1a\nicon")
    (cache / "gif.example.icon").write_bytes(b"GIF89a...")
    (cache / "nothing.example.missing").touch()
    bridge.start()
    resp = bridge.get("/v1/favicon/Example.com", headers=AUTH)
    assert resp.status_code == 200 and resp.content == b"\x89PNG\r\n\x1a\nicon"
    assert resp.headers["content-type"] == "image/png" and resp.headers["cache-control"] == "private, max-age=86400"
    assert bridge.get("/v1/favicon/example.com.", headers=AUTH).status_code == 200
    assert bridge.get("/v1/favicon/gif.example", headers=AUTH).headers["content-type"] == "image/gif"
    missing = bridge.get("/v1/favicon/nothing.example", headers=AUTH)
    assert missing.status_code == 404 and missing.json() == {"detail": "no icon"}
    # Only site names, and only for the app.
    for bad in ("127.0.0.1", "localhost", "a..b", "x" * 300 + ".com", "under_score.com"):
        refused = bridge.get(f"/v1/favicon/{bad}", headers=AUTH)
        assert refused.status_code == 400 and refused.json() == {"detail": "not a site name"}, bad
    assert bridge.get("/v1/favicon/example.com").status_code == 401
