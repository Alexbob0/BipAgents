"""Backups as the place is used (backup.py, 2026-10-31): a look every 5 minutes, a backup only when something changed
(once nothing more changed for 5 minutes, or 15 minutes after the last one while the person goes on), one a day all
the same, one place of a server at a time; the units, the migration and what the seat agent tells the guichet."""
import importlib.util
import json
import os
import shutil
import sqlite3
import subprocess
import time
from pathlib import Path

import pytest

from bipinstall import backup

ROOT = Path(__file__).resolve().parents[2]
MIN = 60


def a_place(tmp_path, paired=True):
    """A paired account: a bridge config, a Hermes with a memory, a session database (WAL) and a cache."""
    home = tmp_path / "bip"
    config = home / ".config" / "bipagents"
    config.mkdir(parents=True)
    hermes_home = home / ".hermes"
    (hermes_home / "memories").mkdir(parents=True)
    (hermes_home / "memories" / "MEMORY.md").write_text("Alex aime le café.\n")
    (hermes_home / "cache").mkdir()
    (hermes_home / "cache" / "big.bin").write_bytes(b"x" * 1000)
    (hermes_home / "cron").mkdir()
    (hermes_home / "cron" / ".tick.lock").write_text("")
    db = sqlite3.connect(hermes_home / "state.db")
    db.execute("PRAGMA journal_mode=WAL")
    db.execute("CREATE TABLE messages (text TEXT)")
    db.execute("INSERT INTO messages VALUES ('bonjour')")
    db.commit()
    (home / "data").mkdir()
    (config / "bridge.toml").write_text(f'bridge_key = "k"\ndata_dir = "{home / "data"}"\n\n[hermes]\nhome = '
                                        f'"{hermes_home}"\n')
    (config / "backup.json").write_text(json.dumps({"url": str(tmp_path / "repo"), "token": "t" * 20,
                                                    "password": "the-key-of-this-place"}))
    if not paired:
        (config / "setup.json").write_text('{"pair": "secret"}')
    return home, db


def fp(home):
    from bipinstall import moving
    return backup.fingerprint(moving.read_inventory(home / ".config" / "bipagents" / "bridge.toml", home), home)


def age(path, seconds_ago, now=None):
    at = (now or time.time()) - seconds_ago
    os.utime(path, (at, at))


# -- the decision ----------------------------------------------------------------------------------------------------

def test_the_decision_nothing_changed_activity_going_on_quiet_for_5_minutes_the_day_s_safety():
    T = 1_790_000_000.0
    # Nothing changed: nothing, however long after the last backup, until a day has gone.
    assert backup.decide(T, False, T - 3 * 3600, T - 5 * MIN) == (False, "nothing changed since the last backup")
    assert backup.decide(T, False, T - 30 * 3600, T - 23 * 3600)[0] is False
    assert backup.decide(T, False, T - 30 * 3600, T - 24 * 3600) == (True, "the day's backup (nothing changed)")
    # The person goes on (a change a minute ago): wait, until the last backup is 15 minutes old.
    assert backup.decide(T, True, T - 1 * MIN, T - 10 * MIN) == (False, "still changing")
    assert backup.decide(T, True, T - 1 * MIN, T - 15 * MIN) == (
        True, "still changing, the last backup is 15 minutes old")
    # Quiet since the last change for 5 minutes: at once, however recent the last backup.
    assert backup.decide(T, True, T - 5 * MIN, T - 6 * MIN) == (True, "nothing more changed for 5 minutes")
    assert backup.decide(T, True, T - 3 * MIN, T - 6 * MIN)[0] is False
    # The timer is a little late or early (AccuracySec, the backup's own time): a minute's slack.
    assert backup.decide(T, True, T - 4 * MIN - 30, T - 6 * MIN)[0] is True
    assert backup.decide(T, True, T - 30, T - 14 * MIN - 30)[0] is True
    # Never backed up: now. A failure: tried again a quarter of an hour later, not at every look.
    assert backup.decide(T, False, 0, None) == (True, "first backup")
    assert backup.decide(T, True, T - 20 * MIN, T - 3600, failed_at=T - 5 * MIN)[0] is False
    assert backup.decide(T, True, T - 20 * MIN, T - 3600, failed_at=T - 15 * MIN)[0] is True


