"""One's own Hermes into a place of the offer: a computer's export (BipAgents, or a Hermes alone) replaces the place's
agents; the place keeps its door, keys, model and sandbox; a failure leaves the place as it was."""
import datetime
import hashlib
import json
import os
import sqlite3
import ssl
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from types import SimpleNamespace

import httpx
import pytest
import yaml

from bipinstall import hermes, migrations, moving, to_place, update

CODE = "K7QF9XZ2MA4D0N8H"
PC = "alex-pc.tail1234.ts.net"


def write(path, text, mode=0o644):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    os.chmod(path, mode)


def database(path, rows):
    path.parent.mkdir(parents=True, exist_ok=True)
    db = sqlite3.connect(path)
    db.execute("CREATE TABLE t (v TEXT)")
    db.executemany("INSERT INTO t VALUES (?)", [(r,) for r in rows])
    db.commit()
    db.close()


@pytest.fixture
def computer(tmp_path, monkeypatch):
    """Alex's computer: BipAgents over the Hermes they had (its main profile is an agent), a profile agent, one in a
    space of its own and one set up by hand."""
    home = tmp_path / "home" / "alex"
    data, hub = home / ".local" / "share" / "bipagents", home / ".hermes"
    config = home / ".config" / "bipagents" / "bridge.toml"
    write(config, f'''bridge_key = "computer-key"
port = 8643
data_dir = "{data}"
public_url = "https://{PC}:8643"

[hermes]
bin = "{home}/.local/bin/hermes"
home = "{hub}"
hub_url = "http://127.0.0.1:8642"

[media.roots]
"{hub}/media" = "{hub}/media"

[lan]
enabled = true
port = 8644
address = "192.168.1.20"

[pronunciations.fr]
Pixel = "Pixèle"

[agents.default]
display_name = "Hermes"
hermes_url = "http://127.0.0.1:8642"
hermes_key = "root-key"
upload_dir_host = "{hub}/uploads"
upload_dir_container = "{hub}/uploads"
public_url = "https://{PC}:8642"

[agents.coach]
display_name = "Coach"
hermes_url = "http://127.0.0.1:8642/p/coach"
hermes_key = "coach-key"
upload_dir_host = "{hub}/profiles/coach/uploads"
public_url = "https://{PC}:8642/p/coach"
voice = "pocket:colibri"

[agents.vie]
display_name = "Vie"
hermes_url = "http://127.0.0.1:8660/p/vie"
hermes_key = "vie-key"
space = "{data}/spaces/vie"

[agents.docker]
display_name = "Docker"
hermes_url = "http://127.0.0.1:9999"
hermes_key = "by-hand"
''', 0o600)
    write(hub / "config.yaml", yaml.safe_dump({
        "model": {"provider": "custom", "default": "glm-4.6", "base_url": "http://192.168.1.30:8888/v1"},
        "terminal": {"backend": "local"}, "providers": {"home": {"base_url": "http://192.168.1.30:8888/v1"}},
        "auxiliary": {"vision": {"provider": "home", "model": "glm-v", "base_url": "http://192.168.1.30:8888/v1"}},
        "mcp_servers": {"bipagents": {"url": "http://127.0.0.1:8643/v1/agent-tools/mcp"}}}))
    write(hub / ".env", "API_SERVER_ENABLED=true\nAPI_SERVER_PORT=8642\nAPI_SERVER_KEY=root-key\n"
                        "OPENROUTER_API_KEY=alex-own\n", 0o600)
    write(hub / "memories" / "MEMORY.md", "Le chat s'appelle Pixel.")
    database(hub / "state.db", ["rappelle-moi le vétérinaire", f"MEDIA:{hub}/media/photo.jpg"])
    write(hub / "media" / "photo.jpg", "jpeg")
    write(hub / "hermes-agent" / "run_agent.py", "code")
    write(hub / "profiles" / "coach" / "config.yaml", yaml.safe_dump({
        "model": {"provider": "custom", "default": "glm-4.6", "base_url": "http://192.168.1.30:8888/v1"},
        "delegation": {"model": "glm-4.6", "provider": "custom"}, "approvals": {"mode": "smart"},
        "mcp_servers": {"bipagents": {"url": "http://127.0.0.1:8643/v1/agent-tools/mcp"}}}))
    write(hub / "profiles" / "coach" / ".env", "API_SERVER_KEY=coach-key\n", 0o600)
    write(hub / "profiles" / "coach" / "memories" / "MEMORY.md", "Alex court le dimanche.")
    database(data / "bridge.db", ["computer phone token"])
    write(data / "vault" / "vault.key", "fernet", 0o600)
    write(data / "lan" / "bridge-lan.pem", "COMPUTER CERTIFICATE")
    write(data / "spaces" / "vie" / "profiles" / "vie" / "SOUL.md", "Tu es Vie.")
    write(data / "browsers" / "coach" / "ports.json", json.dumps({"cdp": 9600, "vnc": 9601}))
    write(data / "browsers" / "coach" / "profile" / "Default" / "Preferences", "{}")
    monkeypatch.setattr(hermes, "sandbox_runtime", lambda: None)
    monkeypatch.setattr(hermes, "hermes_checkout", lambda: tmp_path / "no-git")
    return home, config


