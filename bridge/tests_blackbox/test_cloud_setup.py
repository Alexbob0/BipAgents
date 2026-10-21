"""« No computer »: a server of the all-included offer proves its certificate, is claimed once by the app (model set,
agents created with the fake `hermes`), refuses wrong proofs, restores a moved place, and reopens after an account
recovery (tests/test_cloud_setup.py, over HTTP; the claim runs for real against the fakes)."""
import base64
import hashlib
import hmac
import json
import os
import ssl
import stat
import tomllib

import httpx

from bbkit.common import AUTH, BRIDGE_KEY, REMOTE
from bbkit.servers import free_port, wait_until

SECRET = bytes(range(32))
NONCE = "ab" * 16
SETUP_CONTEXT = "bipagents-setup-v1"
CLAIM_CONTEXT = "bipagents-claim-v1"


def proof(secret, *parts):
    """HMAC-SHA256 of the parts, one per line, lowercase hex (cloud_setup.proof; the apps compute the same)."""
    return hmac.new(secret, "\n".join(parts).encode("utf-8"), hashlib.sha256).hexdigest()


def b64(secret):
    return base64.urlsafe_b64encode(secret).decode().rstrip("=")


class Cloud:
    def __init__(self, bridge):
        self.bridge = bridge
        self.setup_file = bridge.root / "setup" / "setup.json"
        self.progress = bridge.root / "setup" / "progress.json"

    def done(self, flag=True, **extra):
        self.progress.write_text(json.dumps({"step": 6, "total": 6, "label": "Checks", "done": flag, **extra}))

    def fingerprint(self):
        pem = (self.bridge.data / "lan" / "bridge-lan.pem").read_text()
        return hashlib.sha256(ssl.PEM_cert_to_DER_cert(pem)).hexdigest()

    def challenge(self, **headers):
        return self.bridge.get("/v1/setup", params={"nonce": NONCE}, headers=headers).json()["challenge"]


def cloud_bridge(bridges, fakes, start=True):
    bridge = bridges.new()
    home = bridge.root / ".hermes"
    home.mkdir()
    (bridge.root / "setup").mkdir()
    (bridge.root / "setup" / "setup.json").write_text(json.dumps({"pair": b64(SECRET)}))
    bridge.config.update({
        "cloud": True, "public_url": "https://203.0.113.7",
        "lan": {"enabled": True, "address": "203.0.113.7", "host": "127.0.0.1", "port": free_port()},
        "setup": {"file": str(bridge.root / "setup" / "setup.json"), "progress": str(bridge.root / "setup" / "progress.json")},
        "hermes": {"bin": str(bridge.fakebin / "hermes"), "home": str(home), "hub_url": fakes.hermes.url},
    })
    if start:
        bridge.start()
    return bridge, Cloud(bridge)


def claim_body(challenge, request=None, secret=SECRET):
    request = request or json.dumps({"model": {"provider": "custom", "name": "glm-5.3", "key": "sk-eu-0123456789",
                                               "base_url": "https://api.eurouter.example/v1"},
                                     "agents": [{"name": "Quotidien", "kind": "daily"}], "language": "fr"})
    return {"challenge": challenge, "mac": proof(secret, CLAIM_CONTEXT, challenge, request), "request": request}