# -- the signal --------------------------------------------------------------------------------------------------------

def test_the_fingerprint_sees_what_a_backup_takes_change_and_nothing_else(tmp_path):
    home, db = a_place(tmp_path)
    first = fp(home)
    assert first == fp(home) and first["files"] >= 3
    assert first["newest"] > 0 and "hermes/memories" in first["areas"]
    # Caches, logs, Hermes' cron tick lock rewritten every minute, the bridge's note of the app's last request,
    # sockets and pids: no change.
    (home / ".hermes" / "cache" / "big.bin").write_bytes(b"y" * 2000)
    (home / ".hermes" / "cron" / ".tick.lock").write_text("")
    age(home / ".hermes" / "cron" / ".tick.lock", -5)
    (home / "data" / "sleep.json").write_text('{"last": 1}')
    (home / ".hermes" / "gateway.pid").write_text("123")
    (home / ".hermes" / "logs").mkdir()
    (home / ".hermes" / "logs" / "agent.log").write_text("x")
    assert fp(home)["sum"] == first["sum"]
    # A message written by the gateway (in the WAL first): a change, seen in the Hermes root's area.
    db.execute("INSERT INTO messages VALUES ('au revoir')")
    db.commit()
    second = fp(home)
    assert second["sum"] != first["sum"]
    assert [k for k in second["areas"] if second["areas"][k] != first["areas"].get(k)] == ["hermes/state.db-wal"]
    # A memory rewritten with the same size and an older time: its inode tells (an atomic write).
    memory = home / ".hermes" / "memories" / "MEMORY.md"
    stamp = memory.stat()
    replaced = memory.with_name("MEMORY.tmp")
    replaced.write_text("Alex aime le thé!\n")
    os.utime(replaced, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
    os.replace(replaced, memory)
    third = fp(home)
    assert third["sum"] != second["sum"] and third["areas"]["hermes/memories"] != second["areas"]["hermes/memories"]
    # A file gone, an empty folder made: seen / not.
    (home / ".hermes" / "memories" / "MEMORY.md").unlink()
    assert fp(home)["sum"] != third["sum"]


def test_an_idle_place_s_own_rewrites_are_no_change(tmp_path):
    """What a real place nobody used rewrote between two looks (lab server, 2026-11-02): no backup for those."""
    home, db = a_place(tmp_path)
    hermes_home = home / ".hermes"
    for profile in (hermes_home, hermes_home / "profiles" / "coach"):
        (profile / "cron").mkdir(parents=True, exist_ok=True)
        (profile / "cron" / "ticker_heartbeat").write_text("1793600523.97 36417")
        (profile / "cron" / "ticker_last_success").write_text("1793600523.98")
    (hermes_home / "skills").mkdir()
    (hermes_home / "state").mkdir()
    (hermes_home / "state" / "gateway.heartbeat").write_text('{"pid": 1}')
    (hermes_home / "channel_directory.json").write_text('{"updated_at": "a", "platforms": {}}')
    (home / "data" / "storage.json").write_text('{"rules": {}}')
    (home / "data" / "sandbox-proxy" / "reports").mkdir(parents=True)
    (home / "data" / "sandbox-proxy" / "reports" / "usage.json").write_text('{"at": 1}')
    # data_dir's storage.json is only noise there: the path must end with bipagents/storage.json on a place
    (home / ".config" / "bipagents" / "bridge.toml").write_text(
        (home / ".config" / "bipagents" / "bridge.toml").read_text().replace(str(home / "data"),
                                                                              str(home / "share" / "bipagents")))
    (home / "share").mkdir()
    (home / "data").rename(home / "share" / "bipagents")
    first = fp(home)
    for profile in (hermes_home, hermes_home / "profiles" / "coach"):
        (profile / "cron" / "ticker_heartbeat").write_text("1793600583.97 36417")
        (profile / "cron" / "ticker_last_success").write_text("1793600583.99")
        age(profile / "cron" / "ticker_heartbeat", -60)
        (profile / "cron" / "executions.db-wal").write_text("")   # opened at a tick: an empty WAL appears
        (profile / "skills").mkdir(exist_ok=True)
        (profile / "skills" / ".curator_state").write_text('{"last_run_at": "x"}')   # the curator seeded
    (hermes_home / "state" / "gateway.heartbeat").write_text('{"pid": 1, "updated_at": "b"}')
    (hermes_home / "channel_directory.json").write_text('{"updated_at": "bb", "platforms": {}}')
    data = home / "share" / "bipagents"
    (data / "storage.json").write_text('{"rules": {}, "at": 2}')
    (data / "sandbox-proxy" / "reports" / "usage.json").write_text('{"at": 22}')
    assert fp(home)["sum"] == first["sum"]
    # A WAL with something in it is a change.
    (hermes_home / "cron" / "executions.db-wal").write_bytes(b"x" * 100)
    assert fp(home)["sum"] != first["sum"]


def test_the_look_walks_exactly_what_a_backup_takes(tmp_path):
    """stat_walk asks `Inventory.skipped` only about the names it may skip (backup._may_skip): the same files as the
    backup's own walk, whatever the skipped names' places, plus the databases' write-ahead logs."""
    from bipinstall import moving
    home, _ = a_place(tmp_path)
    hermes_home, data = home / ".hermes", home / "data"
    spaces = data / "spaces"
    names = sorted(moving.HERMES_SKIP_ROOT | moving.HERMES_SKIP_PROFILE | moving.CHROME_SKIP | moving.DATA_SKIP
                   | set(moving.RUNTIME_FILES)) + ["x.sock", "bridge.toml.good", "a-journal", "keep.md"]
    for j, where in enumerate((hermes_home, hermes_home / "profiles" / "vie", hermes_home / "profiles" / "vie" / "skills",
                  spaces / "max", spaces / "max" / "profiles" / "max", data, data / "browsers" / "vie" / "profile",
                  data / "browsers" / "vie" / "profile" / "Default", home / ".config" / "bipagents")):
        for i, name in enumerate(names):   # each name a file here, a folder there
            target = where / name
            if (i + j) % 2:
                target.mkdir(parents=True, exist_ok=True)
                (target / "inside.txt").write_text("x")
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text("x")
    inv = moving.read_inventory(home / ".config" / "bipagents" / "bridge.toml", home)
    backed = {str(p) for root in inv.roots().values() if root.is_dir() for p in backup._walk(inv, root)}
    looked = {path for root in inv.roots().values() if root.is_dir() for path, _, _ in backup.stat_walk(inv, root)}
    assert {p for p in looked if not p.endswith("-wal")} == backed
    assert str(hermes_home / "state.db-wal") in looked and len(backed) > 50


def test_the_agents_own_volumes_count_too(tmp_path):
    home, _ = a_place(tmp_path)
    (home / "data" / "sandbox.json").write_text(json.dumps({"adopted": {"vie": {"volumes": [
        {"name": "bipagents-vie-work", "kind": "work"}, {"name": "bipagents-files-vie", "kind": "files"}]}}}))
    data = home / ".local" / "share" / "containers" / "storage" / "volumes" / "bipagents-vie-work" / "_data"
    data.mkdir(parents=True)
    (data / "plan.md").write_text("étape 1\n")
    other = data.parents[1] / "bipagents-files-vie" / "_data"
    other.mkdir(parents=True)
    before = fp(home)
    assert "volume/bipagents-vie-work" in before["areas"]
    (other / "copy.txt").write_text("a copy the bridge keeps elsewhere")
    assert fp(home)["sum"] == before["sum"]
    (data / "plan.md").write_text("étape 1\nétape 2\n")
    assert fp(home)["sum"] != before["sum"]


# -- the timer's look, without restic: when it backs up ----------------------------------------------------------------

@pytest.fixture
def looks(tmp_path, monkeypatch):
    """A paired place whose backups `run` only notes (as the real one does: the fingerprint kept, last_ok)."""
    home, db = a_place(tmp_path)
    runs = []

    def fake_run(base=None, config=None, now=None):
        found = fp(base)
        state = backup.read_json(base / backup.STATE)
        state.update({"last_ok": now, "snapshot": f"s{len(runs)}"})
        backup.write_private(base / backup.STATE, state)
        looked = backup.read_json(base / backup.CHECK)
        looked["backed"] = {"sum": found["sum"], "areas": found["areas"], "at": now}
        backup.write_private(base / backup.CHECK, looked)
        runs.append(now)
        return state

    monkeypatch.setattr(backup, "run", fake_run)
    return home, db, runs, tmp_path / "server.lock"


def test_a_look_backs_up_every_15_minutes_while_it_goes_on_5_minutes_after_it_stops_and_once_a_day(looks):
    home, db, runs, lock = looks
    memory = home / ".hermes" / "memories" / "MEMORY.md"
    T = time.time()
    for path in home.rglob("*"):
        age(path, 3600, T)   # all of it written an hour ago
    assert backup.check(home, now=T, server=lock) == {"backup": True, "why": "first backup", "last_ok": T,
                                                      "snapshot": "s0"}
    # Nothing changed: the next looks do nothing (no restic, no podman), and say so to the seat agent.
    for k in range(1, 6):
        assert backup.check(home, now=T + k * 5 * MIN, server=lock) == {
            "backup": False, "why": "nothing changed since the last backup"}
    assert runs == [T]
    looked = json.loads((home / backup.CHECK).read_text())
    assert looked["current_at"] == T + 25 * MIN and looked["changed"] == []
    # The person talks with an agent, a change before each look: the first look backs up (the last backup is more
    # than 15 minutes old), then every third one (15 minutes), the others wait.
    base = T + 30 * MIN
    whys = []
    for k in range(7):
        now = base + k * 5 * MIN
        memory.write_text(f"Alex aime le café. ({k})\n")
        age(memory, 30, now)
        whys.append(backup.check(home, now=now, server=lock)["why"])
    assert runs == [T, base, base + 15 * MIN, base + 30 * MIN]
    assert whys == ["still changing, the last backup is 15 minutes old", "still changing", "still changing"] * 2 + \
        ["still changing, the last backup is 15 minutes old"]
    looked = json.loads((home / backup.CHECK).read_text())
    assert looked["current_at"] == T + 25 * MIN   # not while it changed
    # One more change, then the person stops: the look after it waits, the next one backs up (5 quiet minutes).
    memory.write_text("Alex aime le café. (fin)\n")
    age(memory, 30, base + 35 * MIN)
    assert backup.check(home, now=base + 35 * MIN, server=lock)["why"] == "still changing"
    assert json.loads((home / backup.CHECK).read_text())["changed"] == ["hermes/memories"]
    assert backup.check(home, now=base + 40 * MIN, server=lock)["why"] == "nothing more changed for 5 minutes"
    assert runs[-1] == base + 40 * MIN
    for k in (45, 50, 55):
        assert backup.check(home, now=base + k * MIN, server=lock)["backup"] is False
    assert len(runs) == 5 and json.loads((home / backup.CHECK).read_text())["current_at"] == base + 55 * MIN
    # A day without a change: the safety backup, once.
    day = runs[-1] + 24 * 3600
    assert backup.check(home, now=day - 10 * MIN, server=lock)["backup"] is False
    assert backup.check(home, now=day, server=lock)["why"] == "the day's backup (nothing changed)"
    assert backup.check(home, now=day + 5 * MIN, server=lock)["backup"] is False
    assert runs[-1] == day and len(runs) == 6


def test_a_change_just_after_a_backup_waits_for_5_quiet_minutes(looks):
    home, db, runs, lock = looks
    T = time.time()
    backup.check(home, now=T, server=lock)
    assert runs == [T]
    # One message 2 minutes after that backup, then nothing: the look at 5 minutes waits, the one at 10 backs up.
    db.execute("INSERT INTO messages VALUES ('une seule')")
    db.commit()
    age(home / ".hermes" / "state.db-wal", -2 * MIN, T)   # WAL mode: the database file itself untouched
    assert backup.check(home, now=T + 5 * MIN, server=lock) == {"backup": False, "why": "still changing"}
    assert json.loads((home / backup.CHECK).read_text())["changed"] == ["hermes/state.db-wal"]
    assert backup.check(home, now=T + 10 * MIN, server=lock)["why"] == "nothing more changed for 5 minutes"
    assert runs == [T, T + 10 * MIN]


def test_a_look_does_nothing_unpaired_unset_restoring_or_while_another_backup_runs(looks, tmp_path):
    home, db, runs, lock = looks
    (home / ".config" / "bipagents" / "setup.json").write_text("{}")
    assert backup.check(home, server=lock) == {"backup": False, "why": "not paired yet"}
    (home / ".config" / "bipagents" / "setup.json").unlink()
    (home / backup.MARKER).write_text("{}")
    assert backup.check(home, server=lock) == {"backup": False, "why": "restoring"}
    (home / backup.MARKER).unlink()
    # Another place of the server is backing up: wait for it (here: not at all), then leave it to the next look.
    lock.write_text("")
    held = os.open(lock, os.O_RDONLY)
    import fcntl
    fcntl.flock(held, fcntl.LOCK_EX)
    T = time.time()
    state = {"last_ok": T - 20 * MIN}
    (home / backup.STATE).parent.mkdir(parents=True, exist_ok=True)
    (home / backup.STATE).write_text(json.dumps(state))
    assert backup.check(home, now=T, server=lock, wait=0) == {
        "backup": False, "why": "another place of this server is backing up"}
    assert runs == []
    # ...unless this place is two hours behind: then without the server's lock (a stuck lock never stops it).
    (home / backup.STATE).write_text(json.dumps({"last_ok": T - 3 * 3600}))
    assert backup.check(home, now=T, server=lock, wait=0)["backup"] is True
    os.close(held)
    # This place's own other backup (the button's) running: nothing to add.
    own = os.open(home / backup.OWN_LOCK, os.O_RDWR)
    fcntl.flock(own, fcntl.LOCK_EX)
    (home / ".hermes" / "memories" / "MEMORY.md").write_text("autre\n")
    age(home / ".hermes" / "memories" / "MEMORY.md", 10 * MIN, T + 20 * MIN)
    assert backup.check(home, now=T + 20 * MIN, server=lock, wait=0)["why"] == "this place's other backup is running"
    os.close(own)
    assert backup.check(home, now=T + 25 * MIN, server=lock, wait=0)["backup"] is True
    # No backups set up: silence.
    (home / backup.SETTINGS).write_text("{}")
    assert backup.check(home, server=lock) == {"backup": False, "why": "no backups set up here"}


def test_one_place_of_a_server_at_a_time(tmp_path):
    import fcntl
    lock = tmp_path / "backup.lock"
    a, b = tmp_path / "a", tmp_path / "b"
    # No server lock (not a server of the offer): only the place's own.
    with backup.one_at_a_time(a, 0, server=lock) as busy:
        assert busy is None
    lock.write_text("")
    lock.chmod(0o444)   # root's, readable by the places: flock needs no more
    with backup.one_at_a_time(a, 0, server=lock) as busy:
        assert busy is None
        with backup.one_at_a_time(b, 0.2, server=lock) as other:
            assert other == "server"
        with backup.one_at_a_time(a, 0.2, server=lock) as same:
            assert same == "this place"
    with backup.one_at_a_time(b, 0, server=lock) as busy:
        assert busy is None
    # Another process holding it: waited for, then taken.
    held = subprocess.Popen(["python3", "-c", "import fcntl,os,sys,time; fd=os.open(sys.argv[1], os.O_RDONLY); "
                             "fcntl.flock(fd, fcntl.LOCK_EX); print('held', flush=True); time.sleep(2)", str(lock)],
                            stdout=subprocess.PIPE, text=True)
    assert held.stdout.readline().strip() == "held"
    started = time.monotonic()
    with backup.one_at_a_time(a, 30, server=lock) as busy:
        assert busy is None and time.monotonic() - started >= 0.5
    held.wait()
    fd = os.open(lock, os.O_RDONLY)
    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)   # let go after the block
    os.close(fd)