@pytest.fixture
def place(tmp_path, monkeypatch):
    """A place of the offer, claimed: its door, its agent made at the claim, the offer's model in a sandbox."""
    home = tmp_path / "home" / "bip3"
    data, hub = home / ".local" / "share" / "bipagents", home / ".hermes"
    config = home / ".config" / "bipagents" / "bridge.toml"
    write(config, f'''bridge_key = "place-key"
host = "127.0.0.1"
port = 8673
data_dir = "{data}"
cloud = true
public_url = "https://203.0.113.7:8701"

[hermes]
bin = "{home}/.local/bin/hermes"
home = "{hub}"
hub_url = "http://127.0.0.1:8672"
isolated = true

[browser]
enabled = true
first_port = 9620
ports = 10

[media.roots]
"{hub}/media" = "{hub}/media"

[model]
health_url = "https://api.eurouter.ai/v1/models"

[lan]
enabled = true
port = 8701
address = "203.0.113.7"

[agents.assistant]
display_name = "Assistant"
hermes_url = "http://127.0.0.1:8672/p/assistant"
hermes_key = "assistant-key"
''', 0o600)
    offer = {"provider": "custom", "default": "mistral-medium", "base_url": "https://api.eurouter.ai/v1",
             "key_env": "CUSTOM_MODEL_API_KEY", "supports_vision": True}
    write(hub / "config.yaml", yaml.safe_dump({"model": offer, "terminal": {"backend": "docker"},
                                               "gateway": {"multiplex_profiles": True}}))
    write(hub / ".env", "API_SERVER_ENABLED=true\nAPI_SERVER_HOST=127.0.0.1\nAPI_SERVER_PORT=8672\n"
                        "API_SERVER_KEY=place-hub\nCUSTOM_MODEL_API_KEY=offer-key\n"
                        "HERMES_DOCKER_BINARY=/usr/bin/podman\n", 0o600)
    write(hub / "hermes-agent" / "run_agent.py", "the place's Hermes")
    write(hub / "profiles" / "assistant" / "memories" / "MEMORY.md", "Bonjour.")
    write(data / "lan" / "bridge-lan.pem", "PLACE CERTIFICATE")
    write(data / "sleep.json", "{}")
    database(data / "bridge.db", ["place phone token"])
    write(data / "browsers" / "assistant" / "ports.json", json.dumps({"cdp": 9620}))
    monkeypatch.setattr(update, "gateway_units", lambda: [("systemd", "bipagents-hermes")])
    monkeypatch.setattr(migrations, "run", lambda runner, home=None, own_hermes=True: [])
    monkeypatch.setenv("HOME", str(home))
    return home, config


def export(inv, tmp_path, lone=False):
    machine = SimpleNamespace(system="linux", tailnet_name=PC, tailscale=None)
    out = tmp_path / "sent.bipagents"
    info, _ = moving.export(inv, out, CODE, machine, extra={"for_place": True, "lone": lone})
    return info, out


