import json

from bipbridge import place_backup
from conftest import AUTH


def test_the_app_gets_the_key_and_when_the_last_backup_worked(client, tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    assert client.get("/v1/backup", headers=AUTH).json() == {"enabled": False}
    assert client.get("/v1/backup").status_code == 401
    (tmp_path / place_backup.SETTINGS).parent.mkdir(parents=True)
    (tmp_path / place_backup.SETTINGS).write_text(json.dumps({"url": "https://g.test/v1/backup/", "token": "t",
                                                              "password": "the-key"}))
    assert client.get("/v1/backup", headers=AUTH).json() == {"enabled": True, "key": "the-key"}
    (tmp_path / place_backup.STATE).parent.mkdir(parents=True)
    (tmp_path / place_backup.STATE).write_text(json.dumps({"last_ok": 1790000000.0, "snapshot": "abc"}))
    status = client.get("/v1/backup", headers=AUTH).json()
    assert status == {"enabled": True, "key": "the-key", "last_ok": 1790000000.0}   # the token never leaves