# -- with a real restic ------------------------------------------------------------------------------------------------

@pytest.mark.skipif(shutil.which("restic") is None, reason="no restic here")
def test_with_restic_backups_follow_the_changes_and_keep_the_last_two_hours(tmp_path):
    home, db = a_place(tmp_path)
    lock = tmp_path / "server.lock"
    lock.write_text("")
    T = time.time()
    for path in home.rglob("*"):
        age(path, 3600, T)
    first = backup.check(home, now=T, server=lock)
    assert first["backup"] is True and first["pruned"] == T and first["snapshot"]
    env = backup.environment(backup.settings(home), home)

    def snapshots():
        out = subprocess.run(["restic", "snapshots", "--json"], env=env, capture_output=True, text=True, check=True)
        return json.loads(out.stdout)

    assert len(snapshots()) == 1
    assert backup.check(home, now=T + 5 * MIN, server=lock)["backup"] is False
    db.execute("INSERT INTO messages VALUES ('au revoir')")
    db.commit()
    for name in ("state.db", "state.db-wal"):
        age(home / ".hermes" / name, 6 * MIN, T + 10 * MIN)
    second = backup.check(home, now=T + 10 * MIN, server=lock)
    assert second["backup"] is True and second["why"] == "nothing more changed for 5 minutes"
    assert len(snapshots()) == 2 and second["pruned"] == T   # thinned once a day only
    # What restic got is the new message.
    out = tmp_path / "restored"
    subprocess.run(["restic", "restore", "latest", "--target", str(out)], env=env, check=True, capture_output=True)
    copy = out / str(home / backup.STAGE / "sqlite" / str(home / ".hermes" / "state.db").lstrip("/")).lstrip("/")
    assert [r[0] for r in sqlite3.connect(copy).execute("SELECT text FROM messages")] == ["bonjour", "au revoir"]
    # Nothing more: no third backup; a day later the safety one, and the history thinned (the last two hours kept,
    # then one an hour: the two of the same hour a day ago become one).
    assert backup.check(home, now=T + 15 * MIN, server=lock)["backup"] is False
    third = backup.check(home, now=T + 10 * MIN + 24 * 3600, server=lock)
    assert third["why"] == "the day's backup (nothing changed)" and third["pruned"] == T + 10 * MIN + 24 * 3600
    # The day's thinning ran: restic dates the snapshots by its own clock (these three within seconds), all kept by
    # --keep-within 2h where --keep-hourly alone would keep one.
    assert len(snapshots()) == 3
    assert json.loads((home / backup.CHECK).read_text())["backed"]["at"] == T + 10 * MIN + 24 * 3600