def test_the_code_s_proofs_are_the_bridge_s():
    # bridge/bipbridge/place_import.py computes the same (bridge/tests/test_place_import.py checks it there).
    assert to_place.normalize_code("k7qf-9xz2-ma4d-0n8h") == CODE
    assert to_place.normalize_code("K7QF 9XZ2 MA4D ON8H") == CODE              # O read as 0
    assert to_place.proof(CODE, "a", "b") == __import__("hmac").new(
        CODE.encode(), b"a\nb", hashlib.sha256).hexdigest()
    assert to_place.parse_address("https://203.0.113.7:8701/") == ("203.0.113.7", 8701)
    assert to_place.parse_address("203.0.113.7") == ("203.0.113.7", 443)


def test_a_bipagents_computer_s_agents_replace_the_place_s(computer, place, tmp_path):
    home, config = computer
    inv = to_place.what_to_send(str(config), home)
    assert inv.names() == ["Hermes", "Coach"] and inv.left_out() == ["Vie", "Docker"]
    info, out = export(inv, tmp_path)
    new, new_config = place
    runner = hermes.Runner(dry=True)
    to_place.bring_in(out, CODE.lower(), runner, base=new, repo=tmp_path / "repo")
    hub, profiles = new / ".hermes", new / ".hermes" / "profiles"
    # The computer's agents, the place's gone; the main profile now a profile of its own.
    assert sorted(p.name for p in profiles.iterdir()) == ["coach", "hermes"]
    assert (profiles / "coach" / "memories" / "MEMORY.md").read_text() == "Alex court le dimanche."
    assert (profiles / "hermes" / "memories" / "MEMORY.md").read_text() == "Le chat s'appelle Pixel."
    assert sqlite3.connect(profiles / "hermes" / "state.db").execute("SELECT v FROM t").fetchall()[0] == (
        "rappelle-moi le vétérinaire",)
    assert not (profiles / "hermes" / "hermes-agent").exists()
    assert (hub / "hermes-agent" / "run_agent.py").read_text() == "the place's Hermes"       # its Hermes stays
    # The place's model and sandbox for every one; their own keys stay.
    for name, key in (("coach", "coach-key"), ("hermes", "root-key")):
        settings = yaml.safe_load((profiles / name / "config.yaml").read_text())
        assert settings["model"]["base_url"] == "https://api.eurouter.ai/v1"
        assert settings["terminal"] == {"backend": "docker"} and settings["approvals"]["mode"] == "manual"
        assert "providers" not in settings and "8673" in settings["mcp_servers"]["bipagents"]["url"]
        env = (profiles / name / ".env").read_text()
        assert f"API_SERVER_KEY={key}" in env and "CUSTOM_MODEL_API_KEY=offer-key" in env
        assert "HERMES_DOCKER_BINARY=/usr/bin/podman" in env and "API_SERVER_PORT" not in env
    assert yaml.safe_load((profiles / "hermes" / "config.yaml").read_text())["auxiliary"]["vision"] == {
        "provider": "auto", "model": ""}
    assert "delegation" not in yaml.safe_load((profiles / "coach" / "config.yaml").read_text()) or \
        "model" not in yaml.safe_load((profiles / "coach" / "config.yaml").read_text())["delegation"]
    assert "OPENROUTER_API_KEY=alex-own" in (profiles / "hermes" / ".env").read_text()   # theirs, unused here
    # The place's hub as it was.
    assert "API_SERVER_KEY=place-hub" in (hub / ".env").read_text()
    # The bridge's data: the person's, with the place's door certificate.
    data = new / ".local" / "share" / "bipagents"
    assert (data / "lan" / "bridge-lan.pem").read_text() == "PLACE CERTIFICATE"
    assert (data / "vault" / "vault.key").read_text() == "fernet" and (data / "sleep.json").exists()
    assert sqlite3.connect(data / "bridge.db").execute("SELECT v FROM t").fetchall() == [("computer phone token",)]
    assert not (data / "spaces").exists() and not (data / "browsers" / "assistant").exists()
    assert not (data / "browsers" / "coach" / "ports.json").exists()                     # picked from the place's
    assert (data / "browsers" / "coach" / "profile" / "Default" / "Preferences").exists()
    # Its config: the place's, with the computer's agents.
    text = new_config.read_text()
    parsed = __import__("tomllib").loads(text)
    assert parsed["bridge_key"] == "place-key" and parsed["public_url"] == "https://203.0.113.7:8701"
    assert parsed["lan"]["port"] == 8701 and parsed["browser"]["first_port"] == 9620
    assert sorted(parsed["agents"]) == ["coach", "hermes"]
    assert parsed["agents"]["coach"]["hermes_url"] == "http://127.0.0.1:8672/p/coach"
    assert parsed["agents"]["coach"]["voice"] == "pocket:colibri" and "public_url" not in parsed["agents"]["coach"]
    assert parsed["agents"]["hermes"]["hermes_url"] == "http://127.0.0.1:8672/p/hermes"
    assert parsed["agents"]["hermes"]["hermes_key"] == "root-key"
    assert parsed["agents"]["hermes"]["upload_dir_host"] == str(profiles / "hermes" / "uploads")
    assert parsed["pronunciations"]["fr"] == {"Pixel": "Pixèle"}
    assert parsed["media"]["roots"][f"{home}/.hermes/media"] == str(profiles / "hermes" / "media")
    assert parsed["media"]["roots"][f"{home}/.hermes/profiles"] == str(profiles)
    assert str(home) not in "\n".join(line for line in text.splitlines() if not line.startswith('"'))
    # Put in, its browsers again, nothing left lying around.
    log = "\n".join(runner.log)
    assert "browser remove --all" in log and " adopt " in log and "browser add --all" in log
    assert log.index("browser remove") < log.index("adopt") < log.index("browser add")
    assert not (new / to_place.FOLDER / "aside").exists() and not (new / to_place.FOLDER / "in").exists()


