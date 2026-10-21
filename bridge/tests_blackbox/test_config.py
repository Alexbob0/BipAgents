"""The config file as the bridge reads it (`BRIDGE_CONFIG`, `--config`), seen through `bipbridge check` and `serve`:
its errors (message on stderr, exit code 2), its warnings (JSON logs), secrets kept in files (tests/test_config.py,
over the CLI)."""
import json
import os

from bbkit.common import AUTH, BRIDGE_KEY


def check(bridge, *extra):
    out = bridge.cli("check", *extra)
    return out.returncode, out.stdout, out.stderr.strip()


def broken(bridges, name, change=None, text=None):
    bridge = bridges.new(name, change=change)
    bridge.write_config()
    if text is not None:
        bridge.config_path.write_text(text)
    return bridge


def test_config_errors_are_said_and_stop_the_bridge(bridges, fakes):
    cases = [
        (lambda c: c.pop("bridge_key"), "bridge.bridge_key (or bridge_key_file) is required"),
        (lambda c: c.update(bridge_key="CHANGE-ME-run-python-m-bipbridge-genkey"),
         "bridge.bridge_key still holds a placeholder value"),
        (lambda c: c["agents"]["Wellness"].pop("hermes_url"), "agents.wellness.hermes_url is required"),
        (lambda c: c["agents"]["Wellness"].pop("hermes_key"), "agents.wellness.hermes_key (or hermes_key_file) is required"),
        (lambda c: c["agents"]["Wellness"].update(hermes_key="<the key>"), "agents.wellness.hermes_key still holds a placeholder value"),
        (lambda c: c["agents"]["Wellness"].pop("upload_dir_container"),
         "agents.wellness: upload_dir_host and upload_dir_container go together"),
        (lambda c: c["apns"].update(environment="dev"), "apns.environment must be 'sandbox' or 'production'"),
        (lambda c: c.update(spaces={"sandbox": "maybe"}), "spaces.sandbox must be 'auto' or 'none'"),
        (lambda c: c.update(browser={"runtime": "lxc"}),
         "browser.runtime must be 'podman' or 'docker', browser.gpu 'auto' or 'none'"),
        (lambda c: c.update(push={"relay_url": "https://admin:8643"}), "push.relay_key (or relay_key_file) is required"),
    ]
    for index, (change, message) in enumerate(cases):
        bridge = broken(bridges, f"case{index}", change)
        code, _, err = check(bridge)
        assert (code, err) == (2, f"config error: {message}"), message
    bridge = broken(bridges, "serve", cases[2][0])
    bridge.start(wait=False)
    assert bridge.process.wait(timeout=30) == 2
    assert "config error: agents.wellness.hermes_url is required" in bridge.log_tail()


def test_a_missing_or_invalid_file(bridges):
    missing = bridges.new("missing")
    code, _, err = check(missing)
    assert code == 2 and err == f"config error: config file not found: {missing.config_path}"
    bad = broken(bridges, "bad", text="bridge_key = \n")
    code, _, err = check(bad)
    assert code == 2 and err.startswith(f"config error: invalid TOML in {bad.config_path}: ")
    table = broken(bridges, "table", text=f'bridge_key = "{BRIDGE_KEY}"\n[pocket]\nurl = "http://p"\n[agents]\nvie = 3\n')
    assert check(table)[2] == "config error: agents.vie must be a table"


def test_secrets_kept_in_files_and_the_config_flag(bridges, fakes):
    bridge = bridges.new()
    key_file = bridge.root / "bridge.key"
    key_file.write_text(BRIDGE_KEY + "\n")
    hermes_file = bridge.root / "wellness.key"
    hermes_file.write_text(bridge.config["agents"]["Wellness"].pop("hermes_key"))
    bridge.config.pop("bridge_key")
    bridge.config["bridge_key_file"] = str(key_file)
    bridge.config["agents"]["Wellness"]["hermes_key_file"] = str(hermes_file)
    bridge.start()
    assert bridge.get("/v1/agents", headers=AUTH).status_code == 200
    assert bridge.get("/v1/status", headers=AUTH).json()["agents"]["wellness"] == {"ok": True}   # its key, from the file
    elsewhere = bridge.root / "other.toml"
    elsewhere.write_text(bridge.config_path.read_text())
    os.chmod(elsewhere, 0o600)
    out = bridge.cli("check", "--config", str(elsewhere))
    assert out.returncode == 0 and out.stdout.splitlines()[0] == f"config OK: {elsewhere}"
    key_file.unlink()
    code, _, err = check(bridge)
    assert code == 2 and err.startswith(f"config error: bridge.bridge_key_file: cannot read {key_file}: ")


def test_warnings_are_logged(bridges):
    bridge = bridges.new(change=lambda c: c.update(bridge_key="k" * 20))
    bridge.write_config()
    os.chmod(bridge.config_path, 0o644)
    out = bridge.cli("check")
    assert out.returncode == 0
    logged = [json.loads(line) for line in out.stdout.splitlines() if line.startswith("{")]
    assert {"msg": "bridge_key is shorter than 32 characters", "level": "warning", "logger": "bipbridge.config"}.items() \
        <= next(e for e in logged if e["msg"].startswith("bridge_key")).items()
    perms = next(e for e in logged if e["msg"].startswith("config file is readable"))
    assert perms == {**perms, "msg": "config file is readable by group/others; run chmod 600", "level": "warning",
                     "path": str(bridge.config_path), "mode": "0o644"}


def test_defaults(bridges, fakes):
    bridge = bridges.new(change=lambda c: (c.pop("port"), c.pop("host")))
    bridge.write_config()
    code, out, _ = check(bridge)
    assert code == 0 and "  listen      127.0.0.1:8643" in out.splitlines()
    assert "  outbox db   " + str(bridge.data / "bridge.db") in out.splitlines()
    assert len(bridge.cli("genkey").stdout.strip()) == 43