@pytest.mark.skipif(shutil.which("restic") is None, reason="no restic here")
def test_the_thinning_keeps_everything_of_the_last_two_hours(tmp_path):
    """restic's forget with backup.KEEP over snapshots 10 minutes apart: those of the last two hours all kept, before
    them one an hour."""
    repo = tmp_path / "repo"
    env = {**os.environ, "RESTIC_REPOSITORY": str(repo), "RESTIC_PASSWORD": "k", "RESTIC_CACHE_DIR": str(tmp_path / "c")}
    subprocess.run(["restic", "init"], env=env, check=True, capture_output=True)
    (tmp_path / "f").write_text("x")
    now = time.time()
    for k in range(18):   # three hours, every 10 minutes
        at = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(now - (17 - k) * 10 * MIN))
        (tmp_path / "f").write_text(str(k))
        subprocess.run(["restic", "backup", str(tmp_path / "f"), "--host", backup.HOST, "--time", at], env=env,
                       check=True, capture_output=True)
    subprocess.run(["restic", "forget", "--host", backup.HOST, *backup.KEEP], env=env, check=True,
                   capture_output=True)
    left = json.loads(subprocess.run(["restic", "snapshots", "--json"], env=env, capture_output=True, text=True,
                                     check=True).stdout)
    assert 13 <= len(left) <= 15   # the last 2 h (12 or 13 of them), then the last one of each hour before


