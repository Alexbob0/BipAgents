"""`bipagents encrypt` (installer/bipinstall/space.py): the agents' services wait for the open folder, the bridge
does the encryption, a failure starts everything again as before; the offer's setup turns it on per place."""
import json
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from bipinstall import cloud, space

ROOT = Path(__file__).resolve().parents[2]


def _units(home: Path, names):
    folder = home / ".config" / "systemd" / "user"
    folder.mkdir(parents=True, exist_ok=True)
    for name in names:
        (folder / name).write_text("[Service]\n")


def test_the_services_holding_someone_s_data_wait_for_the_open_folder(tmp_path):
    _units(tmp_path, ["hermes-gateway.service", "bipagents-bridge.service", "bipagents-pocket.service",
                      "bipagents-backup.service", "bipagents-backup.timer", "bipagents-refresh-browser.service",
                      "bipagents-update.service"])
    quadlets = tmp_path / ".config" / "containers" / "systemd"
    quadlets.mkdir(parents=True)
    (quadlets / "bipagents-browser-vie.container").write_text("[Container]\n")
    gated = space.gated_units(tmp_path)
    assert gated == ["bipagents-backup.service", "bipagents-browser-vie.service", "bipagents-refresh-browser.service",
                     "bipagents-update.service", "hermes-gateway.service"]
    assert "bipagents-bridge.service" not in gated and "bipagents-pocket.service" not in gated
    assert space.started_units(gated) == ["hermes-gateway.service"]
    written = space.write_drop_ins(gated, tmp_path / "space", tmp_path)
    text = written[0].read_text()
    assert f"ConditionPathExists={tmp_path / 'space' / 'open' / '.bipagents-space'}" in text
    space.remove_drop_ins(tmp_path)
    assert not list((tmp_path / ".config" / "systemd" / "user").glob("*.d"))


def test_the_key_service_from_the_offer_s_setup_and_the_backup_token(tmp_path, monkeypatch):
    monkeypatch.delenv("BIPAGENTS_KEYS_URL", raising=False)
    monkeypatch.delenv("BIPAGENTS_KEYS_TOKEN", raising=False)
    config = tmp_path / ".config" / "bipagents"
    config.mkdir(parents=True)
    (config / "keys.json").write_text(json.dumps({"url": "https://keys.example", "grant": "{}", "signature": "c2ln"}))
    (config / "backup.json").write_text(json.dumps({"url": "https://g", "token": "place-token-0123456789"}))
    found = space.key_service(None, tmp_path)
    assert found == {"BIPAGENTS_KEYS_URL": "https://keys.example", "BIPAGENTS_KEYS_TOKEN": "place-token-0123456789",
                     "BIPAGENTS_KEYS_GRANT": "{}", "BIPAGENTS_KEYS_SIGNATURE": "c2ln"}
    assert space.key_service("https://other", tmp_path)["BIPAGENTS_KEYS_URL"] == "https://other"


def _encrypting(tmp_path, monkeypatch, bridge_code):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr(space, "missing_tools", lambda: None)
    monkeypatch.setattr(space, "bridge_command", lambda base=None: ["/bin/bipbridge"])
    monkeypatch.setenv("BIPAGENTS_KEYS_URL", "https://keys.example")
    monkeypatch.setenv("BIPAGENTS_KEYS_TOKEN", "place-token-0123456789")
    _units(tmp_path, ["hermes-gateway.service", "bipagents-backup.service"])
    calls = []

    def runner(args, env=None):
        calls.append((list(args), env))
        code = bridge_code if args[0] == "/bin/bipbridge" else 0
        return SimpleNamespace(returncode=code, stdout="", stderr="bridge said no" if code else "")

    said = []
    status = space.encrypt(None, say=said.append, runner=runner)
    return status, calls, said


def test_encrypt_stops_everything_has_the_bridge_do_it_and_starts_again(tmp_path, monkeypatch):
    status, calls, said = _encrypting(tmp_path, monkeypatch, 0)
    assert status == 0
    commands = [c[0] for c in calls]
    stop = commands.index(["systemctl", "--user", "stop", "bipagents-bridge.service", "bipagents-backup.service",
                           "hermes-gateway.service"])
    enable = next(i for i, c in enumerate(commands) if c[0] == "/bin/bipbridge")
    assert stop < enable
    assert commands[enable][:3] == ["/bin/bipbridge", "space", "enable"]
    assert commands[enable][-2:] == ["--unit", "hermes-gateway.service"]
    # the token never on the command line
    assert "place-token-0123456789" not in " ".join(commands[enable])
    assert calls[enable][1]["BIPAGENTS_KEYS_TOKEN"] == "place-token-0123456789"
    assert commands[-1] == ["systemctl", "--user", "start", "bipagents-bridge.service", "hermes-gateway.service"]
    assert (tmp_path / ".config/systemd/user/hermes-gateway.service.d" / space.DROP_IN).exists()


def test_a_failure_removes_the_drop_ins_and_starts_everything_as_before(tmp_path, monkeypatch):
    status, calls, said = _encrypting(tmp_path, monkeypatch, 1)
    assert status == 1
    assert not (tmp_path / ".config/systemd/user/hermes-gateway.service.d").exists()
    assert calls[-1][0] == ["systemctl", "--user", "start", "bipagents-bridge.service", "hermes-gateway.service"]
    assert "bridge said no" in said[-1]


def test_the_offer_s_setup_turns_it_on_per_place(tmp_path):
    base = {"v": 1, "pair": "A" * 43, "backup": {"url": "https://g.example", "token": "t" * 32}}
    path = tmp_path / "cloud.json"
    path.write_text(json.dumps(base))
    assert cloud.read(path).keys is None   # off unless the guichet asks
    path.write_text(json.dumps({**base, "keys": {"url": "https://keys.example", "grant": "{}", "signature": "c2ln"}}))
    assert cloud.read(path).keys == {"url": "https://keys.example", "grant": "{}", "signature": "c2ln"}
    for wrong in ({"url": "http://keys.example", "grant": "{}", "signature": "s"}, {"url": "https://k"}, "x"):
        path.write_text(json.dumps({**base, "keys": wrong}))
        with pytest.raises(cloud.CloudError):
            cloud.read(path)
    path.write_text(json.dumps({"v": 1, "pair": "A" * 43, "keys": {"url": "https://k", "grant": "{}", "signature": "s"}}))
    with pytest.raises(cloud.CloudError):   # its token is the backups' one
        cloud.read(path)


def test_the_hardening_script_is_ascii_and_valid_bash():
    path = ROOT / "installer" / "cloud" / "space-hardening.sh"
    text = path.read_text(encoding="utf-8")
    assert text.isascii() and subprocess.run(["bash", "-n", str(path)]).returncode == 0
    assert "Storage=none" in text and "Storage=volatile" in text and "/dev/urandom swap" in text