def test_the_server_proves_its_certificate_before_anything(bridges, fakes):
    bridge, cloud = cloud_bridge(bridges, fakes)
    state = bridge.get("/v1/setup", params={"nonce": NONCE}).json()
    assert state["state"] == "installing" and state["fingerprint"] == cloud.fingerprint()
    assert set(state) == {"state", "step", "total", "label", "fingerprint", "challenge", "proof"}
    assert len(state["challenge"]) == 32
    # Only a server that knows the secret from the user-data can make this proof for this certificate.
    assert state["proof"] == proof(SECRET, SETUP_CONTEXT, NONCE, state["fingerprint"], state["challenge"])
    upper = bridge.get("/v1/setup", params={"nonce": NONCE.upper()}).json()      # the nonce's case does not matter
    assert upper["proof"] == proof(SECRET, SETUP_CONTEXT, NONCE, upper["fingerprint"], upper["challenge"])
    cloud.done()
    ready = bridge.get("/v1/setup", params={"nonce": NONCE}).json()
    assert ready["state"] == "ready" and (ready["step"], ready["total"], ready["label"]) == (6, 6, "Checks")
    assert bridge.get("/v1/setup", params={"nonce": "short"}).status_code == 422
    assert bridge.get("/v1/setup", params={"nonce": "zz" * 16}).status_code == 422
    # The same over the door (the app pins this certificate).
    with httpx.Client(base_url=f"https://127.0.0.1:{bridge.config['lan']['port']}", verify=False) as door:
        assert door.get("/v1/setup", params={"nonce": NONCE}).json()["fingerprint"] == cloud.fingerprint()


def test_a_claim_with_the_secret_pairs_the_app_once(bridges, fakes):
    bridge, cloud = cloud_bridge(bridges, fakes)
    cloud.done()
    challenge = cloud.challenge()
    paired = bridge.post("/v1/setup/claim", json=claim_body(challenge))
    assert paired.status_code == 200, paired.text
    assert paired.json() == {"bridgeURL": "https://203.0.113.7", "bridgeKey": BRIDGE_KEY,
                             "fingerprint": cloud.fingerprint(), "agents": [{"id": "quotidien", "name": "Quotidien"}]}
    hermes = str(bridge.fakebin / "hermes")
    commands = bridge.commands.of("hermes")
    for wanted in (["config", "set", "model.provider", "custom"], ["config", "set", "model.default", "glm-5.3"],
                   ["config", "set", "model.base_url", "https://api.eurouter.example/v1"],
                   ["config", "set", "model.key_env", "CUSTOM_MODEL_API_KEY"],
                   ["config", "set", "CUSTOM_MODEL_API_KEY", "sk-eu-0123456789"],
                   ["config", "set", "model.supports_vision", "false"], ["gateway", "restart"]):
        assert wanted in commands, wanted
    assert commands.index(["gateway", "restart"]) < next(i for i, c in enumerate(commands) if c[:2] == ["profile", "create"])
    assert all(e["hermes_home"] == str(bridge.root / ".hermes") for e in bridge.commands.entries("hermes"))
    model = tomllib.loads(bridge.toml_text())["model"]
    assert model == {"health_url": "https://api.eurouter.example/v1/models", "health_key": "sk-eu-0123456789"}
    assert stat.S_IMODE(os.stat(bridge.config_path).st_mode) == 0o600
    agent = tomllib.loads(bridge.toml_text())["agents"]["quotidien"]
    assert agent["hermes_url"] == f"{fakes.hermes.url}/p/quotidien" and agent["public_url"] == "https://203.0.113.7/hermes/quotidien"
    listed = {a["id"]: a for a in bridge.get("/v1/agents", headers=AUTH).json()["agents"]}
    assert listed["quotidien"]["url"] == "https://203.0.113.7/hermes/quotidien" and listed["quotidien"]["key"]
    assert hermes  # the fake ran as [hermes] bin
    # The secret is gone: the server says so, and a second claim, even with the same proof, is refused.
    assert not cloud.setup_file.exists()
    assert bridge.get("/v1/setup", params={"nonce": NONCE}).json() == {"state": "claimed"}
    again = bridge.post("/v1/setup/claim", json=claim_body(challenge))
    assert again.status_code == 410 and again.json() == {"detail": "this server is already paired"}


