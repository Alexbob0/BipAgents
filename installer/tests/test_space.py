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


def test_the_key_service_token_is_the_setup_s_own_never_the_backups(tmp_path, monkeypatch):
    """Audit 2026-10-27, 17: the key service's token stays in clear beside the encrypted folder; it is a token of its
    own (the setup's keys.token), never the backups' one, which opens the place's backups and mailboxes."""
    monkeypatch.delenv("BIPAGENTS_KEYS_URL", raising=False)
    monkeypatch.delenv("BIPAGENTS_KEYS_TOKEN", raising=False)
    config = tmp_path / ".config" / "bipagents"
    config.mkdir(parents=True)
    (config / "backup.json").write_text(json.dumps({"url": "https://g", "token": "place-token-0123456789"}))
    (config / "keys.json").write_text(json.dumps({"url": "https://keys.example", "grant": "{}", "signature": "c2ln"}))
    found = space.key_service(None, tmp_path)
    assert found["BIPAGENTS_KEYS_TOKEN"] == ""   # no fallback to the backups' token: encrypt refuses
    (config / "keys.json").write_text(json.dumps({"url": "https://keys.example", "grant": "{}", "signature": "c2ln",
                                                  "token": "keys-token-0123456789"}))
    found = space.key_service(None, tmp_path)
    assert found == {"BIPAGENTS_KEYS_URL": "https://keys.example", "BIPAGENTS_KEYS_TOKEN": "keys-token-0123456789",
                     "BIPAGENTS_KEYS_GRANT": "{}", "BIPAGENTS_KEYS_SIGNATURE": "c2ln"}
    assert space.key_service("https://other", tmp_path)["BIPAGENTS_KEYS_URL"] == "https://other"


def test_encrypt_refuses_without_a_key_service_token_of_its_own(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr(space, "missing_tools", lambda: None)
    monkeypatch.setenv("BIPAGENTS_KEYS_URL", "https://keys.example")
    monkeypatch.delenv("BIPAGENTS_KEYS_TOKEN", raising=False)
    config = tmp_path / ".config" / "bipagents"
    config.mkdir(parents=True)
    (config / "backup.json").write_text(json.dumps({"url": "https://g", "token": "place-token-0123456789"}))
    said, ran = [], []
    assert space.encrypt(None, say=said.append, runner=lambda *a, **k: ran.append(a)) == 1
    assert ran == [] and "key-service token" in said[-1]


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
    keys = {"url": "https://keys.example", "grant": "{}", "signature": "c2ln", "token": "k" * 43}
    path = tmp_path / "cloud.json"
    path.write_text(json.dumps(base))
    assert cloud.read(path).keys is None   # off unless the guichet asks
    path.write_text(json.dumps({**base, "keys": keys}))
    assert cloud.read(path).keys == keys
    for wrong in ({"url": "http://keys.example", "grant": "{}", "signature": "s", "token": "k" * 43},
                  {"url": "https://k"}, "x",
                  # an older guichet's setup, without a token of its own: encryption refused, never the backups'
                  {"url": "https://keys.example", "grant": "{}", "signature": "c2ln"},
                  dict(keys, token="short"), dict(keys, token="t" * 32)):
        path.write_text(json.dumps({**base, "keys": wrong}))
        with pytest.raises(cloud.CloudError):
            cloud.read(path)
    path.write_text(json.dumps({"v": 1, "pair": "A" * 43, "keys": keys}))   # its token is its own: backups apart
    assert cloud.read(path).keys == keys


def test_the_hardening_script_is_ascii_and_valid_bash():
    path = ROOT / "installer" / "cloud" / "space-hardening.sh"
    text = path.read_text(encoding="utf-8")
    assert text.isascii() and subprocess.run(["bash", "-n", str(path)]).returncode == 0
    assert "Storage=none" in text and "Storage=volatile" in text and "/dev/urandom swap" in text