# -- units, migration, the seat agent ----------------------------------------------------------------------------------

def test_the_units_look_every_5_minutes_at_their_own_minute_and_the_button_still_backs_up_now(tmp_path):
    made = dict(backup.units(tmp_path / "repo", tmp_path / "python"))
    assert set(made) == {"bipagents-backup.service", "bipagents-backup-check.service", "bipagents-backup.timer"}
    now, look, timer = (made["bipagents-backup.service"], made["bipagents-backup-check.service"],
                        made["bipagents-backup.timer"])
    for unit in (now, look):
        assert "Nice=19" in unit and "IOSchedulingClass=idle" in unit and "Type=oneshot" in unit
    assert "-m bipinstall backup --quiet\n" in now   # the bridge's POST /v1/backup/now starts this one
    assert "-m bipinstall.backup --quiet --if-needed\n" in look
    assert "OnCalendar=*:0/5\n" in timer and "RandomizedDelaySec=4min\n" in timer
    assert "FixedRandomDelay=true\n" in timer and "Unit=bipagents-backup-check.service\n" in timer
    assert "hourly" not in timer and "Persistent" not in timer
    assert "--keep-within" in backup.KEEP and backup.KEEP[backup.KEEP.index("--keep-within") + 1] == "2h"
    assert backup.KEEP[2:] == ["--keep-hourly", "48", "--keep-daily", "30", "--keep-weekly", "12"]


