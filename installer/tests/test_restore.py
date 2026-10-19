"""A place moved after an outage: the backup of the dead one (another account, other ports, another door) put back
over a fresh install, with the key the phone gives back."""
import json
import shutil
import sqlite3

import pytest

from bipinstall import backup

T = 1_790_000_000.0


def toml(home, port, door, address, browser_first, setup=True):
    text = (f'bridge_key = "old-bridge-key"\nhost = "127.0.0.1"\nport = {port}\ndata_dir = "{home / "data"}"\n'
            f'cloud = true\npublic_url = "https://{address}' + ("" if door == 443 else f":{door}") + '"\n\n'
            f'[hermes]\nbin = "{home}/.local/bin/hermes"\nhome = "{home / ".hermes"}"\n'
            f'hub_url = "http://127.0.0.1:{port - 1}"\n\n[browser]\nenabled = true\nfirst_port = {browser_first}\n'
            f'ports = 10\n\n[media.roots]\n"{home}/.hermes/media" = "{home}/.hermes/media"\n\n'
            f'[lan]\nenabled = true\nport = {door}\naddress = "{address}"\n')
    if setup:
        text += (f'\n[setup]\nfile = "{home}/.config/bipagents/setup.json"\n'
                 f'progress = "{home}/.config/bipagents/setup-progress.json"\n')
    text += (f'\n[agents.coach]\ndisplay_name = "Coach"\nhermes_url = "http://127.0.0.1:{port - 1}/p/coach"\n'
             f'hermes_key = "agent-key"\n')
    return text


def make_home(root, name, port, door, address, browser_first):
    home = root / name
    config = home / ".config" / "bipagents"
    config.mkdir(parents=True)
    (home / "data" / "lan").mkdir(parents=True)
    (home / ".hermes" / "memories").mkdir(parents=True)
    (config / "bridge.toml").write_text(toml(home, port, door, address, browser_first))
    (home / ".hermes" / ".env").write_text(f"API_SERVER_PORT={port - 1}\n")
    return home


@pytest.mark.skipif(shutil.which("restic") is None, reason="no restic here")
def test_a_new_place_gets_the_dead_one_s_agents_with_its_own_door(tmp_path, monkeypatch):
    repo_dir = tmp_path / "repo"
    old = make_home(tmp_path, "bip3", 8673, 445, "203.0.113.5", 9620)
    (old / ".hermes" / "memories" / "MEMORY.md").write_text("Le chat s'appelle Pixel.\n")
    (old / "data" / "lan" / "cert.pem").write_text("OLD CERTIFICATE")
    db = sqlite3.connect(old / ".hermes" / "state.db")
    db.execute("CREATE TABLE messages (text TEXT)")
    db.execute("INSERT INTO messages VALUES ('rappelle-moi le vétérinaire')")
    db.commit()
    db.close()
    monkeypatch.setenv("HOME", str(old))
    key = backup.configure(str(repo_dir), "old-seat-token")["password"]
    backup.run(old, now=T)

    new = make_home(tmp_path, "bip", 8643, 443, "198.51.100.7", 9600)
    (new / ".config" / "bipagents" / "setup.json").write_text('{"pair": "fresh"}')
    (new / "data" / "lan" / "cert.pem").write_text("FRESH CERTIFICATE")
    monkeypatch.setenv("HOME", str(new))
    backup.configure(str(repo_dir), "new-seat-token")
    with pytest.raises(backup.BackupError, match="doesn't open"):
        backup.fetch(new, "not the key")
    fetched = backup.fetch(new, key)
    info = backup.put_back(fetched, new, tmp_path / "checkout", "198.51.100.7")
    assert info["roots"]["hermes"] == str(old / ".hermes")
    assert (new / ".hermes" / "memories" / "MEMORY.md").read_text() == "Le chat s'appelle Pixel.\n"
    assert (new / "data" / "lan" / "cert.pem").read_text() == "OLD CERTIFICATE"      # the app's pin holds
    rows = sqlite3.connect(new / ".hermes" / "state.db").execute("SELECT text FROM messages").fetchall()
    assert rows == [("rappelle-moi le vétérinaire",)]
    text = (new / ".config" / "bipagents" / "bridge.toml").read_text()
    assert 'bridge_key = "old-bridge-key"' in text                                    # the phones' key
    assert "port = 8643\n" in text and 'hub_url = "http://127.0.0.1:8642"' in text   # this account's ports
    assert 'hermes_url = "http://127.0.0.1:8642/p/coach"' in text
    assert "[lan]\nenabled = true\nport = 443\naddress = \"198.51.100.7\"" in text     # this place's door
    assert 'public_url = "https://198.51.100.7"' in text and "first_port = 9600" in text
    others = [line for line in text.splitlines() if str(old) in line]
    assert others == [f'"{old}/.hermes/media" = "{new}/.hermes/media"']   # old MEDIA: lines still found
    assert f'file = "{new}/.config/bipagents/setup.json"' in text
    assert (new / ".hermes" / ".env").read_text() == "API_SERVER_PORT=8642\n"
    kept = backup.settings(new)
    assert kept["token"] == "new-seat-token" and kept["password"] == key              # its own seat, the person's key


def test_the_restore_says_its_steps_and_its_failure(tmp_path):
    backup.progress(tmp_path, 2)
    state = json.loads((tmp_path / ".config" / "bipagents" / "setup-progress.json").read_text())
    assert state == {"step": 2, "total": 4, "label": "Putting your agents back", "done": False, "restoring": True,
                     "error": None}
    backup.progress(tmp_path, 0, error="this key doesn't open these backups")
    state = json.loads((tmp_path / ".config" / "bipagents" / "setup-progress.json").read_text())
    assert state["error"] == state["label"] == "this key doesn't open these backups" and not state["done"]