def test_challenges_are_single_use_and_claims_wait_for_the_install(bridges, fakes):
    bridge, cloud = cloud_bridge(bridges, fakes)
    challenge = cloud.challenge()
    waiting = bridge.post("/v1/setup/claim", json=claim_body(challenge))
    assert waiting.status_code == 409 and waiting.json() == {"detail": "still installing"}
    cloud.done()
    used = bridge.post("/v1/setup/claim", json=claim_body(challenge))
    assert used.status_code == 400 and used.json() == {"detail": "challenge unknown or expired: ask /v1/setup again"}
    fresh = cloud.challenge()
    bad = json.dumps({"model": {"provider": "custom", "name": "x", "key": "sk-0123456789"}, "agents": []})
    no_url = bridge.post("/v1/setup/claim", json=claim_body(fresh, bad))
    assert no_url.status_code == 400 and no_url.json() == {"detail": "a custom model needs its base_url"}
    invalid = json.dumps({"model": {"provider": "mistral", "name": "x", "key": "sk-0123456789"}})
    refused = bridge.post("/v1/setup/claim", json=claim_body(cloud.challenge(), invalid))
    assert refused.status_code == 400 and refused.json()["detail"].startswith("invalid request: ")
    not_json = bridge.post("/v1/setup/claim", json=claim_body(cloud.challenge(), "{oops"))
    assert not_json.status_code == 400 and not_json.json() == {"detail": "invalid request"}
    assert bridge.post("/v1/setup/claim", json={"challenge": "x", "mac": "y", "request": "{}"}).status_code == 422
    assert bridge.commands.of("hermes") == [] or all(c[:2] != ["profile", "create"] for c in bridge.commands.of("hermes"))


def test_wrong_proofs_get_the_address_blocked(bridges, fakes):
    bridge, cloud = cloud_bridge(bridges, fakes)
    cloud.done()
    remote = {"X-Forwarded-For": REMOTE}
    for _ in range(20):
        challenge = cloud.challenge(**remote)
        wrong = bridge.post("/v1/setup/claim", json=claim_body(challenge, secret=b"y" * 32), headers=remote)
        assert wrong.status_code == 401 and wrong.json() == {"detail": "wrong pairing proof"}
    blocked = bridge.get("/v1/setup", params={"nonce": NONCE}, headers=remote)
    assert blocked.status_code == 429 and blocked.json() == {"detail": "too many wrong proofs, try again later"}
    assert bridge.get("/v1/setup", params={"nonce": NONCE}).status_code == 200      # another address
    assert bridge.logged("wrong key")[0]["what"] == "pairing proof"


def test_wrong_bridge_keys_get_the_address_blocked_and_the_console_is_off(bridges, fakes):
    bridge, _ = cloud_bridge(bridges, fakes)
    remote = {"X-Forwarded-For": REMOTE}
    for _ in range(20):
        assert bridge.get("/v1/agents", headers={**remote, "Authorization": "Bearer nope"}).status_code == 401
    # Blocked for a while, even with the right key; requests without a key don't count.
    assert bridge.get("/v1/agents", headers={**remote, **AUTH}).status_code == 429
    assert bridge.get("/console/").status_code == 404
    assert bridge.post("/console/api/pairing").status_code == 404
    assert bridge.get("/v1/console/devices", headers=AUTH).status_code == 404


def test_a_setup_route_without_a_setup_file(bridge):
    assert bridge.get("/v1/setup", params={"nonce": NONCE}).status_code == 404
    assert bridge.post("/v1/setup/claim", json=claim_body("c" * 32)).status_code == 404


