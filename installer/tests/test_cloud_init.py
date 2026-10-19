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


def test_the_apps_carry_the_same_user_data():
    """The apps build the user-data themselves (no server of ours in between): their copy is this file, to the byte."""
    import re
    expected = TEMPLATE.read_text(encoding="utf-8").rstrip("\n")
    swift = (ROOT / "App" / "Features" / "Cloud" / "CloudInit.swift").read_text(encoding="utf-8")
    found = re.search(r'static let template = #"""\n(.*?)\n"""#', swift, re.S)
    assert found and found.group(1) == expected, "App/Features/Cloud/CloudInit.swift differs from installer/cloud/user-data.yaml"
    kotlin = ROOT / "android" / "app" / "src" / "main" / "java" / "io" / "github" / "bipagents" / "cloud" / "CloudInit.kt"
    if kotlin.exists():
        found = re.search(r'val TEMPLATE = """\n(.*?)\n"""\.trimIndent\(\)', kotlin.read_text(encoding="utf-8"), re.S)
        assert found and found.group(1).replace("${'$'}", "$") == expected, f"{kotlin.name} differs from user-data.yaml"
