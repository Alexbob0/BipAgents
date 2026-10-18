import json
import os
import sqlite3
import tomllib
from pathlib import Path
from types import SimpleNamespace

import pytest

from bipinstall import hermes, moving, sealed

OLD_NAME, NEW_NAME = "aibox.tail1234.ts.net", "newbox.tail1234.ts.net"


def database(path: Path, rows):
    path.parent.mkdir(parents=True, exist_ok=True)
    with sqlite3.connect(path) as db:
        db.execute("PRAGMA journal_mode=WAL")
        db.execute("CREATE TABLE t (v TEXT)")
        db.executemany("INSERT INTO t VALUES (?)", [(r,) for r in rows])


def write(path: Path, text: str, mode: int = 0o644):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    os.chmod(path, mode)


@pytest.fixture
def source(tmp_path, monkeypatch):
    """An install as the installer leaves it, in the home of « alex », with two agents (one in its own space) and a
    browser."""
    home = tmp_path / "home" / "alex"
    data, hub = home / ".local" / "share" / "bipagents", home / ".hermes"
    config = home / ".config" / "bipagents" / "bridge.toml"
    write(config, f'''bridge_key = "k"
port = 8643
data_dir = "{data}"
public_url = "https://{OLD_NAME}:8643"

[hermes]
bin = "{home}/.local/bin/hermes"
home = "{hub}"
hub_url = "http://127.0.0.1:8642"
public_hub_url = "https://{OLD_NAME}:8642"
isolated = true

[media.roots]
"{hub}/media" = "{hub}/media"

[apns]
p8_path = "{home}/keys/AuthKey.p8"

[lan]
enabled = true
port = 8644
address = "192.168.1.20"

[agents.vie]
display_name = "Vie"
hermes_url = "http://127.0.0.1:8642/p/vie"
hermes_key = "vie-key"
upload_dir_host = "{hub}/profiles/vie/uploads"
public_url = "https://{OLD_NAME}:8642/p/vie"
desktop_vnc = "tcp://127.0.0.1:9601"
desktop_vnc_password_file = "{data}/browsers/vie/vnc.pass"

[agents.coach]
display_name = "Coach"
hermes_url = "http://127.0.0.1:8660/p/coach"
hermes_key = "coach-key"
space = "{data}/spaces/coach"
''', 0o600)
    write(home / "keys" / "AuthKey.p8", "-----BEGIN PRIVATE KEY-----", 0o600)
    write(hub / "config.yaml", "model:\n  base_url: http://192.168.1.30:8888/v1\nbot_peers:\n  bip:\n    url: http://127.0.0.1:8643/peer\n")
    write(hub / ".env", "API_SERVER_PORT=8642\nAPI_SERVER_KEY=hub\nHERMES_DOCKER_BINARY=/usr/bin/podman\n", 0o600)
    write(hub / "profiles" / "vie" / "SOUL.md", f"Tu es Vie. Tes fichiers sont dans {home}/notes.")
    write(hub / "profiles" / "vie" / "config.yaml",
          "mcp_servers:\n  bipagents:\n    url: http://127.0.0.1:8643/v1/agent-tools/mcp\n")
    database(hub / "profiles" / "vie" / "state.db", ["a conversation", f"MEDIA:{hub}/media/photo.jpg"])
    write(hub / "profiles" / "vie" / "sessions" / "s1.json", json.dumps({"text": f"see {home}/notes"}))
    write(hub / "profiles" / "vie" / "memories" / "MEMORY.md", "Alex likes trains.")
    write(hub / "profiles" / "vie" / "cron" / "jobs.json", json.dumps({"jobs": [{"id": "morning"}]}))
    write(hub / "media" / "photo.jpg", "jpeg")
    os.symlink(hub / "media" / "photo.jpg", hub / "profiles" / "vie" / "last-photo.jpg")
    # What is not moved: Hermes' code and runtimes, caches, logs, a running gateway's files.
    for skipped in ("hermes-agent/run_agent.py", "tools/python-3.11/bin/python3", "cache/x", "logs/gateway.log",
                    "gateway.pid", "profiles/vie/sandboxes/box/x", "profiles/vie/state.db-wal-copy"):
        write(hub / skipped, "runtime")
    # The bridge's data, a space, a browser.
    database(data / "bridge.db", ["device-token"])
    write(data / "vault" / "vault.key", "fernet", 0o600)
    write(data / "migrations.json", json.dumps({"done": ["2026-10-15-local-compression"]}))
    write(data / "spaces" / "coach" / ".env", "API_SERVER_PORT=8660\nHERMES_DOCKER_BINARY=/usr/bin/podman\n", 0o600)
    write(data / "spaces" / "coach" / "profiles" / "coach" / "SOUL.md", "Tu es Coach.")
    write(data / "spaces" / "coach" / "hermes-agent" / "x", "code")
    browser = data / "browsers" / "vie"
    write(browser / "ports.json", json.dumps({"cdp": 9600, "vnc": 9601}))
    write(browser / "vnc.pass", "secret", 0o600)
    write(browser / "busy", "")
    write(browser / "mcp.json", json.dumps({"busy": str(browser / "busy"), "server": ["/usr/bin/podman"]}))
    database(browser / "profile" / "Default" / "Cookies", ["session=1"])
    write(browser / "profile" / "Default" / "Cache" / "data_0", "cache")
    write(browser / "profile" / "SingletonLock", "lock")
    monkeypatch.setattr(hermes, "sandbox_runtime", lambda: "/usr/bin/podman")
    monkeypatch.setattr(hermes, "hermes_checkout", lambda: tmp_path / "no-git")
    return home, config