def _seat_agent():
    spec = importlib.util.spec_from_file_location("seat_agent", ROOT / "installer" / "cloud" / "seat-agent.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_the_seat_agent_tells_until_when_the_place_is_safe_and_makes_the_server_s_lock(tmp_path, monkeypatch):
    import pwd
    agent = _seat_agent()
    home = tmp_path / "bip"
    state = home / ".local" / "state" / "bipagents"
    state.mkdir(parents=True)
    monkeypatch.setattr(agent.pwd, "getpwnam", lambda name: pwd.struct_passwd(
        (name, "x", 1000, 1000, "", str(home), "/bin/bash")))
    assert agent.last_backup("bip") is None
    (state / "backup-check.json").write_text('{"current_at": 1790009000.0}')
    assert agent.last_backup("bip") is None            # a look without any backup that worked: nothing
    (state / "backup.json").write_text('{"last_ok": 1790000000.5}')
    assert agent.last_backup("bip") == 1790009000.0    # a quiet place: its last look found it all backed up
    (state / "backup-check.json").write_text('{"current_at": 1}')
    assert agent.last_backup("bip") == 1790000000.5
    (state / "backup-check.json").write_text("not json")
    assert agent.last_backup("bip") == 1790000000.5
    lock = tmp_path / "lib" / "backup.lock"
    agent.backup_lock(str(lock))
    assert lock.stat().st_mode & 0o777 == 0o644 and lock.parent.stat().st_mode & 0o777 == 0o711
    agent.backup_lock(str(lock))   # once
