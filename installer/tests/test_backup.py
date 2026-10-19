import json
import os
import shutil
import sqlite3
import subprocess

import pytest

from bipinstall import backup, cloud

T = 1_790_000_000.0


def a_place(tmp_path):
    """An account with a bridge, a Hermes with a memory, a database and a cache, still waiting for its app."""
    home = tmp_path / "bip"
    config = home / ".config" / "bipagents"
    config.mkdir(parents=True)
    hermes_home = home / ".hermes"
    (hermes_home / "memories").mkdir(parents=True)
    (hermes_home / "memories" / "MEMORY.md").write_text("Alex aime le café.\n")
    (hermes_home / "cache").mkdir()
    (hermes_home / "cache" / "big.bin").write_bytes(b"x" * 1000)
    db = sqlite3.connect(hermes_home / "state.db")
    db.execute("PRAGMA journal_mode=WAL")
    db.execute("CREATE TABLE messages (text TEXT)")
    db.execute("INSERT INTO messages VALUES ('bonjour')")
    db.commit()
    (config / "bridge.toml").write_text(f'bridge_key = "k"\ndata_dir = "{home / "data"}"\n\n[hermes]\nhome = '
                                        f'"{hermes_home}"\n')
    (home / "data").mkdir()
    (config / "setup.json").write_text('{"pair": "secret"}')
    return home, db


@pytest.mark.skipif(shutil.which("restic") is None, reason="no restic here")
def test_a_paired_place_is_backed_up_encrypted_without_caches(tmp_path, monkeypatch):
    home, db = a_place(tmp_path)
    monkeypatch.setenv("HOME", str(home))
    data = backup.configure(str(tmp_path / "repo"), "token-of-this-place")
    assert len(data["password"]) >= 40 and (home / backup.SETTINGS).stat().st_mode & 0o777 == 0o600
    assert backup.configure(str(tmp_path / "repo"), "new-token")["password"] == data["password"]   # same key
    assert backup.run(home)["skipped"] == "not paired yet"
    assert not (tmp_path / "repo").exists()
    (home / ".config" / "bipagents" / "setup.json").unlink()     # the app paired it
    first = backup.run(home, now=T)
    assert first["last_ok"] == T and first["snapshot"] and first["pruned"] == T
    db.execute("INSERT INTO messages VALUES ('au revoir')")      # written meanwhile, in the WAL
    db.commit()
    (home / ".hermes" / "memories" / "MEMORY.md").write_text("Alex aime le thé.\n")
    second = backup.run(home, now=T + 3600)
    assert second["snapshot"] != first["snapshot"] and second["pruned"] == T   # thinned once a day
    env = backup.environment(backup.settings(home), home)
    listed = subprocess.run(["restic", "ls", "latest"], env=env, capture_output=True, text=True).stdout
    assert str(home / ".hermes" / "memories" / "MEMORY.md") in listed
    assert str(home / "cache") not in listed and "big.bin" not in listed               # caches stay
    copy = str(home / backup.STAGE / "sqlite" / str(home / ".hermes" / "state.db").lstrip("/"))
    assert copy in listed and f"\n{home / '.hermes' / 'state.db'}\n" not in listed     # the copy, not the live file
    out = tmp_path / "restored"
    subprocess.run(["restic", "restore", "latest", "--target", str(out)], env=env, check=True, capture_output=True)
    restored = sqlite3.connect(out / copy.lstrip("/"))
    assert [r[0] for r in restored.execute("SELECT text FROM messages")] == ["bonjour", "au revoir"]
    assert (out / str(home / ".hermes" / "memories" / "MEMORY.md").lstrip("/")).read_text() == "Alex aime le thé.\n"
    manifest = json.loads((out / str(home / backup.STAGE / "manifest.json").lstrip("/")).read_text())
    assert manifest["roots"]["hermes"] == str(home / ".hermes") and manifest["stage"] == str(home / backup.STAGE)
    state = json.loads((home / backup.STATE).read_text())
    assert state["last_ok"] == T + 3600 and "last_error" not in state
    wrong = dict(backup.settings(home), password="not the key")
    (home / backup.SETTINGS).write_text(json.dumps(wrong))
    with pytest.raises(backup.BackupError):
        backup.run(home, now=T + 7200)
    state = json.loads((home / backup.STATE).read_text())
    assert state["last_ok"] == T + 3600 and state["last_error"] and "not the key" not in state["last_error"]


def test_the_guichet_s_address_carries_the_token_and_the_units_run_hourly_at_low_priority(tmp_path):
    data = {"url": "https://guichet.test/v1/backup/", "token": "tok", "password": "pw"}
    assert backup.repository(data) == "rest:https://place:tok@guichet.test/v1/backup/"
    env = backup.environment(data, tmp_path)
    assert env["RESTIC_PASSWORD"] == "pw" and "tok" in env["RESTIC_REPOSITORY"]
    (service, unit), (timer, text) = backup.units(tmp_path / "repo", tmp_path / "python")
    assert "Nice=19" in unit and "IOSchedulingClass=idle" in unit and "-m bipinstall backup --quiet" in unit
    assert "pw" not in unit and "tok" not in unit                                     # no secret in the unit
    assert "OnCalendar=hourly" in text and "RandomizedDelaySec=50min" in text


def test_the_setup_s_backup_must_be_https_with_a_token(tmp_path):
    path = tmp_path / "cloud.json"
    base = {"v": 1, "pair": "p" * 43, "language": "fr", "profile": "solo"}
    path.write_text(json.dumps({**base, "backup": {"url": "https://guichet.test/v1/backup/", "token": "t" * 43}}))
    assert cloud.read(path).backup == {"url": "https://guichet.test/v1/backup/", "token": "t" * 43}
    path.write_text(json.dumps(base))
    assert cloud.read(path).backup is None
    for bad in ({"url": "http://guichet.test/", "token": "t" * 43}, {"url": "https://g.test/"}, "x"):
        path.write_text(json.dumps({**base, "backup": bad}))
        with pytest.raises(cloud.CloudError):
            cloud.read(path)