def move(source, tmp_path, ports=None, lan="192.168.1.50"):
    home, config = source
    inv = moving.read_inventory(config, home)
    machine = SimpleNamespace(system="linux", tailnet_name=OLD_NAME, tailscale=None)
    phrase = sealed.new_passphrase()
    out = tmp_path / "export.bipagents"
    info, tally = moving.export(inv, out, phrase, machine)
    new_home = tmp_path / "home" / "bip"
    target = moving.Target(home=new_home, repo=moving.REPO, tailnet_name=NEW_NAME, lan_address=lan,
                           ports=ports or {}, sandbox="/usr/bin/podman")
    tar, opened, handle = moving.open_archive(out, phrase)
    summary, changed = moving.unpack(tar, opened, target, new_home / ".config" / "bipagents" / "bridge.toml")
    tar.close()
    handle.close()
    return info, tally, summary, new_home, out


def test_an_install_moves_to_another_home_name_and_ports(source, tmp_path):
    home, _ = source
    info, tally, summary, new, out = move(source, tmp_path, ports={8642: 9202, 8643: 9203, 8644: 9204})
    assert oct(out.stat().st_mode & 0o777) == "0o600" and summary["files"] == tally.files and not tally.skipped
    assert info["ports"] == {"hub": 8642, "bridge": 8643, "lan": 8644, "desk": 8645}
    assert info["published"] == [8642, 8643] and info["isolated"] and info["browsers"]
    assert {a["name"]: a["space_port"] for a in info["agents"]} == {"vie": None, "coach": 8660}
    hub, data = new / ".hermes", new / ".local" / "share" / "bipagents"

    # Everything the agents need is there: memory, routines, conversations, files, keys, the vault, logins.
    assert (hub / "profiles" / "vie" / "memories" / "MEMORY.md").read_text() == "Alex likes trains."
    assert json.loads((hub / "profiles" / "vie" / "cron" / "jobs.json").read_text())["jobs"][0]["id"] == "morning"
    with sqlite3.connect(hub / "profiles" / "vie" / "state.db") as db:
        assert [r[0] for r in db.execute("SELECT v FROM t")][0] == "a conversation"
    with sqlite3.connect(data / "browsers" / "vie" / "profile" / "Default" / "Cookies") as db:
        assert db.execute("SELECT v FROM t").fetchone()[0] == "session=1"
    assert (data / "vault" / "vault.key").read_text() == "fernet"
    assert oct((data / "vault" / "vault.key").stat().st_mode & 0o777) == "0o600"
    assert (new / "keys" / "AuthKey.p8").exists()                       # a key the config points to, outside
    assert os.readlink(hub / "profiles" / "vie" / "last-photo.jpg") == str(hub / "media" / "photo.jpg")
    assert (data / "spaces" / "coach" / "profiles" / "coach" / "SOUL.md").exists()
    # Not what the machine makes again, nor its caches.
    for gone in ("hermes-agent", "tools", "cache", "logs", "gateway.pid", "profiles/vie/sandboxes"):
        assert not (hub / gone).exists(), gone
    assert not (data / "spaces" / "coach" / "hermes-agent").exists()
    assert not (data / "browsers" / "vie" / "busy").exists()
    assert not (data / "browsers" / "vie" / "profile" / "Default" / "Cache").exists()
    assert not (data / "browsers" / "vie" / "profile" / "SingletonLock").exists()

    # The settings follow: the new home, this machine's tailnet name, its ports, its local network.
    bridge = tomllib.loads((new / ".config" / "bipagents" / "bridge.toml").read_text())
    assert bridge["port"] == 9203 and bridge["data_dir"] == str(data)
    assert bridge["public_url"] == f"https://{NEW_NAME}:9203"
    assert bridge["hermes"]["hub_url"] == "http://127.0.0.1:9202" and bridge["hermes"]["home"] == str(hub)
    assert bridge["agents"]["vie"]["hermes_url"] == "http://127.0.0.1:9202/p/vie"
    assert bridge["agents"]["vie"]["public_url"] == f"https://{NEW_NAME}:9202/p/vie"
    assert bridge["agents"]["coach"]["hermes_url"] == "http://127.0.0.1:8660/p/coach"   # its space keeps its port
    assert bridge["agents"]["coach"]["space"] == str(data / "spaces" / "coach")
    assert bridge["apns"]["p8_path"] == str(new / "keys" / "AuthKey.p8")
    assert bridge["lan"] == {"enabled": True, "port": 9204, "address": "192.168.1.50"}
    # Old conversations name files by their old path: found in their new place.
    assert bridge["media"]["roots"] == {f"{hub}/media": f"{hub}/media", f"{home}/.hermes/media": f"{hub}/media"}
    assert "API_SERVER_PORT=9202" in (hub / ".env").read_text()
    assert "url: http://127.0.0.1:9203/peer" in (hub / "config.yaml").read_text()
    assert "http://127.0.0.1:9203/v1/agent-tools/mcp" in (hub / "profiles" / "vie" / "config.yaml").read_text()
    assert "API_SERVER_PORT=8660" in (data / "spaces" / "coach" / ".env").read_text()
    assert json.loads((data / "browsers" / "vie" / "mcp.json").read_text())["busy"] == str(data / "browsers" / "vie" / "busy")
    # What the person and their agents wrote stays word for word.
    assert str(home) in (hub / "profiles" / "vie" / "sessions" / "s1.json").read_text()
    assert str(home) in (hub / "profiles" / "vie" / "SOUL.md").read_text()
    assert "http://192.168.1.30:8888/v1" in (hub / "config.yaml").read_text()   # the model: checked, not changed


