import base64
import json
import secrets
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "installer"))

TEMPLATE = ROOT / "installer" / "cloud" / "user-data.yaml"


def render(pair: str, language: str = "fr", profile: str = "solo") -> str:
    """What the apps do: three placeholders, nothing else."""
    return (TEMPLATE.read_text(encoding="utf-8").replace("{{pair}}", pair).replace("{{language}}", language)
            .replace("{{profile}}", profile))


def test_the_user_data_the_apps_send_sets_up_the_server_and_runs_the_cloud_install(tmp_path):
    from bipinstall import cloud

    pair = base64.urlsafe_b64encode(secrets.token_bytes(32)).decode().rstrip("=")
    text = render(pair)
    assert text.startswith("#cloud-config\n") and "{{" not in text
    data = yaml.safe_load(text)
    assert data["users"][1] == {"name": "bip", "shell": "/bin/bash", "lock_passwd": True}   # no sudo
    assert {"podman", "uidmap", "ufw", "unattended-upgrades"} <= set(data["packages"])
    files = {f["path"]: f for f in data["write_files"]}
    setup = files["/etc/bipagents/cloud.json"]
    assert setup["permissions"] == "0600"
    (tmp_path / "cloud.json").write_text(setup["content"])
    assert cloud.read(tmp_path / "cloud.json") == cloud.CloudSetup(pair=pair, language="fr", profile="solo")
    assert "ip_unprivileged_port_start = 443" in files["/etc/sysctl.d/60-bipagents.conf"]["content"]
    assert "PasswordAuthentication no" in files["/etc/ssh/sshd_config.d/10-bipagents.conf"]["content"]
    commands = data["runcmd"]
    # Nothing in but SSH (by key) and the bridge's door; the install as `bip`, then the secret's copy removed.
    assert commands.index("ufw default deny incoming") < commands.index("ufw --force enable")
    assert "ufw allow 443/tcp" in commands and not any("ufw allow 80" in c for c in commands)
    install = next(c for c in commands if "install.sh" in c)
    assert install.startswith("sudo -iu bip ") and "--cloud /etc/bipagents/cloud.json" in install
    assert commands.index(install) < next(i for i, c in enumerate(commands) if c.startswith("shred"))
    assert commands.index("loginctl enable-linger bip") < commands.index(install)


def test_no_model_key_or_other_secret_has_a_place_in_it():
    text = TEMPLATE.read_text(encoding="utf-8")
    import re
    assert text.isascii(), "DigitalOcean ignores a user-data holding non-ASCII characters (accents, guillemets)"
    assert sorted(set(re.findall(r"\{\{(\w+)\}\}", text))) == ["language", "pair", "profile"]
    assert "KEY" not in text.replace("SSH", "")
    assert len(render("x" * 43).encode()) < 16_000   # far under the providers' user-data limits (64 KB at DigitalOcean)



def test_the_seat_script_is_ascii_and_refuses_a_wrong_call(tmp_path):
    """add-seat.sh runs as root at first boot (the guichet's script) and for the seat agent: ASCII like user-data,
    and nothing done when called without a place number of 1 or more and an existing setup file."""
    import subprocess
    path = ROOT / "installer" / "cloud" / "add-seat.sh"
    text = path.read_text(encoding="utf-8")
    assert text.isascii() and "--cloud $setup" in text and 'ufw allow "$port/tcp"' in text
    assert '[ "$n" -eq 1 ] && user="bip"' in text
    for args in ([], ["0", str(path)], ["x", str(path)], ["2", str(tmp_path / "missing.json")]):
        assert subprocess.run(["bash", str(path), *args], capture_output=True).returncode == 2


def _seat_agent():
    import importlib.util
    spec = importlib.util.spec_from_file_location("seat_agent", ROOT / "installer" / "cloud" / "seat-agent.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _signed(key, body):
    return {"id": body["order"], "body": json.dumps(body, sort_keys=True, separators=(",", ":")),
            "signature": base64.b64encode(key.sign(json.dumps(body, sort_keys=True, separators=(",", ":")).encode()))
            .decode()}


def test_the_seat_agent_does_only_signed_orders_for_its_server(tmp_path):
    """installer/cloud/seat-agent.py (root, every two minutes): an order is done only when the guichet's key signed
    it, for this server, not expired, for a place whose door matches, and not done before."""
    import shutil
    import subprocess
    import time
    import pytest
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

    agent = _seat_agent()
    assert (ROOT / "installer" / "cloud" / "seat-agent.py").read_text(encoding="utf-8").isascii()
    key, other = Ed25519PrivateKey.generate(), Ed25519PrivateKey.generate()
    pub = tmp_path / "guichet.pub"
    pub.write_bytes(key.public_key().public_bytes(serialization.Encoding.PEM,
                                                  serialization.PublicFormat.SubjectPublicKeyInfo))
    now = time.time()
    body = {"order": 7, "server": "vps-1", "seat": 2, "action": "reset", "expires": int(now + 3600),
            "setup": {"v": 1, "pair": "p" * 43, "language": "fr", "profile": "solo", "port": 444}}
    order = _signed(key, body)
    assert agent.accepted(order, "vps-1", set(), now) == body
    assert agent.accepted(order, "vps-1", {7}, now) is None                 # done before
    assert agent.accepted(order, "vps-2", set(), now) is None               # another server's
    assert agent.accepted(order, "vps-1", set(), now + 7200) is None        # expired
    for change in ({"action": "run"}, {"seat": 0}, {"seat": 3}):
        assert agent.accepted(_signed(key, {**body, **change}), "vps-1", set(), now) is None
    assert agent.accepted({**order, "id": 8}, "vps-1", set(), now) is None   # signed for another order
    assert agent.account(1) == "bip" and agent.account(3) == "bip3"
    probe = subprocess.run(["openssl", "pkeyutl", "-help"], capture_output=True, text=True)
    if shutil.which("openssl") is None or "-rawin" not in probe.stdout + probe.stderr:
        pytest.skip("no OpenSSL 3 here")
    assert agent.verified(order["body"], order["signature"], str(pub))
    assert not agent.verified(order["body"].replace("vps-1", "vps-2"), order["signature"], str(pub))
    assert not agent.verified(order["body"], _signed(other, body)["signature"], str(pub))
    assert not agent.verified(order["body"], "not base64!", str(pub))