def test_claiming_sets_the_model_once_and_creates_the_agents_left(bridges, fakes):
    """A claim that failed half-way (the second agent's profile) is tried again without redoing what it did."""
    bridge, cloud = cloud_bridge(bridges, fakes)
    cloud.done()
    bridge.commands.rule("hermes", ["profile", "create", "maison"], exit=1, stderr="disk full", times=1)
    wanted = json.dumps({"model": {"provider": "openrouter", "name": "z-ai/glm-5.3", "key": "sk-or-0123456789",
                                   "vision": True},
                         "agents": [{"name": "Quotidien", "kind": "daily"}, {"name": "Maison", "kind": "home"}],
                         "language": "fr"})
    failed = bridge.post("/v1/setup/claim", json=claim_body(cloud.challenge(), wanted))
    assert failed.status_code == 502 and failed.json() == {"detail": "the server could not finish: try again"}
    commands = bridge.commands.of("hermes")
    assert ["config", "set", "model.provider", "openrouter"] in commands
    assert ["config", "set", "model.base_url", "https://openrouter.ai/api/v1"] in commands
    assert ["config", "set", "OPENROUTER_API_KEY", "sk-or-0123456789"] in commands
    assert ["config", "set", "model.supports_vision", "true"] in commands
    assert ["gateway", "restart"] in commands
    assert not any(c[:3] == ["config", "set", "model.key_env"] for c in commands)
    model = tomllib.loads(bridge.toml_text())["model"]
    assert model == {"health_url": "https://openrouter.ai/api/v1/models", "health_key": "sk-or-0123456789"}
    remembered = json.loads(cloud.setup_file.read_text())
    assert remembered["model_set"] is True and remembered["created"] == {"0": "quotidien"}
    assert remembered["pair"] == b64(SECRET)                    # not paired yet: the app may try again
    assert stat.S_IMODE(os.stat(cloud.setup_file).st_mode) == 0o600
    assert bridge.get("/v1/setup", params={"nonce": NONCE}).json()["state"] == "ready"
    bridge.commands.clear()
    agents = bridge.post("/v1/setup/claim", json=claim_body(cloud.challenge(), wanted))
    assert agents.status_code == 200, agents.text
    assert agents.json()["agents"] == [{"id": "quotidien", "name": "Quotidien"}, {"id": "maison", "name": "Maison"}]
    again = bridge.commands.of("hermes")
    assert not any(c[:2] == ["config", "set"] for c in again)
    assert sum(1 for c in again if c[:2] == ["profile", "create"]) == 1


def test_a_moved_place_is_claimed_with_the_backup_key_and_says_it_is_restoring(bridges, fakes):
    bridge, cloud = cloud_bridge(bridges, fakes)
    cloud.done()
    request = json.dumps({"restore": {"key": "the-key-the-phone-kept-0123456789"}})
    answer = bridge.post("/v1/setup/claim", json=claim_body(cloud.challenge(), request))
    assert answer.status_code == 200, answer.text
    assert answer.json() == {"restoring": True, "bridgeURL": "https://203.0.113.7", "fingerprint": cloud.fingerprint()}
    key_file = bridge.root / "setup" / "restore-key"
    assert key_file.read_text() == "the-key-the-phone-kept-0123456789"
    assert stat.S_IMODE(os.stat(key_file).st_mode) == 0o600
    assert wait_until(lambda: bridge.commands.of("systemd-run"))        # started on its own, not waited for
    run = bridge.commands.of("systemd-run")[0]
    assert run[:5] == ["--user", "--unit", "bipagents-restore", "--collect", run[4]] and run[4].startswith("--setenv=PYTHONPATH=")
    assert run[4].endswith("/installer") and any(a.startswith("--working-directory=") for a in run)
    assert run[-5:] == ["-m", "bipinstall", "restore", "--key-file", str(key_file)]
    assert (bridge.root / "setup" / "restoring.json").exists()
    assert bridge.commands.of("hermes") == [] or not any(c[:2] == ["profile", "create"] for c in bridge.commands.of("hermes"))
    state = bridge.get("/v1/setup", params={"nonce": NONCE}).json()
    assert state["state"] == "restoring" and state["proof"]                          # still proved with the secret
    busy = bridge.post("/v1/setup/claim", json=claim_body(cloud.challenge(), request))
    assert busy.status_code == 409 and busy.json() == {"detail": "already restoring"}
    # The restore failed (a wrong key): the place says why and takes another claim.
    (bridge.root / "setup" / "restoring.json").unlink()
    cloud.progress.write_text(json.dumps({"step": 0, "total": 4, "label": "this key doesn't open these backups",
                                          "done": False, "restoring": False,
                                          "error": "this key doesn't open these backups"}))
    state = bridge.get("/v1/setup", params={"nonce": NONCE}).json()
    assert state["state"] == "ready" and state["error"] == "this key doesn't open these backups"
    bad = json.dumps({"restore": {"key": "short"}})
    refused = bridge.post("/v1/setup/claim", json=claim_body(cloud.challenge(), bad))
    assert refused.status_code == 400 and refused.json() == {"detail": "invalid restore request"}