def test_a_rented_server_has_no_local_network_door_and_same_ports_stay(source, tmp_path):
    info, tally, summary, new, out = move(source, tmp_path, lan=None)
    bridge = tomllib.loads((new / ".config" / "bipagents" / "bridge.toml").read_text())
    assert bridge["lan"] == {"enabled": False, "port": 8644}
    assert bridge["port"] == 8643 and bridge["hermes"]["hub_url"] == "http://127.0.0.1:8642"


def test_the_wrong_passphrase_unpacks_nothing(source, tmp_path):
    home, config = source
    out = tmp_path / "export.bipagents"
    moving.export(moving.read_inventory(config, home), out, "right",
                  SimpleNamespace(system="linux", tailnet_name=None, tailscale=None))
    with pytest.raises(sealed.SealedError, match="wrong passphrase"):
        moving.open_archive(out, "wrong")


def test_moving_stops_this_install_and_resume_starts_it_again(tmp_path, monkeypatch):
    runner = hermes.Runner()
    runner.dry = False
    ran = []
    runner.run = lambda args, **kw: ran.append(list(args)) or ""
    found = [("systemd", "bipagents-hermes"), ("systemd", "bipagents-bridge"), ("systemd", "bipagents-browser-vie"),
             ("systemd", "bipagents-update.timer"), ("hermes", "gateway")]
    moving.stop(runner, found, tmp_path)
    assert ["systemctl", "--user", "disable", "--now", "bipagents-update.timer"] in ran   # no nightly restart
    assert ran[-1][1:] == ["gateway", "uninstall"]
    ran.clear()
    assert moving.resume(runner, tmp_path) == ["bipagents-hermes", "bipagents-bridge", "bipagents-update.timer", "gateway"]
    assert ["systemctl", "--user", "enable", "--now", "bipagents-bridge"] in ran
    assert not any("bipagents-browser-vie" in c for c in ran)   # browsers start when used
    assert ran[-1][1:] == ["gateway", "install", "--start-now", "--start-on-login"]
    assert moving.resume(runner, tmp_path) == []   # once
