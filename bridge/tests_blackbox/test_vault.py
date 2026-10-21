"""The agents' vault (`/v1/vault`): credentials kept encrypted by the bridge, written into the Hermes vault of the
agents allowed (their Python run through the fake `podman exec`, the request on stdin), never shown back
(tests/test_vault.py, over HTTP)."""
import json
import os
import stat

from cryptography.fernet import Fernet

from bbkit.common import AUTH

LOGIN = {"label": "SNCF", "site": "sncf-connect.com/login", "identifier": "alex@example.org",
         "password": "s3cret-Pass!", "otp_secret": "JBSWY3DPEHPK3PXP"}
ANSWER = json.dumps({"id": "vault_1", "removed": True}) + "\n"


def vault_bridge(bridges, fakes):
    bridge = bridges.new()
    python = f"{bridge.fakebin}/podman exec -i hermes-vie /opt/hermes/venv/bin/python"
    bridge.config["agents"]["Vie"] = {"hermes_url": fakes.down.url, "hermes_key": "k" * 32, "isolated": True,
                                      "python": python, "hermes_home": "/home/hermes/.hermes"}
    bridge.config["agents"]["Budget"] = {"hermes_url": f"{fakes.hermes.url}/p/budget", "hermes_key": "b" * 32}
    bridge.config["hermes"] = {"isolated": True, "bin": str(bridge.fakebin / "hermes"), "hub_url": fakes.hermes.url}
    bridge.commands.rule("podman", ["exec", "-i", "hermes-vie"], stdout=ANSWER)
    return bridge.start()


def requests_to_vie(bridge):
    return [(e["argv"], json.loads(e["stdin"])) for e in bridge.commands.entries("podman")]


def test_items_reach_only_the_agents_allowed_and_never_come_back(bridges, fakes):
    bridge = vault_bridge(bridges, fakes)
    made = bridge.post("/v1/vault", headers=AUTH, json={**LOGIN, "agents": ["vie"]})
    assert made.status_code == 201, made.text
    item = made.json()
    assert item["id"].startswith("item_") and len(item["id"]) == 17
    assert {k: item[k] for k in ("kind", "label", "origin", "identifier_type", "identifier", "has_otp", "agents")} == {
        "kind": "login", "label": "SNCF", "origin": "https://sncf-connect.com", "identifier_type": "email",
        "identifier": "alex@example.org", "has_otp": True, "agents": ["vie"]}
    assert set(item) == {"id", "kind", "label", "origin", "identifier_type", "identifier", "has_otp", "agents",
                         "created_at", "updated_at"}
    assert "s3cret" not in made.text and "JBSWY" not in made.text
    [(argv, request)] = requests_to_vie(bridge)
    assert argv[:5] == ["exec", "-i", "hermes-vie", "/opt/hermes/venv/bin/python", "-c"] and len(argv) == 6
    compile(argv[5], "<vault script>", "exec")                     # the script Hermes' Python runs
    assert "from agent.vault_store import VaultStore" in argv[5]
    assert request == {"home": "/home/hermes/.hermes", "root": None, "op": "add", "kind": "login", "label": "SNCF",
                       "origin": "https://sncf-connect.com",
                       "secret": {"password": "s3cret-Pass!", "otp_secret": "JBSWY3DPEHPK3PXP",
                                  "identifier_type": "email", "identifier": "alex@example.org"}}
    # The app only ever reads metadata; on disk, the secret is encrypted with the key next to it.
    listed = bridge.get("/v1/vault", headers=AUTH).json()
    assert listed["items"] == [item] and listed["protected"] is True
    folder = bridge.state_dir / "vault"
    assert b"s3cret" not in (folder / "vault.json.enc").read_bytes()
    assert stat.S_IMODE(os.stat(folder / "vault.key").st_mode) == 0o600
    assert stat.S_IMODE(os.stat(folder / "vault.json.enc").st_mode) == 0o600
    records = json.loads(Fernet((folder / "vault.key").read_bytes().strip()).decrypt((folder / "vault.json.enc").read_bytes()))
    assert records[0]["secret"] == {"password": "s3cret-Pass!", "otp_secret": "JBSWY3DPEHPK3PXP"}
    assert records[0]["agents"] == {"vie": "vault_1"}
    assert bridge.get("/v1/vault").status_code == 401


def test_who_can_hold_credentials(bridges, fakes):
    bridge = vault_bridge(bridges, fakes)
    agents = {a["id"]: (a["can_hold"], a["why"]) for a in bridge.get("/v1/vault", headers=AUTH).json()["agents"]}
    assert agents == {"wellness": (False, "not_configured"), "vie": (True, None), "budget": (False, "needs_own_space")}
    refused = bridge.post("/v1/vault", headers=AUTH, json={**LOGIN, "agents": ["vie", "budget"]})
    assert refused.status_code == 409
    assert refused.json() == {"detail": {"code": "cannot_hold", "agents": {"budget": "needs_own_space"}}}
    assert requests_to_vie(bridge) == [] and bridge.get("/v1/vault", headers=AUTH).json()["items"] == []
    unknown = bridge.post("/v1/vault", headers=AUTH, json={**LOGIN, "agents": ["ghost"]})
    assert unknown.status_code == 400 and unknown.json() == {"detail": "unknown agent ghost"}


