import dataclasses
import hashlib
import hmac
import json
import secrets

from bipbridge import place_import
from conftest import AUTH


def mac(code, *parts):
    return hmac.new(code.encode(), "\n".join(parts).encode(), hashlib.sha256).hexdigest()


def test_a_computer_proves_the_code_then_sends_its_export_once(client, config, tmp_path, monkeypatch):
    from bipbridge.lan import ensure_certificate, fingerprint
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    started = []
    monkeypatch.setattr(place_import, "start_import", lambda folder, archive, secret: started.append((archive, secret)))
    assert client.post("/v1/import", headers=AUTH).status_code == 404               # someone's own computer
    client.app.state.services.config = dataclasses.replace(config, cloud=True, public_url="https://203.0.113.7:8701")
    ensure_certificate(config)
    assert client.get("/v1/import/hello", params={"nonce": secrets.token_hex(16)}).status_code == 404   # no code yet
    assert client.post("/v1/import").status_code == 401
    given = client.post("/v1/import", headers=AUTH).json()
    assert given["state"] == "waiting" and given["address"] == "203.0.113.7:8701"
    assert given["command"] == f"bipagents send 203.0.113.7:8701 {given['code']}"
    assert given["command_alone"].endswith(f"| bash -s -- send 203.0.113.7:8701 {given['code']}")
    code = given["code"].replace("-", "")
    assert len(code) == 16 and set(code) <= set(place_import.ALPHABET)

    nonce = secrets.token_hex(16)
    hello = client.get("/v1/import/hello", params={"nonce": nonce}).json()
    assert hello["fingerprint"] == fingerprint(config)
    assert hello["proof"] == mac(code, place_import.IMPORT_CONTEXT, nonce, hello["fingerprint"], hello["challenge"])

    body = b"sealed export " * 1000
    wrong = client.put("/v1/import/archive", content=body, headers={
        "X-Import-Challenge": hello["challenge"], "X-Import-Proof": mac("0" * 16, place_import.UPLOAD_CONTEXT,
                                                                        hello["challenge"], str(len(body)))})
    assert wrong.status_code == 401 and not started
    challenge = client.get("/v1/import/hello", params={"nonce": nonce}).json()["challenge"]
    sent = client.put("/v1/import/archive", content=body, headers={
        "X-Import-Challenge": challenge, "X-Import-Proof": mac(code, place_import.UPLOAD_CONTEXT, challenge, str(len(body)))})
    assert sent.json() == {"started": True}
    archive, secret = started[0]
    assert archive.read_bytes() == body and secret.read_text() == code
    assert oct(archive.stat().st_mode & 0o777) == "0o600" and oct(secret.stat().st_mode & 0o777) == "0o600"
    assert client.get("/v1/import", headers=AUTH).json()["state"] == "importing"
    # Used: no second sending with it, and nothing to say hello to.
    assert client.get("/v1/import/hello", params={"nonce": nonce}).status_code == 404
    again = client.put("/v1/import/archive", content=body, headers={
        "X-Import-Challenge": challenge, "X-Import-Proof": mac(code, place_import.UPLOAD_CONTEXT, challenge, str(len(body)))})
    assert again.status_code == 403
    assert client.post("/v1/import", headers=AUTH).status_code == 409                # not while it is put in

    progress = home / place_import.FOLDER / "progress.json"
    progress.write_text(json.dumps({"step": 4, "total": 4, "label": "Ready", "done": True, "at": 1}))
    assert client.get("/v1/import", headers=AUTH).json()["state"] == "done"
    assert client.delete("/v1/import", headers=AUTH).status_code == 204
    assert client.get("/v1/import", headers=AUTH).json() == {"state": "none"}


def test_a_code_lasts_an_hour_and_a_stuck_import_is_told(tmp_path):
    from bipbridge.config import Config
    now = [1000.0]
    imports = place_import.Imports(tmp_path, clock=lambda: now[0])
    config = dataclasses.replace(Config(bridge_key="k" * 32), cloud=True, public_url="https://203.0.113.7")
    assert imports.new_code(config)["address"] == "203.0.113.7"
    now[0] += place_import.CODE_SECONDS + 1
    assert imports.code() is None and imports.status(config) == {"state": "none"}
    (tmp_path / place_import.FOLDER / "progress.json").write_text(json.dumps({"step": 2, "total": 4, "at": now[0]}))
    assert imports.status(config)["state"] == "importing" and imports.importing()
    now[0] += place_import.STALE_SECONDS
    assert imports.status(config)["state"] == "failed" and not imports.importing()