def test_a_device_key_opens_the_bridge_and_a_reopen_leaves_only_the_recovering_device(bridges, fakes):
    """Accounts: each device its own key; after a recovery without any device, the guichet's order reopens the
    pairing and only the device that proves the new secret comes in."""
    bridge, cloud = cloud_bridge(bridges, fakes)
    owner = AUTH
    made = bridge.post("/v1/device-keys", json={"name": "iPad  d'Alex"}, headers=owner)
    assert made.status_code == 200, made.text
    made = made.json()
    assert set(made) == {"id", "key"} and len(made["id"]) == 12
    ipad = {"Authorization": f"Bearer {made['key']}"}
    listed = bridge.get("/v1/device-keys", headers=ipad).json()["devices"]
    assert [(d["name"], d["me"]) for d in listed] == [("iPad d'Alex", True)]
    assert set(listed[0]) == {"id", "name", "created", "seen", "me"}
    assert bridge.get("/v1/device-keys", headers=owner).json()["devices"][0]["me"] is False
    assert bridge.get("/v1/backup", headers=ipad).status_code == 200            # any route, as with the bridge key
    stored = bridge.state_dir / "device-keys.json"
    assert made["key"] not in stored.read_text() and stat.S_IMODE(os.stat(stored).st_mode) == 0o600
    rows = json.loads(stored.read_text())
    assert rows[0]["hash"] == hashlib.sha256(made["key"].encode()).hexdigest()
    other = bridge.post("/v1/device-keys", json={"name": "Pixel"}, headers=owner).json()
    assert bridge.delete(f"/v1/device-keys/{other['id']}", headers=ipad).json() == {"ok": True}
    assert bridge.delete(f"/v1/device-keys/{other['id']}", headers=ipad).status_code == 404
    assert bridge.get("/v1/device-keys", headers={"Authorization": f"Bearer {other['key']}"}).status_code == 401
    assert bridge.post("/v1/device-keys", json={"name": ""}, headers=owner).status_code == 422

    # The seat agent wrote a new secret (installer/cloud/seat-agent.py): the place is claimed again.
    new_secret = bytes(range(32, 64))
    cloud.setup_file.write_text(json.dumps({"pair": b64(new_secret), "reopen": True}))
    cloud.done()
    request = json.dumps({"reopen": {"name": "Nouvel iPhone"}})
    again = bridge.post("/v1/setup/claim", json=claim_body(cloud.challenge(), request, secret=new_secret)).json()
    assert again["reopened"] and again["fingerprint"] == cloud.fingerprint() and again["bridgeKey"] != BRIDGE_KEY
    assert again["bridgeURL"] == "https://203.0.113.7" and again["agents"] == [{"id": "wellness", "name": "Wellness"}]
    assert bridge.get("/v1/device-keys", headers=owner).status_code == 401           # the old bridge key: gone
    assert bridge.get("/v1/device-keys", headers=ipad).status_code == 401            # the other devices: gone
    mine = {"Authorization": f"Bearer {again['bridgeKey']}"}
    assert [d["name"] for d in bridge.get("/v1/device-keys", headers=mine).json()["devices"]] == ["Nouvel iPhone"]
    text = bridge.toml_text()
    assert "bridge_key = " in text and BRIDGE_KEY not in text
    assert tomllib.loads(text)["bridge_key"] != again["bridgeKey"]                    # the device's key is its own
    assert bridge.get("/v1/setup", params={"nonce": NONCE}).json() == {"state": "claimed"}
    # After a restart, the new bridge key from the file is the one.
    bridge.restart(write=False)
    assert bridge.get("/v1/device-keys", headers=mine).status_code == 200
    assert bridge.get("/v1/agents", headers={"Authorization": f"Bearer {tomllib.loads(text)['bridge_key']}"}).status_code == 200
