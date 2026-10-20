import dataclasses
import shutil
import subprocess

import pytest

from bipbridge import ssh_keys
from conftest import AUTH


def a_key(tmp_path, name, kind="ed25519"):
    path = tmp_path / name
    subprocess.run(["ssh-keygen", "-q", "-t", kind, "-N", "", "-C", f"{name}@laptop", "-f", str(path)], check=True)
    public = (tmp_path / f"{name}.pub").read_text().strip()
    shown = subprocess.run(["ssh-keygen", "-l", "-f", str(tmp_path / f"{name}.pub")], capture_output=True,
                           text=True, check=True).stdout.split()[1]
    return public, shown


@pytest.mark.skipif(shutil.which("ssh-keygen") is None, reason="no ssh-keygen here")
def test_the_person_adds_lists_and_removes_their_keys_on_a_server_of_the_offer(client, config, tmp_path, monkeypatch):
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    assert client.get("/v1/ssh", headers=AUTH).json() == {"enabled": False, "keys": []}   # someone's own computer
    public, _ = a_key(tmp_path, "one")
    assert client.post("/v1/ssh/keys", json={"key": public}, headers=AUTH).status_code == 403
    client.app.state.services.config = dataclasses.replace(config, cloud=True, public_url="https://203.0.113.7")
    assert client.get("/v1/ssh").status_code == 401
    (home / ".ssh").mkdir(mode=0o755)
    (home / ".ssh" / "authorized_keys").write_text("# kept\n")
    added = client.post("/v1/ssh/keys", json={"key": public}, headers=AUTH).json()
    other, shown = a_key(tmp_path, "two", "rsa")
    client.post("/v1/ssh/keys", json={"key": other + "\n"}, headers=AUTH)
    client.post("/v1/ssh/keys", json={"key": public}, headers=AUTH)          # once only
    status = client.get("/v1/ssh", headers=AUTH).json()
    assert status["enabled"] and status["login"].endswith("@203.0.113.7")
    assert [(k["type"], k["comment"]) for k in status["keys"]] == [("ssh-ed25519", "one@laptop"), ("ssh-rsa", "two@laptop")]
    assert status["keys"][1]["fingerprint"] == shown and added["fingerprint"] == status["keys"][0]["fingerprint"]
    text = (home / ".ssh" / "authorized_keys").read_text()
    assert text.startswith("# kept\n") and text.count("\n") == 3
    assert oct((home / ".ssh").stat().st_mode & 0o777) == "0o700"
    assert oct((home / ".ssh" / "authorized_keys").stat().st_mode & 0o777) == "0o600"
    gone = client.delete("/v1/ssh/keys", params={"fingerprint": shown}, headers=AUTH)
    assert gone.status_code == 200 and "two@laptop" not in (home / ".ssh" / "authorized_keys").read_text()
    assert client.delete("/v1/ssh/keys", params={"fingerprint": shown}, headers=AUTH).status_code == 404


def test_only_a_plain_public_key_goes_in(tmp_path):
    good = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl me"
    assert ssh_keys.parse(good)["type"] == "ssh-ed25519"
    for bad in ('command="rm -rf ~" ' + good, "from=\"*\" " + good, "ssh-dss AAAAB3NzaC1kc3MAAACBAP me",
                "ssh-ed25519 AAAAB3NzaC1yc2EAAAADAQABAAABAQC me",            # an RSA blob called ed25519
                "ssh-ed25519 not-base64!", "-----BEGIN OPENSSH PRIVATE KEY-----", good + "\nssh-rsa AAAA second"):
        with pytest.raises(ssh_keys.SshKeyError):
            ssh_keys.parse(bad)