def test_update_rewrites_the_item_and_follows_the_agents(bridges, fakes):
    bridge = vault_bridge(bridges, fakes)
    item = bridge.post("/v1/vault", headers=AUTH, json={**LOGIN, "agents": ["vie"]}).json()
    bridge.commands.clear()
    # Only the agents change: nothing to rewrite for Vie.
    same = bridge.patch(f"/v1/vault/{item['id']}", headers=AUTH, json={"agents": ["vie"]})
    assert same.status_code == 200 and same.json()["agents"] == ["vie"] and requests_to_vie(bridge) == []
    # A new password: taken back from Vie, then given again; the password left out of later edits is kept.
    bridge.patch(f"/v1/vault/{item['id']}", headers=AUTH, json={"password": "n3w"})
    calls = [r for _, r in requests_to_vie(bridge)]
    assert [r["op"] for r in calls] == ["remove", "add"] and calls[0] == {"home": "/home/hermes/.hermes", "root": None,
                                                                         "op": "remove", "id": "vault_1"}
    assert calls[1]["secret"]["password"] == "n3w"
    bridge.commands.clear()
    renamed = bridge.patch(f"/v1/vault/{item['id']}", headers=AUTH, json={"label": "SNCF Connect"}).json()
    calls = [r for _, r in requests_to_vie(bridge)]
    assert calls[1]["secret"]["password"] == "n3w" and calls[1]["label"] == "SNCF Connect"
    assert renamed["label"] == "SNCF Connect" and renamed["created_at"] == item["created_at"]
    bridge.commands.clear()
    taken = bridge.patch(f"/v1/vault/{item['id']}", headers=AUTH, json={"agents": []}).json()
    assert [r["op"] for _, r in requests_to_vie(bridge)] == ["remove"] and taken["agents"] == []
    no_otp = bridge.patch(f"/v1/vault/{item['id']}", headers=AUTH, json={"otp_secret": ""}).json()
    assert no_otp["has_otp"] is False
    missing = bridge.patch("/v1/vault/item_000000000000", headers=AUTH, json={"label": "x"})
    assert missing.status_code == 404 and missing.json() == {"detail": "no such item"}
    assert bridge.delete(f"/v1/vault/{item['id']}", headers=AUTH).status_code == 204
    assert bridge.delete(f"/v1/vault/{item['id']}", headers=AUTH).status_code == 404
    assert bridge.get("/v1/vault", headers=AUTH).json()["items"] == []


def test_what_the_app_must_send(bridges, fakes):
    bridge = vault_bridge(bridges, fakes)
    for body, detail in (({**LOGIN, "password": ""}, "password is required"),
                         ({**LOGIN, "identifier": " "}, "user name is required"),
                         ({**LOGIN, "site": "ftp://x"}, "site must be a web address"),
                         ({**LOGIN, "site": ""}, "site is required"),
                         ({**LOGIN, "label": ""}, "name is required"),
                         ({**LOGIN, "kind": "ssh"}, "unknown kind"),
                         ({**LOGIN, "identifier_type": "fax"}, "unknown identifier type")):
        resp = bridge.post("/v1/vault", headers=AUTH, json={**body, "agents": []})
        assert resp.status_code == 400 and resp.json() == {"detail": detail}, detail
    card = bridge.post("/v1/vault", headers=AUTH, json={"kind": "payment", "label": "Visa", "card_number": "4111",
                                                         "cvc": "123", "agents": []}).json()
    assert card["kind"] == "payment" and card["origin"] is None and card["identifier"] is None
    port = bridge.post("/v1/vault", headers=AUTH, json={**LOGIN, "site": "HTTPS://Example.com:443/a?b", "agents": []})
    assert port.json()["origin"] == "https://example.com"
    other = bridge.post("/v1/vault", headers=AUTH, json={**LOGIN, "site": "example.com:8443", "agents": []})
    assert other.json()["origin"] == "https://example.com:8443"


def test_hermes_refusing_an_item_is_said_without_the_secret(bridges, fakes):
    bridge = vault_bridge(bridges, fakes)
    bridge.commands.clear_rules()
    bridge.commands.rule("podman", ["exec"], exit=1, stderr="Traceback…\nVaultError: origin not allowed\n")
    refused = bridge.post("/v1/vault", headers=AUTH, json={**LOGIN, "agents": ["vie"]})
    assert refused.status_code == 400
    assert refused.json() == {"detail": "vie: Hermes refused the item (VaultError: origin not allowed)"}
    assert bridge.get("/v1/vault", headers=AUTH).json()["items"] == []


def test_an_install_whose_agents_can_read_the_bridge_keeps_no_credentials(bridge):
    listing = bridge.get("/v1/vault", headers=AUTH).json()
    assert listing == {"protected": False, "items": [],
                       "agents": [{"id": "wellness", "name": "Wellness", "can_hold": False, "why": "unprotected_install"}]}
    refused = bridge.post("/v1/vault", headers=AUTH, json={**LOGIN, "agents": []})
    assert refused.status_code == 409 and refused.json() == {"detail": {"code": "unprotected_install"}}
    assert bridge.patch("/v1/vault/item_x", headers=AUTH, json={}).status_code == 409
    assert not (bridge.state_dir / "vault").exists()   # nothing written, not even encrypted
    assert bridge.get("/v1/status", headers=AUTH).json()["protected"] is False
