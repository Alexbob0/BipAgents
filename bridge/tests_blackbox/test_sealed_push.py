"""Sealed notifications (refonte phase 5; bridge-rs push_runs/seal.rs): a phone that registers with its key
(`push_key`) gets the agent's words only in `enc`, sealed for it, and a neutral line in clear; a phone without a key
gets today's push; a registration without the key drops it; a key that is not one is refused."""
import base64
import json

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.hkdf import HKDF

from bbkit.common import AUTH, sse
from bbkit.servers import wait_until

SEALED, PLAIN = "a" * 64, "b" * 64


def new_key():
    private = ec.generate_private_key(ec.SECP256R1())
    public = private.public_key().public_bytes(serialization.Encoding.X962, serialization.PublicFormat.UncompressedPoint)
    return private, public


def open_enc(enc: str, agent: str, private, public: bytes) -> dict:
    raw = base64.b64decode(enc)
    ephemeral = ec.EllipticCurvePublicKey.from_encoded_point(ec.SECP256R1(), raw[:65])
    shared = private.exchange(ec.ECDH(), ephemeral)
    key = HKDF(algorithm=hashes.SHA256(), length=32, salt=raw[:65] + public, info=b"bipagents-notif-v1").derive(shared)
    return json.loads(AESGCM(key).decrypt(raw[65:77], raw[77:], agent.encode()))


def register(bridge, token, push_key=None):
    body = {"token": token, "environment": "production", "agent_ids": ["wellness"]}
    if push_key is not None:
        body["push_key"] = push_key
    return bridge.post("/v1/devices", headers=AUTH, json=body)


def test_a_phone_with_a_key_gets_the_words_sealed_and_a_neutral_line(bridge, fakes):
    private, public = new_key()
    resp = register(bridge, SEALED, base64.b64encode(public).decode())
    assert resp.status_code == 200 and resp.json()["sealed"] is True and "push_key" not in resp.json()
    assert register(bridge, PLAIN).status_code == 200
    with bridge.db() as db:
        assert db.execute("SELECT push_key FROM devices WHERE token=?", (SEALED,)).fetchone()[0] == base64.b64encode(public).decode()
    assert "push_seal" in bridge.get("/health").json().get("features", [])
    fakes.hermes.runs["run_1"] = [sse("run.completed", output="Le rendez-vous chez le dentiste est à 9 h.")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_1"})
    assert wait_until(lambda: len(fakes.apns.pushes()) == 2)
    by_token = {fakes.apns.token(p): p.json() for p in fakes.apns.pushes()}
    sealed, plain = by_token[SEALED], by_token[PLAIN]
    assert sealed["aps"]["alert"] == {"loc-key": "notif.reply", "body": "notif.reply"}
    assert "dentiste" not in json.dumps(sealed["aps"]) and "title" not in sealed
    opened = open_enc(sealed["enc"], "wellness", private, public)
    assert opened["v"] == 1 and opened["t"] == "reply" and opened["body"] == "Le rendez-vous chez le dentiste est à 9 h."
    assert opened["title"]
    # the ids the apps route by are the same for both phones
    for key in ("agent", "n", "t", "r"):
        assert sealed[key] == plain[key]
    # the phone without a key: the neutral line only, no enc, no words
    assert "enc" not in plain and plain["aps"]["alert"] == {"loc-key": "notif.reply", "body": "notif.reply"}


def test_a_registration_without_the_key_drops_it_and_a_bad_key_is_refused(bridge, fakes):
    _, public = new_key()
    assert register(bridge, SEALED, base64.b64encode(public).decode()).status_code == 200
    again = register(bridge, SEALED)
    assert again.status_code == 200 and "sealed" not in again.json()
    with bridge.db() as db:
        assert db.execute("SELECT push_key FROM devices WHERE token=?", (SEALED,)).fetchone()[0] is None
    for bad in ["pas une clé", base64.b64encode(b"\x04" + b"\x01" * 64).decode(), base64.b64encode(public[:33]).decode()]:
        resp = register(bridge, SEALED, bad)
        assert resp.status_code == 400 and "push_key" in resp.json()["detail"], bad
    assert register(bridge, SEALED, "x" * 201).status_code == 422
    # the device removed: its key with it
    assert register(bridge, SEALED, base64.b64encode(public).decode()).status_code == 200
    assert bridge.delete(f"/v1/devices/{SEALED}", headers=AUTH).status_code == 204
    with bridge.db() as db:
        assert db.execute("SELECT count(*) FROM devices").fetchone()[0] == 0