def test_a_failure_leaves_the_place_as_it_was(computer, place, tmp_path, monkeypatch):
    home, config = computer
    _, out = export(to_place.what_to_send(str(config), home), tmp_path)
    new, new_config = place
    before = new_config.read_text()

    def broken(*args):
        raise OSError("disk full")

    monkeypatch.setattr(to_place, "place_profile", broken)
    with pytest.raises(OSError):
        to_place.bring_in(out, CODE, hermes.Runner(dry=True), base=new, repo=tmp_path / "repo")
    profiles = new / ".hermes" / "profiles"
    data = new / ".local" / "share" / "bipagents"
    assert sorted(p.name for p in profiles.iterdir()) == ["assistant"]
    assert (profiles / "assistant" / "memories" / "MEMORY.md").read_text() == "Bonjour."
    assert sqlite3.connect(data / "bridge.db").execute("SELECT v FROM t").fetchall() == [("place phone token",)]
    assert not (data / "vault").exists() and (data / "browsers" / "assistant" / "ports.json").exists()
    assert new_config.read_text() == before
    with pytest.raises(Exception):
        to_place.bring_in(out, "wrong code", hermes.Runner(dry=True), base=new, repo=tmp_path / "repo")
    assert new_config.read_text() == before


def test_a_hermes_alone_comes_in_as_profiles_the_bridge_then_claims(place, tmp_path):
    alone = tmp_path / "home" / "sam"
    hub = alone / ".hermes"
    write(hub / "config.yaml", yaml.safe_dump({"model": {"provider": "anthropic", "default": "claude"}}))
    write(hub / ".env", "ANTHROPIC_API_KEY=sam-own\n", 0o600)
    write(hub / "memories" / "MEMORY.md", "Sam a deux chats.")
    write(hub / "profiles" / "cuisine" / "config.yaml", "model:\n  default: claude\n")
    write(hub / "profiles" / "cuisine" / "SOUL.md", "Tu cuisines.")
    inv = to_place.LoneHermes(hub, alone)
    assert inv.names() == ["Hermes", "Cuisine"] and inv.ports()["hub"] == 8642
    _, out = export(inv, tmp_path, lone=True)
    new, new_config = place
    runner = hermes.Runner(dry=True)
    to_place.bring_in(out, CODE, runner, base=new, repo=tmp_path / "repo")
    profiles = new / ".hermes" / "profiles"
    assert sorted(p.name for p in profiles.iterdir()) == ["cuisine", "hermes"]
    assert (profiles / "hermes" / "memories" / "MEMORY.md").read_text() == "Sam a deux chats."
    assert yaml.safe_load((profiles / "cuisine" / "config.yaml").read_text())["model"]["default"] == "mistral-medium"
    parsed = __import__("tomllib").loads(new_config.read_text())
    assert "agents" not in parsed and parsed["bridge_key"] == "place-key"       # claimed by `bipbridge adopt`
    data = new / ".local" / "share" / "bipagents"
    assert sqlite3.connect(data / "bridge.db").execute("SELECT v FROM t").fetchall() == [("place phone token",)]
    assert parsed["media"]["roots"][f"{hub}/media"] == str(profiles / "hermes" / "media")


