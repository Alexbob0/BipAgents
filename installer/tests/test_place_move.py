"""A planned move of a place to another server (installer/cloud/place-move.py as root, bipinstall/placemove.py as
the place): the one-time key both servers derive the same SSH keys from, what the target's forced command accepts,
the owners moved to the target's ids, the rounds' numbers, the quiet moment, and the place's own steps (its files
over a fresh place, which keeps its door and ports)."""
import base64
import importlib.util
import json
import os
import shutil
import sqlite3
import subprocess
from pathlib import Path

import pytest

from bipinstall import hermes, moving, placemove

ROOT = Path(__file__).resolve().parents[2]
KEY = base64.urlsafe_b64encode(bytes(range(32))).decode().rstrip("=")


def _load(name, file):
    spec = importlib.util.spec_from_file_location(name, ROOT / "installer" / "cloud" / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


pm = _load("place_move", "place-move.py")


def test_the_root_scripts_are_ascii():
    for name in ("place-move.py", "seat-agent.py"):
        assert (ROOT / "installer" / "cloud" / name).read_text(encoding="utf-8").isascii()


@pytest.mark.skipif(shutil.which("openssl") is None or shutil.which("ssh-keygen") is None, reason="no openssl/ssh")
def test_both_servers_derive_the_same_keys_from_the_move_s_key(tmp_path):
    host_private, host_public = pm.keypair(KEY, "host")
    client_private, client_public = pm.keypair(KEY, "client")
    assert pm.keypair(KEY, "host")[1] == host_public and host_public != client_public
    other = base64.urlsafe_b64encode(bytes(32)).decode()
    assert pm.keypair(other, "client")[1] != client_public
    for private, public in ((host_private, host_public), (client_private, client_public)):
        path = tmp_path / "key"
        path.write_text(private)
        path.chmod(0o600)
        shown = subprocess.run(["ssh-keygen", "-y", "-f", str(path)], capture_output=True, text=True).stdout
        assert shown.split()[:2] == public.split()   # ssh reads the key, and it is the one derived
    with pytest.raises(pm.MoveError):
        pm.secret_of("c2hvcnQ")


def test_the_target_lets_in_the_move_s_key_from_the_source_only(tmp_path):
    line = pm.authorized_line("ssh-ed25519 AAAA", "198.51.100.4", "m-0123456", "/usr/local/lib/bipagents/place-move.py")
    assert line.startswith('restrict,from="198.51.100.4",command="/usr/bin/sudo -n /usr/bin/python3 /usr/local/lib/'
                           'bipagents/place-move.py serve m-0123456" ssh-ed25519 AAAA')
    rule = pm.sudoers("/usr/local/lib/bipagents/place-move.py")
    assert "bipmove ALL=(root) NOPASSWD: /usr/bin/python3 /usr/local/lib/bipagents/place-move.py serve *" in rule
    assert 'env_keep += "SSH_ORIGINAL_COMMAND"' in rule
    config = pm.sshd_config("/var/lib/bipagents/moves/m-0123456", 2222, "/etc/bipagents/move-keys", "m-0123456")
    for needed in ("Port 2222", "PermitRootLogin no", "AllowUsers bipmove", "PasswordAuthentication no",
                   "PermitTTY no", "AllowTcpForwarding no", "AuthorizedKeysFile /etc/bipagents/move-keys/m-0123456"):
        assert needed in config.splitlines()
    order = tmp_path / "order.json"
    order.write_text(json.dumps({"seat": 2, "action": "move_in", "move": {
        "id": "m-0123456", "key": KEY, "source": {"address": "198.51.100.4"}, "port": 443}}))
    with pytest.raises(pm.MoveError, match="port"):
        pm.listen(str(order), root=str(tmp_path / "moves"), run=lambda *a, **k: None)
    with pytest.raises(pm.MoveError):
        pm.folder("../etc", str(tmp_path))


def test_the_forced_command_allows_rsync_receiving_under_the_place_s_home_only():
    home = "/home/bip3"
    ok = ["rsync", "--server", "-logDtpHrze.iLsfxCIvu", "--delete", "--partial", "--numeric-ids", ".",
          "/home/bip3/.hermes/"]
    assert pm.allowed_rsync(ok, home)
    assert not pm.allowed_rsync(ok[:1] + ["--server", "--sender", "-logDtpr", ".", "/home/bip3/.hermes/"], home)
    assert not pm.allowed_rsync(ok[:-1] + ["/etc/"], home)
    assert not pm.allowed_rsync(ok[:-1] + ["/home/bip3/../bip/.hermes/"], home)
    assert not pm.allowed_rsync(ok[:-1] + ["/home/bip30/x/"], home)
    assert not pm.allowed_rsync(ok[:-1] + ["--rsync-path=sh", "/home/bip3/x/"], home)
    assert not pm.allowed_rsync(["sh", "-c", "id", "/home/bip3/x"], home)


def test_owners_move_to_the_target_s_ids():
    src = {"uid": 1002, "gid": 1002, "sub_uid": 400000, "sub_gid": 400000}
    dst = {"uid": 1003, "gid": 1004, "sub_uid": 500000, "sub_gid": 500000}
    assert pm.shifted(1002, src["uid"], dst["uid"], src["sub_uid"], dst["sub_uid"]) == 1003
    assert pm.shifted(400999, src["uid"], dst["uid"], src["sub_uid"], dst["sub_uid"]) == 500999
    assert pm.shifted(0, src["uid"], dst["uid"], src["sub_uid"], dst["sub_uid"]) == 0
    assert pm.shifted(465536, src["uid"], dst["uid"], src["sub_uid"], dst["sub_uid"]) == 465536


def test_folders_go_under_the_target_s_home_and_nothing_outside():
    assert pm.rehome("/home/bip2/.hermes", "/home/bip2", "/home/bip3") == "/home/bip3/.hermes"
    with pytest.raises(pm.MoveError):
        pm.rehome("/srv/data", "/home/bip2", "/home/bip3")


def test_rsync_s_numbers_are_read():
    out = ("Number of files: 1,204 (reg: 1,000, dir: 204)\nNumber of regular files transferred: 12\n"
           "Total file size: 9,999,999 bytes\nTotal transferred file size: 1,234,567 bytes\n")
    assert pm.stats(out) == (1234567, 12)


def test_the_quiet_moment_waits_for_the_window_then_for_no_turn(monkeypatch):
    clock = [1000.0]
    answers = iter([{"turns_running": 1}, {"turns_running": 1}, {"turns_running": 0, "routines": []}])
    monkeypatch.setattr(pm, "bridge", lambda *a, **k: next(answers))
    rounds = []
    got = pm.quiet_moment("/home/bip", 1300.0, 5000.0, lambda: rounds.append(clock[0]), now=lambda: clock[0],
                          sleep=lambda s: clock.__setitem__(0, clock[0] + 60))
    assert got["turns_running"] == 0 and clock[0] >= 1300 and len(rounds) >= 1
    # Never past the window: a busy place switches anyway LATEST_BEFORE_END before its end.
    clock[0] = 4800.0
    monkeypatch.setattr(pm, "bridge", lambda *a, **k: {"turns_running": 3})
    assert pm.quiet_moment("/home/bip", 1300.0, 5000.0, lambda: None, now=lambda: clock[0],
                           sleep=lambda s: None) == {"turns_running": 3}


def test_the_drain_waits_for_turns_and_tasks(monkeypatch):
    clock = [0.0]
    answers = iter([{"turns_running": 1, "jobs_running": []}, {"turns_running": 0, "jobs_running": [{"id": "t"}]},
                    {"turns_running": 0, "jobs_running": []}])
    monkeypatch.setattr(pm, "bridge", lambda *a, **k: next(answers))
    assert pm.drained("/home/bip", now=lambda: clock[0], sleep=lambda s: clock.__setitem__(0, clock[0] + 2)) == \
        {"turns_running": 0, "jobs_running": []}


def test_states_say_phases_and_numbers_never_the_key(tmp_path):
    root = str(tmp_path / "moves")
    pm.save("m-0123456", root, role="out", seat=2, phase="copying", round=2, copied_bytes=5, source="198.51.100.4")
    pm.write_private(os.path.join(root, "m-0123456", "order.json"), json.dumps({"move": {"key": KEY}}))
    found = pm.states(root)
    assert found[0]["phase"] == "copying" and found[0]["round"] == 2
    assert KEY not in json.dumps(found) and "source" not in found[0]


def test_old_source_copies_go_after_seven_days_only_if_still_marked(tmp_path, monkeypatch):
    root = str(tmp_path / "moves")
    home = tmp_path / "bip2"
    (home / ".config" / "bipagents").mkdir(parents=True)
    entry = type("E", (), {"pw_dir": str(home), "pw_name": "bip2"})
    monkeypatch.setattr(pm, "place", lambda n: entry)
    pm.save("m-0123456", root, role="out", seat=2, phase="done", kept_until=2000)
    wiped = []
    assert pm.cleanup(root, now=1500, wipe=wiped.append) == []
    assert pm.cleanup(root, now=2500, wipe=wiped.append) == [] and wiped == []   # no mark: someone else's now
    pm.save("m-0123456", root, phase="done")
    (home / pm.MOVED_AWAY).write_text(json.dumps({"move": "m-0123456"}))
    assert pm.cleanup(root, now=2500, wipe=wiped.append) == ["bip2"]
    assert pm.states(root)[0]["phase"] == "removed"


# -- the seat agent's new orders ---------------------------------------------------------------------------------------

def test_the_seat_agent_checks_move_orders_and_hands_them_to_place_move(tmp_path):
    agent = _load("seat_agent_moves", "seat-agent.py")
    out = {"order": 3, "server": "vps-1", "seat": 2, "action": "move_out", "expires": 9_999_999_999,
           "move": {"id": "m-0123456", "key": KEY, "target": {"address": "203.0.113.9", "port": 2222},
                    "window": {"start": 1, "end": 2}, "back_in": 60}}
    assert agent.move_valid(out)
    assert agent.accepted({"id": 3, "body": json.dumps(out)}, "vps-1", set(), 1000) == out
    for wrong in ({"id": "../x"}, {"key": "short"}, {"target": {"address": "evil.example"}},
                  {"target": {"address": "203.0.113.9", "port": 22}}, {"window": {"start": "now"}}):
        assert not agent.move_valid({**out, "move": {**out["move"], **wrong}})
    into = {"order": 4, "server": "vps-2", "seat": 3, "action": "move_in", "expires": 9_999_999_999,
            "move": {"id": "m-0123456", "key": KEY, "source": {"address": "198.51.100.4"}, "port": 2222}}
    assert agent.move_valid(into)
    assert agent.move_valid({"action": "move_cancel", "move": {"id": "m-0123456"}})
    calls = []
    agent.move(out, moves=str(tmp_path), lib="/lib", run=lambda args, **k: calls.append(args))
    assert calls[-1][:4] == ["systemd-run", "--unit", "bipagents-move-m-0123456", "--collect"]
    assert calls[-1][-3:] == ["/lib/place-move.py", "out", str(tmp_path / "m-0123456" / "order.json")]
    assert oct((tmp_path / "m-0123456" / "order.json").stat().st_mode & 0o777) == "0o600"
    agent.move(into, moves=str(tmp_path), lib="/lib", run=lambda args, **k: calls.append(args))
    assert calls[-1] == ["/usr/bin/python3", "/lib/place-move.py", "in", str(tmp_path / "m-0123456" / "order.json")]


# -- the place's own steps ------------------------------------------------------------------------------------------------

def _toml(home, port, door, address, browser_first):
    return (f'bridge_key = "{"old" if door == 445 else "fresh"}-bridge-key"\nhost = "127.0.0.1"\nport = {port}\n'
            f'data_dir = "{home / "data"}"\ncloud = true\npublic_url = "https://{address}:{door}"\n\n'
            f'[hermes]\nbin = "{home}/.local/bin/hermes"\nhome = "{home / ".hermes"}"\n'
            f'hub_url = "http://127.0.0.1:{port - 1}"\n\n[browser]\nenabled = true\nfirst_port = {browser_first}\n'
            f'ports = 10\n\n[lan]\nenabled = true\nport = {door}\naddress = "{address}"\n\n'
            f'[agents.coach]\ndisplay_name = "Coach"\nhermes_url = "http://127.0.0.1:{port - 1}/p/coach"\n'
            f'hermes_key = "agent-key"\n')


def _home(root, name, port, door, address, browser_first):
    home = root / name
    (home / ".config" / "bipagents").mkdir(parents=True)
    (home / "data" / "lan").mkdir(parents=True)
    (home / "data" / "jobs").mkdir(parents=True)
    (home / ".hermes" / "memories").mkdir(parents=True)
    (home / ".hermes" / "hermes-agent").mkdir(parents=True)
    (home / ".hermes" / "cron").mkdir(parents=True)
    (home / ".config" / "bipagents" / "bridge.toml").write_text(_toml(home, port, door, address, browser_first))
    (home / ".hermes" / ".env").write_text(f"API_SERVER_PORT={port - 1}\n")
    return home


def _copy(plan, target_home):
    """What the root's rsync rounds do (with --delete): each root's tree over the target's."""
    for name, path in plan["roots"].items():
        dest = Path(pm.rehome(path, plan["home"], str(target_home)))
        if Path(path).is_dir():
            if dest.exists():
                shutil.rmtree(dest)
            shutil.copytree(path, dest, symlinks=True, ignore=shutil.ignore_patterns("hermes-agent", "*-shm"))
        else:
            shutil.copy2(path, dest)
    shutil.copytree(plan["stage"], target_home / placemove.MOVE_IN / "stage", dirs_exist_ok=True)


def test_the_moved_place_keeps_its_keys_and_the_target_its_door_and_ports(tmp_path, monkeypatch):
    old = _home(tmp_path, "bip2", 8653, 445, "203.0.113.5", 9610)
    (old / ".hermes" / "memories" / "MEMORY.md").write_text("Le chat s'appelle Pixel.\n")
    (old / "data" / "lan" / "cert.pem").write_text("OLD CERTIFICATE")
    (old / ".hermes" / "cron" / "jobs.json").write_text('{"jobs": [{"id": "j1"}]}')
    (old / "data" / "place-move.json").write_text('{"state": "switching", "paused_jobs": ["t0123456789ab"]}')
    (old / ".config" / "bipagents" / "backup.json").write_text(json.dumps({"url": "https://g/v1/backup/",
                                                                           "token": "old-seat", "password": "key"}))
    db = sqlite3.connect(old / ".hermes" / "state.db")
    db.execute("PRAGMA journal_mode=WAL")
    db.execute("CREATE TABLE messages (text TEXT)")
    db.execute("INSERT INTO messages VALUES ('rappelle-moi le vétérinaire')")
    db.commit()
    monkeypatch.setenv("HOME", str(old))
    plan = placemove.plan(old)
    assert set(plan["roots"]) >= {"config", "data", "hermes"}
    assert "- /hermes-agent" in plan["rules"]["hermes"] and "- *-shm" in plan["rules"]["hermes"]
    assert not any(r.endswith("-wal") for r in plan["rules"]["hermes"])   # a stopped place's last writes go
    assert json.loads((old / placemove.STAGE / "manifest.json").read_text())["ports"]["bridge"] == 8653

    new = _home(tmp_path, "bip3", 8663, 446, "198.51.100.7", 9620)
    (new / ".config" / "bipagents" / "setup.json").write_text('{"pair": "fresh"}')
    (new / ".config" / "bipagents" / "backup.json").write_text(json.dumps({"url": "https://g/v1/backup/",
                                                                           "token": "new-seat", "password": "x"}))
    monkeypatch.setenv("HOME", str(new))
    runner = hermes.Runner(dry=True)
    assert placemove.prepare(runner, new)["prepared"]
    assert placemove.prepare(runner, new)["again"]   # asked again: the first notes stand
    db.close()
    _copy(plan, new)
    monkeypatch.setattr("bipinstall.migrations.run", lambda runner: [])
    done = placemove.finish(runner, new, tmp_path / "checkout", "198.51.100.7")
    assert done["finished"]
    text = (new / ".config" / "bipagents" / "bridge.toml").read_text()
    assert 'bridge_key = "old-bridge-key"' in text                                   # the phones' key
    assert "port = 8663\n" in text and 'hub_url = "http://127.0.0.1:8662"' in text  # the target's ports
    assert 'public_url = "https://198.51.100.7:446"' in text and "port = 446" in text and "first_port = 9620" in text
    assert str(old) not in text.replace(f'"{old}/', "")
    assert (new / "data" / "lan" / "cert.pem").read_text() == "OLD CERTIFICATE"     # the app's pin holds
    assert (new / ".hermes" / "memories" / "MEMORY.md").read_text() == "Le chat s'appelle Pixel.\n"
    rows = sqlite3.connect(new / ".hermes" / "state.db").execute("SELECT text FROM messages").fetchall()
    assert rows == [("rappelle-moi le vétérinaire",)]
    assert (new / ".hermes" / ".env").read_text() == "API_SERVER_PORT=8662\n"
    assert (new / "data" / "place-move.json").exists()          # the new bridge starts « moving » until resumed
    assert not (new / ".config" / "bipagents" / "setup.json").exists()
    kept = json.loads((new / ".config" / "bipagents" / "backup.json").read_text())
    assert kept["token"] == "new-seat" and kept["password"] == "key"             # its own seat, the person's key
    assert any(word in line for line in runner.log for word in ("restart", "kickstart", "enable"))  # started again


@pytest.mark.skipif(b"version 3." not in subprocess.run(["rsync", "--version"], capture_output=True).stdout
                    if shutil.which("rsync") else True, reason="no GNU rsync 3 here")
def test_rsync_with_the_rules_copies_what_a_backup_takes_plus_the_sidecars(tmp_path):
    home = _home(tmp_path, "bip2", 8653, 445, "203.0.113.5", 9610)
    for rel in (".hermes/hermes-agent/run.py", ".hermes/cache/x", ".hermes/profiles/coach/models/m.bin",
                ".hermes/profiles/coach/memories/M.md", ".hermes/state.db-wal", ".hermes/state.db-shm",
                ".hermes/gateway.pid", "data/archive-index/i", "data/browsers/vie/profile/Default/Cache/c",
                "data/browsers/vie/profile/Default/Cookies", "data/outbox.db"):
        (home / rel).parent.mkdir(parents=True, exist_ok=True)
        (home / rel).write_text("x")
    inv = moving.read_inventory(home / ".config" / "bipagents" / "bridge.toml", home)
    copied = set()
    for name, root in inv.roots().items():
        rules = tmp_path / f"{name}.rules"
        rules.write_text("\n".join(placemove.rules(inv, root)) + "\n")
        out = subprocess.run(["rsync", "-a", "--dry-run", "--out-format=%n", f"--filter=merge {rules}", f"{root}/",
                              str(tmp_path / "dest" / name) + "/"], capture_output=True, text=True, check=True)
        copied |= {str(root / line) for line in out.stdout.splitlines() if line and not line.endswith("/")}
    assert str(home / ".hermes/profiles/coach/memories/M.md") in copied
    assert str(home / ".hermes/state.db-wal") in copied
    for left in (".hermes/hermes-agent/run.py", ".hermes/cache/x", ".hermes/profiles/coach/models/m.bin",
                 ".hermes/state.db-shm", ".hermes/gateway.pid", "data/archive-index/i",
                 "data/browsers/vie/profile/Default/Cache/c"):
        assert str(home / left) not in copied, left


def test_remove_clears_only_the_place_a_completed_move_left(tmp_path, monkeypatch):
    agent = _load("seat_agent_remove", "seat-agent.py")
    body = {"order": 5, "server": "vps-1", "seat": 2, "action": "remove", "expires": 9_999_999_999,
            "move": {"id": "m-0123456"}}
    assert agent.accepted({"id": 5, "body": json.dumps(body)}, "vps-1", set(), 1000) == body
    assert agent.remove_valid({"removed": True}) and not agent.remove_valid({})
    assert not agent.remove_valid({"move": {"id": "../x"}})
    moves = tmp_path / "moves"
    home = tmp_path / "bip2"
    (home / ".config" / "bipagents").mkdir(parents=True)
    monkeypatch.setattr(agent.pwd, "getpwnam", lambda user: type("E", (), {"pw_dir": str(home)}))
    wiped, calls = [], []

    def run(args, **kw):
        calls.append(args)
    # No completed move here: refused.
    assert not agent.remove(2, body, str(moves), wiped.append, run) and wiped == []
    pm.save("m-0123456", str(moves), role="out", seat=2, phase="copying")
    assert not agent.remove(2, body, str(moves), wiped.append, run)
    pm.save("m-0123456", str(moves), phase="done")
    # Completed, but the account is someone else's now (no mark of this move).
    assert not agent.remove(2, body, str(moves), wiped.append, run) and wiped == []
    (home / ".config" / "bipagents" / "moved-away.json").write_text(json.dumps({"move": "m-0123456"}))
    assert agent.remove(2, body, str(moves), wiped.append, run)
    assert wiped == ["bip2"] and ["ufw", "delete", "allow", "444/tcp"] in calls
    assert pm.states(str(moves))[0]["phase"] == "removed"

    def gone(user):
        raise KeyError(user)
    # Again (the account gone): done, nothing broken.
    monkeypatch.setattr(agent.pwd, "getpwnam", gone)
    assert agent.remove(2, body, str(moves), wiped.append, run)
    # Marked removed by the guichet without a move.
    assert agent.remove(3, {"removed": True}, str(moves), wiped.append, run) and wiped[-1] == "bip3"
    assert not agent.remove(3, {"removed": "yes"}, str(moves), wiped.append, run)


def test_each_server_keeps_its_own_bridge_binaries(tmp_path):
    """A place of the offer keeps its data in ~/.local/share/bipagents, beside this server's bridge binaries."""
    home = tmp_path / "bip"
    (home / ".config" / "bipagents").mkdir(parents=True)
    (home / ".local" / "share" / "bipagents" / "bridge" / "d07b754db429").mkdir(parents=True)
    (home / ".config" / "bipagents" / "bridge.toml").write_text(
        f'bridge_key = "k"\nport = 8643\ndata_dir = "{home}/.local/share/bipagents"\n'
        f'[hermes]\nhome = "{home}/.hermes"\n')
    inv = moving.read_inventory(home / ".config" / "bipagents" / "bridge.toml", home)
    assert "- /bridge" in placemove.rules(inv, inv.data_dir)
    assert "- /bridge" not in placemove.rules(inv, home / ".hermes")


def test_the_switch_never_waits_systemd_s_90_seconds_for_a_hanging_service():
    class Fake:
        dry = False

        def __init__(self, hanging):
            self.hanging, self.log = hanging, []

        def run(self, args, check=True):
            self.log.append(args)
            if args[2] == "is-active":
                return "deactivating\n" if args[3] in self.hanging else "inactive\n"
            return ""
    runner = Fake({"bipagents-bridge"})
    killed = placemove.quick_stop(runner, ["hermes-gateway", "bipagents-bridge"], grace=2, sleep=lambda s: None)
    assert killed == ["bipagents-bridge"]
    assert ["systemctl", "--user", "kill", "--signal=SIGKILL", "bipagents-bridge"] in runner.log
    assert ["systemctl", "--user", "stop", "--no-block", "hermes-gateway"] in runner.log
    assert placemove.quick_stop(Fake(set()), ["hermes-gateway"], sleep=lambda s: None) == []


# -- failures and a stopped place (2026-10-30) ----------------------------------------------------------------------------

class _Channel:
    """The source's end of the SSH channel, faked: what was asked, what was copied."""
    asked, copied = [], []

    def __init__(self, base, address, port, key):
        type(self).asked, type(self).copied = [], []

    def ask(self, *words, timeout=900):
        type(self).asked.append(words[0])
        if words[0] == "info":
            return {"home": "/home/bip3"}
        if words[0] == "finish":
            return {"ok": True, "verified": {"bridge": True}}
        return {}

    def rsync(self, source, dest, rules=None, checksum=False, timeout=0):
        type(self).copied.append((os.path.basename(source.rstrip("/")), checksum))
        return 10, 1


def _a_move_out(tmp_path, monkeypatch, drain=True):
    """A move_out order for place 2, its account and its own steps faked: (order file, moves root, home, the
    place's own steps asked)."""
    home = tmp_path / "bip2"
    (home / ".hermes").mkdir(parents=True)
    (home / ".config" / "bipagents").mkdir(parents=True)
    stage = home / "stage"
    stage.mkdir()
    entry = type("E", (), {"pw_dir": str(home), "pw_name": "bip2", "pw_uid": 4242, "pw_gid": 4242})
    monkeypatch.setattr(pm, "place", lambda n: entry)
    monkeypatch.setattr(pm, "Channel", _Channel)
    steps = []

    def as_user(e, args, timeout=0):
        steps.append(" ".join(args))
        if args == ["move-out", "plan"]:
            return {"home": str(home), "stage": str(stage), "roots": {"hermes": str(home / ".hermes")}, "rules": {}}
        return {}
    monkeypatch.setattr(pm, "as_user", as_user)
    monkeypatch.setattr(pm, "quiet_moment", lambda *a, **k: {"turns_running": 0, "routines": []})
    move = {"id": "m-0123456", "key": KEY, "target": {"address": "203.0.113.9", "port": 2222}, "back_in": 60}
    if not drain:
        move["drain"] = False
    order = tmp_path / "order.json"
    order.write_text(json.dumps({"seat": 2, "action": "move_out", "move": move}))
    return str(order), str(tmp_path / "moves"), home, steps


def test_a_bridge_down_at_the_drain_ends_the_move_in_failed_and_the_place_resumed(tmp_path, monkeypatch):
    """The drain's call raised urllib's error (not a MoveError): the move used to stay « draining » until the guichet
    gave up 20 minutes later. Now: rolled back (the target's copy stopped, the bridge resumed), `failed`, exit 1."""
    import urllib.error
    order, root, home, steps = _a_move_out(tmp_path, monkeypatch)
    calls = []

    def bridge(h, method, path, body=None, timeout=30):
        calls.append(path)
        if path == "/v1/place/move/drain":
            raise urllib.error.URLError(ConnectionRefusedError(111, "Connection refused"))
        return {}
    monkeypatch.setattr(pm, "bridge", bridge)
    assert pm.out(order, root) == 1
    state = pm.states(root)[0]
    assert state["phase"] == "failed" and state["result"] == "rolled back"
    assert state["error"].startswith("URLError") and KEY not in json.dumps(state)
    assert "abort" in _Channel.asked and "finish" not in _Channel.asked
    assert calls[-1] == "/v1/place/move/resume"            # the drain undone
    assert "move-out stop" not in steps                      # never stopped
    assert not (home / pm.MOVED_AWAY).exists()


def test_any_failure_of_the_unit_is_said_failed_never_left_in_its_phase(tmp_path):
    root = str(tmp_path / "moves")
    order = tmp_path / "order.json"
    order.write_text(json.dumps({"seat": 2, "move": {"id": "m-0123456", "key": KEY}}))

    def broken(path, moves):
        pm.save("m-0123456", moves, role="out", seat=2, phase="waiting")
        raise KeyError("routines")
    assert pm.out(str(order), root, mover=broken) == 1
    state = pm.states(root)[0]
    assert (state["phase"], state["result"], state["error"]) == ("failed", "not moved", "KeyError: 'routines'")

    def cancelled_then_broken(path, moves):   # a final phase is kept as it is
        pm.save("m-0123456", moves, phase="cancelled", result=None)
        raise RuntimeError("late")
    assert pm.out(str(order), root, mover=cancelled_then_broken) == 1
    assert pm.states(root)[0]["phase"] == "cancelled"
    order.write_text("not json")                              # nothing to say it of: still a clean exit
    assert pm.out(str(order), root, mover=broken) == 1


def test_a_stopped_place_moves_without_its_bridge(tmp_path, monkeypatch):
    """`"drain": false`: no copies while it runs, no window, no drain, nothing asked of its bridge: stopped (made
    sure), copied as it is, then the same switch, checksums and the stage included."""
    order, root, home, steps = _a_move_out(tmp_path, monkeypatch, drain=False)

    def never(*a, **k):
        raise AssertionError("the bridge is never asked")
    monkeypatch.setattr(pm, "bridge", never)
    monkeypatch.setattr(pm, "quiet_moment", never)
    checked = []
    monkeypatch.setattr(pm, "made_stopped", lambda entry, proc="/proc", run=None: checked.append(entry.pw_uid))
    assert pm.out(order, root) == 0
    state = pm.states(root)[0]
    assert state["phase"] == "done" and state["drain_seconds"] == 0 and state["round"] == 1
    assert steps == ["move-out plan", "move-out stop"] and checked == [4242]
    assert _Channel.asked == ["info", "prepare", "finish"]
    assert _Channel.copied == [(".hermes", False), (".hermes", True), ("stage", True)]
    assert json.loads((home / pm.MOVED_AWAY).read_text())["move"] == "m-0123456"


def test_a_stopped_place_that_still_runs_is_not_moved(tmp_path, monkeypatch):
    order, root, home, steps = _a_move_out(tmp_path, monkeypatch, drain=False)
    monkeypatch.setattr(pm, "bridge", lambda *a, **k: {})

    def still(entry, proc="/proc", run=None):
        raise pm.MoveError("the place still runs: bipbridge")
    monkeypatch.setattr(pm, "made_stopped", still)
    assert pm.out(order, root) == 1
    state = pm.states(root)[0]
    assert (state["phase"], state["error"]) == ("failed", "the place still runs: bipbridge")
    assert steps[-1] == "resume" and "abort" in _Channel.asked and "finish" not in _Channel.asked
    assert _Channel.copied == []


def test_what_still_runs_of_the_place_is_killed_or_the_move_refused(tmp_path):
    proc = tmp_path / "proc"
    for number, command in enumerate(("/home/bip2/.local/share/bipagents/bridge/abc/bipbridge serve",
                                      "/opt/bipagents/hermes/bin/python -m hermes gateway run", "sleep 10"), 100):
        (proc / str(number)).mkdir(parents=True)
        (proc / str(number) / "cmdline").write_bytes(b"\0".join(w.encode() for w in command.split()) + b"\0")
    uid = os.stat(proc / "100").st_uid
    assert pm.still_running(uid, str(proc)) == ["bipbridge", "hermes gateway"]
    assert pm.still_running(uid + 1, str(proc)) == []        # another account's
    entry = type("E", (), {"pw_uid": uid, "pw_name": "bip2"})
    killed = []

    def pkill(args, **kw):
        killed.append(args)
        if args[-1] == "bipbridge":
            shutil.rmtree(proc / "100")
    with pytest.raises(pm.MoveError, match="hermes gateway"):
        pm.made_stopped(entry, str(proc), pkill, sleep=lambda s: None, wait=3)
    assert ["pkill", "-KILL", "-u", "bip2", "-f", "bipbridge"] in killed
    assert ["pkill", "-KILL", "-u", "bip2", "-f", "hermes.*gateway"] in killed
    shutil.rmtree(proc / "101")
    killed.clear()
    pm.made_stopped(entry, str(proc), pkill, sleep=lambda s: None)   # nothing left: nothing killed
    assert killed == []


def test_a_cancel_starts_again_a_stopped_place_being_copied(tmp_path, monkeypatch):
    root = str(tmp_path / "moves")
    entry = type("E", (), {"pw_dir": str(tmp_path), "pw_name": "bip2"})
    monkeypatch.setattr(pm, "place", lambda n: entry)
    monkeypatch.setattr(pm.subprocess, "run", lambda *a, **k: None)
    monkeypatch.setattr(pm, "bridge", lambda *a, **k: {})
    steps = []
    monkeypatch.setattr(pm, "as_user", lambda e, args, timeout=0: steps.append(args) or {})
    pm.save("m-0123456", root, role="out", seat=2, phase="copying", drain=True)
    pm.cancel("m-0123456", root)
    assert steps == []                                        # a running place being copied: nothing to resume
    pm.save("m-0123456", root, phase="copying", drain=False)
    pm.cancel("m-0123456", root)
    assert steps == [["resume"]] and pm.states(root)[0]["phase"] == "cancelled"


def test_the_seat_agent_takes_a_stopped_place_s_move_and_both_remove_keys(tmp_path):
    agent = _load("seat_agent_drain", "seat-agent.py")
    out = {"order": 3, "server": "vps-1", "seat": 2, "action": "move_out", "expires": 9_999_999_999,
           "move": {"id": "m-0123456", "key": KEY, "target": {"address": "203.0.113.9", "port": 2222},
                    "back_in": 60, "drain": False}}
    assert agent.move_valid(out)
    assert not agent.move_valid({**out, "move": {**out["move"], "drain": "no"}})
    # what the guichet's orders::remove signed until 2026-10-30, and what it signs now
    for body in ({"remove": True}, {"removed": True}):
        order = {"order": 5, "server": "vps-1", "seat": 2, "action": "remove", "expires": 9_999_999_999, **body}
        assert agent.accepted({"id": 5, "body": json.dumps(order)}, "vps-1", set(), 1000) == order
    assert not agent.remove_valid({"remove": "yes"}) and not agent.remove_valid({"removed": 1})
    wiped = []
    assert agent.remove(3, {"remove": True}, str(tmp_path), wiped.append, lambda *a, **k: None) and wiped == ["bip3"]


def test_an_update_waits_while_the_place_moves_and_never_starts_a_moved_copy(tmp_path):
    import time
    agent = _load("seat_agent_update", "seat-agent.py")
    moves = str(tmp_path / "moves")
    home = tmp_path / "bip2"
    (home / ".config" / "bipagents").mkdir(parents=True)
    done = []

    def updater(number, commit):
        done.append(number)
    commit = "a" * 40
    assert agent.update_order(2, commit, moves, str(home), updater) and done == [2]   # no move: updated
    for role, phase in (("out", "copying"), ("out", "draining"), ("out", "switching"), ("in", "receiving")):
        pm.save("m-0123456", moves, role=role, seat=2, phase=phase)
        assert agent.moving(2, moves) == "m-0123456" and agent.moving(3, moves) is None
        assert not agent.update_order(2, commit, moves, str(home), updater)   # held: served again next beat
    assert done == [2]
    pm.save("m-0123456", moves, role="out", phase="failed")
    assert agent.update_order(2, commit, moves, str(home), updater) and done == [2, 2]   # it stayed: updated
    pm.save("m-0123456", moves, phase="copying")
    assert agent.moving(2, moves, now=time.time() + agent.MOVE_STALE + 1) is None   # a move long silent is over
    pm.save("m-0123456", moves, phase="done")
    (home / agent.MOVED_AWAY).write_text(json.dumps({"move": "m-0123456"}))
    assert agent.update_order(2, commit, moves, str(home), updater) and done == [2, 2]   # dropped, never started
