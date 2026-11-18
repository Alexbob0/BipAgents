"""Review offer-cards-on (18/11): R1 (a restore keeps the card switch) and R2 (a [payments] header with a comment)."""
import json
import shutil

import pytest

from bipinstall import backup, bridgeconf, hermes, migrations
from test_restore import make_home


def _runner(ran):
    runner = hermes.Runner()
    runner.dry = False
    runner.run = lambda args, **kw: ran.append(list(args)) or ""
    return runner


def test_a_payments_header_with_a_comment_is_not_a_crash_of_the_update(tmp_path, monkeypatch):
    """R2: `[payments]  # ...` (valid TOML) is not seen as the table: a second [payments] is appended, bridgeconf
    refuses it and BridgeConfigError escapes migrations.run (the whole update stops) instead of NotYet / a string."""
    from bipinstall import drain
    monkeypatch.setattr(drain, "until_quiet", lambda path, what: "")
    config = tmp_path / ".config" / "bipagents" / "bridge.toml"
    config.parent.mkdir(parents=True)
    (config.parent / "backup.json").write_text(json.dumps({"url": "https://g.example", "token": "t"}))
    config.write_text('bridge_key = "k"\n\n[payments]  # Kerfy\noffer_cards = false\n')
    said = migrations.offer_cards_on(_runner([]), tmp_path)   # raises BridgeConfigError today
    assert "open" in said
    assert bridgeconf.tomllib.loads(config.read_text())["payments"]["offer_cards"] is True


@pytest.mark.skipif(shutil.which("restic") is None, reason="no restic here")
def test_a_place_restored_from_a_backup_made_before_the_switch_keeps_cards_available(tmp_path, monkeypatch):
    """R1: a fresh place (stack.py wrote offer_cards = true, every migration marked done) gets a dead place's backup
    whose bridge.toml has no [payments] (made before its update wave). put_back puts the old bridge.toml over the
    fresh one, FRESH_WINS doesn't keep [payments], and the migration is already marked done: cards stay off there."""
    monkeypatch.setattr(migrations, "STATE", tmp_path / "migrations.json")
    repo_dir = tmp_path / "repo"
    old = make_home(tmp_path, "bip3", 8673, 445, "203.0.113.5", 9620)
    monkeypatch.setenv("HOME", str(old))
    key = backup.configure(str(repo_dir), "old-seat-token")["password"]
    backup.run(old, now=1_790_000_000.0)

    new = make_home(tmp_path, "bip", 8643, 443, "198.51.100.7", 9600)
    fresh = new / ".config" / "bipagents" / "bridge.toml"
    fresh.write_text(fresh.read_text() + "\n[payments]\noffer_cards = true\n")   # stack.py, fresh --cloud install
    (new / ".config" / "bipagents" / "setup.json").write_text('{"pair": "fresh"}')
    monkeypatch.setenv("HOME", str(new))
    backup.configure(str(repo_dir), "new-seat-token")
    ran = []
    runner = _runner(ran)
    migrations.mark([mid for mid, _ in migrations.MIGRATIONS], runner)   # __main__.cloud_install
    backup.put_back(backup.fetch(new, key), new, tmp_path / "checkout", "198.51.100.7")
    for _line in migrations.run(runner, new):   # what restore() runs next
        pass
    doc = bridgeconf.tomllib.loads(fresh.read_text())
    assert (doc.get("payments") or {}).get("offer_cards") is True


@pytest.mark.skipif(shutil.which("restic") is None, reason="no restic here")
def test_p2_a_restore_whose_old_payments_header_has_a_comment_still_restores(tmp_path, monkeypatch):
    """Passe 2: FRESH_WINS now sets [payments] offer_cards through moving.set_in_section, which only finds the exact
    line `[payments]`. An old bridge.toml with `[payments]  # Kerfy` (the shape R2 handles in the migration) gets a
    second [payments]: bridgeconf refuses it and settle raises, the restore stops."""
    monkeypatch.setattr(migrations, "STATE", tmp_path / "migrations.json")
    repo_dir = tmp_path / "repo"
    old = make_home(tmp_path, "bip3", 8673, 445, "203.0.113.5", 9620)
    cfg_old = old / ".config" / "bipagents" / "bridge.toml"
    cfg_old.write_text(cfg_old.read_text() + "\n[payments]  # Kerfy\noffer_cap_max_eur = 80\n")
    monkeypatch.setenv("HOME", str(old))
    key = backup.configure(str(repo_dir), "old-seat-token")["password"]
    backup.run(old, now=1_790_000_000.0)
    new = make_home(tmp_path, "bip", 8643, 443, "198.51.100.7", 9600)
    fresh = new / ".config" / "bipagents" / "bridge.toml"
    fresh.write_text(fresh.read_text() + "\n[payments]\noffer_cards = true\n")
    (new / ".config" / "bipagents" / "setup.json").write_text('{"pair": "fresh"}')
    monkeypatch.setenv("HOME", str(new))
    backup.configure(str(repo_dir), "new-seat-token")
    backup.put_back(backup.fetch(new, key), new, tmp_path / "checkout", "198.51.100.7")   # raises today
    doc = bridgeconf.tomllib.loads(fresh.read_text())
    assert doc["payments"] == {"offer_cards": True, "offer_cap_max_eur": 80}


@pytest.mark.skipif(shutil.which("restic") is None, reason="no restic here")
def test_p2_a_restore_that_cant_set_the_switch_goes_on_and_the_next_update_tries_again(tmp_path, monkeypatch):
    """Passe 2: an old [payments] the edit can't change (a dotted key): the restored text kept, the restore finishes,
    and offer-cards-on is pending again although the fresh install marked every migration done."""
    monkeypatch.setattr(migrations, "STATE", tmp_path / "migrations.json")
    repo_dir = tmp_path / "repo"
    old = make_home(tmp_path, "bip3", 8673, 445, "203.0.113.5", 9620)
    cfg_old = old / ".config" / "bipagents" / "bridge.toml"
    first, rest = cfg_old.read_text().split("\n", 1)
    cfg_old.write_text(first + "\npayments.offer_cards = false\n" + rest)
    monkeypatch.setenv("HOME", str(old))
    key = backup.configure(str(repo_dir), "old-seat-token")["password"]
    backup.run(old, now=1_790_000_000.0)
    new = make_home(tmp_path, "bip", 8643, 443, "198.51.100.7", 9600)
    fresh = new / ".config" / "bipagents" / "bridge.toml"
    fresh.write_text(fresh.read_text() + "\n[payments]\noffer_cards = true\n")
    (new / ".config" / "bipagents" / "setup.json").write_text('{"pair": "fresh"}')
    monkeypatch.setenv("HOME", str(new))
    backup.configure(str(repo_dir), "new-seat-token")
    migrations.mark([mid for mid, _ in migrations.MIGRATIONS], _runner([]))
    backup.put_back(backup.fetch(new, key), new, tmp_path / "checkout", "198.51.100.7")   # doesn't raise
    assert "payments.offer_cards = false" in fresh.read_text()
    assert migrations.pending() == [migrations.OFFER_CARDS_ON]