def certificate(tmp_path):
    """A certificate made as the bridge's door makes its own (lan.py): self-signed, no extension."""
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import ec
    from cryptography.x509.oid import NameOID
    key = ec.generate_private_key(ec.SECP256R1())
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "BipAgents bridge (LAN)")])
    now = datetime.datetime.now(datetime.timezone.utc)
    cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name).public_key(key.public_key())
            .serial_number(x509.random_serial_number()).not_valid_before(now - datetime.timedelta(days=1))
            .not_valid_after(now + datetime.timedelta(days=365)).sign(key, hashes.SHA256()))
    cert_path, key_path = tmp_path / "door.pem", tmp_path / "door.key"
    cert_path.write_bytes(cert.public_bytes(serialization.Encoding.PEM))
    key_path.write_bytes(key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                                           serialization.NoEncryption()))
    return cert_path, key_path


def test_the_computer_trusts_the_place_s_own_certificate_and_no_other(tmp_path):
    cert_path, key_path = certificate(tmp_path)
    other, _ = certificate(tmp_path / "other") if (tmp_path / "other").mkdir() is None else (None, None)

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"ok")

        def log_message(self, *args):
            pass

    server = HTTPServer(("127.0.0.1", 0), Handler)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert_path, key_path)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        url = f"https://127.0.0.1:{server.server_address[1]}/"
        with httpx.Client(verify=to_place.pinned(cert_path.read_text())) as client:
            assert client.get(url).text == "ok"
        with httpx.Client(verify=to_place.pinned(other.read_text())) as client, pytest.raises(httpx.ConnectError):
            client.get(url)
    finally:
        server.shutdown()


def test_meeting_a_place_checks_its_proof_and_certificate(tmp_path):
    cert_path, _ = certificate(tmp_path)
    pem = cert_path.read_text()
    fingerprint = hashlib.sha256(ssl.PEM_cert_to_DER_cert(pem)).hexdigest()

    def place_with(code, status=200):
        def answer(request):
            if status != 200:
                return httpx.Response(status)
            nonce = request.url.params["nonce"]
            return httpx.Response(200, json={"fingerprint": fingerprint, "challenge": "c1",
                                             "proof": to_place.proof(code, to_place.IMPORT_CONTEXT, nonce, fingerprint, "c1")})
        return httpx.Client(transport=httpx.MockTransport(answer))

    got = to_place.meet("203.0.113.7", 8701, CODE, client=place_with(CODE), certificate=lambda h, p: pem)
    assert got == (fingerprint, "c1", pem)
    with pytest.raises(to_place.PlaceError, match="isn't this place's"):
        to_place.meet("203.0.113.7", 8701, CODE, client=place_with("0" * 16), certificate=lambda h, p: pem)
    other, _ = certificate(tmp_path / "x") if (tmp_path / "x").mkdir() is None else (None, None)
    with pytest.raises(to_place.PlaceError, match="certificate"):
        to_place.meet("203.0.113.7", 8701, CODE, client=place_with(CODE), certificate=lambda h, p: other.read_text())
    with pytest.raises(to_place.PlaceError, match="waits for nothing"):
        to_place.meet("203.0.113.7", 8701, CODE, client=place_with(CODE, 404), certificate=lambda h, p: pem)

    sent = {}

    def receive(request):
        sent.update(headers=dict(request.headers), body=request.read())
        return httpx.Response(200, json={"started": True})

    archive = tmp_path / "export.bipagents"
    archive.write_bytes(b"x" * 3_000_000)
    answer = to_place.upload("203.0.113.7", 8701, pem, CODE, "c2", archive,
                             client=httpx.Client(transport=httpx.MockTransport(receive)))
    assert answer == {"started": True} and sent["body"] == archive.read_bytes()
    assert sent["headers"]["content-length"] == "3000000" and "transfer-encoding" not in sent["headers"]
    assert sent["headers"]["x-import-proof"] == to_place.proof(CODE, to_place.UPLOAD_CONTEXT, "c2", "3000000")
