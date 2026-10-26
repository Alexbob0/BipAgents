import base64
import json
import secrets
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "installer"))

TEMPLATE = ROOT / "installer" / "cloud" / "user-data.yaml"


def render(pair: str, language: str = "fr", profile: str = "solo") -> str:
    """What the apps do: three placeholders, nothing else."""
    return (TEMPLATE.read_text(encoding="utf-8").replace("{{pair}}", pair).replace("{{language}}", language)
            .replace("{{profile}}", profile))


def test_the_user_data_the_apps_send_sets_up_the_server_and_runs_the_cloud_install(tmp_path):
    from bipinstall import cloud

    pair = base64.urlsafe_b64encode(secrets.token_bytes(32)).decode().rstrip("=")
    text = render(pair)
    assert text.startswith("#cloud-config\n") and "{{" not in text
    data = yaml.safe_load(text)
    assert data["users"][1] == {"name": "bip", "shell": "/bin/bash", "lock_passwd": True}   # no sudo
    assert {"podman", "uidmap", "ufw", "unattended-upgrades"} <= set(data["packages"])
    files = {f["path"]: f for f in data["write_files"]}
    setup = files["/etc/bipagents/cloud.json"]
    assert setup["permissions"] == "0600"
    (tmp_path / "cloud.json").write_text(setup["content"])
    assert cloud.read(tmp_path / "cloud.json") == cloud.CloudSetup(pair=pair, language="fr", profile="solo")
    assert "ip_unprivileged_port_start = 443" in files["/etc/sysctl.d/60-bipagents.conf"]["content"]
    assert "PasswordAuthentication no" in files["/etc/ssh/sshd_config.d/10-bipagents.conf"]["content"]
    commands = data["runcmd"]
    # Nothing in but SSH (by key) and the bridge's door; the install as `bip`, then the secret's copy removed.
    assert commands.index("ufw default deny incoming") < commands.index("ufw --force enable")
    assert "ufw allow 443/tcp" in commands and not any("ufw allow 80" in c for c in commands)
    install = next(c for c in commands if "install.sh" in c)
    assert install.startswith("sudo -iu bip ") and "--cloud /etc/bipagents/cloud.json" in install
    assert commands.index(install) < next(i for i, c in enumerate(commands) if c.startswith("shred"))
    assert commands.index("loginctl enable-linger bip") < commands.index(install)
    # Compressed memory ahead of the swap file, its module installed when the kernel keeps it apart (never failing
    # the boot); logs kept small.
    zram = next(c for c in commands if "zram" in c)
    assert zram.startswith('modprobe zram || apt-get install -y "linux-modules-extra-$(uname -r)"; modprobe zram && ')
    assert zram.endswith("|| true")
    assert commands.index(zram) < next(i for i, c in enumerate(commands) if "/swapfile" in c) < commands.index(install)
    assert "SystemMaxUse=200M" in files["/etc/systemd/journald.conf.d/60-bipagents.conf"]["content"]


def test_no_model_key_or_other_secret_has_a_place_in_it():
    text = TEMPLATE.read_text(encoding="utf-8")
    import re
    assert text.isascii(), "DigitalOcean ignores a user-data holding non-ASCII characters (accents, guillemets)"
    assert sorted(set(re.findall(r"\{\{(\w+)\}\}", text))) == ["language", "pair", "profile"]
    assert "KEY" not in text.replace("SSH", "")
    assert len(render("x" * 43).encode()) < 16_000   # far under the providers' user-data limits (64 KB at DigitalOcean)


def test_the_server_reboots_at_night_when_an_update_asks_at_a_minute_of_its_own(tmp_path):
    """unattended-upgrades reboots by itself when an update needs it (a kernel's fixes only count after one), whoever
    is connected, between 00:00 and 00:44 server time: the minute comes from the machine id, so it is the same each
    time and the servers of a region don't go down together. Updates run at 23:00-23:30, just before."""
    import subprocess
    data = yaml.safe_load(render("x" * 43))
    files = {f["path"]: f["content"] for f in data["write_files"]}
    reboot = files["/etc/apt/apt.conf.d/52bipagents-reboot"]
    assert 'Automatic-Reboot "true";' in reboot and 'Automatic-Reboot-WithUsers "true";' in reboot
    assert 'Automatic-Reboot-Time "00:20";' in reboot   # until the minute of its own is written
    timer = files["/etc/systemd/system/apt-daily-upgrade.timer.d/60-bipagents.conf"]
    assert "OnCalendar=\nOnCalendar=*-*-* 23:00\n" in timer and "RandomizedDelaySec=30min" in timer
    assert "$nrconf{restart} = 'a';" in files["/etc/needrestart/conf.d/60-bipagents.conf"]
    commands = data["runcmd"]
    minute = next(c for c in commands if "53bipagents-reboot-time" in c)
    assert commands.index(minute) < commands.index("systemctl daemon-reload") < commands.index(
        "loginctl enable-linger bip")
    found = set()
    for machine_id in ("0000aaaa", "ffffffff", "4f3a1b2c", "a1b2c3d4"):
        (tmp_path / "machine-id").write_text(machine_id + "0" * 24 + "\n")
        out = tmp_path / "time"
        command = minute.replace("/etc/machine-id", str(tmp_path / "machine-id")).replace(
            "/etc/apt/apt.conf.d/53bipagents-reboot-time", str(out))
        assert subprocess.run(["sh", "-c", command]).returncode == 0
        text = out.read_text()
        assert text == 'Unattended-Upgrade::Automatic-Reboot-Time "00:%02d";\n' % (int(machine_id[:4], 16) % 45)
        found.add(text)
    assert len(found) > 1


def test_servers_installed_before_get_the_same_night_reboot_with_one_command():
    """nightly-reboot.sh (root, once, on a server installed before 2026-10-22): user-data's settings word for word,
    the same minute, and on a guichet's server the seat agent taken again with its reboot notice."""
    import subprocess
    path = ROOT / "installer" / "cloud" / "nightly-reboot.sh"
    text = path.read_text(encoding="utf-8")
    assert text.isascii() and subprocess.run(["bash", "-n", str(path)]).returncode == 0
    data = yaml.safe_load(render("x" * 43))
    files = {f["path"]: f["content"] for f in data["write_files"]}
    for name in ("/etc/apt/apt.conf.d/52bipagents-reboot",
                 "/etc/systemd/system/apt-daily-upgrade.timer.d/60-bipagents.conf",
                 "/etc/needrestart/conf.d/60-bipagents.conf"):
        assert f"cat > {name} <<'EOF'\n{files[name]}EOF\n" in text
    minute = next(c for c in data["runcmd"] if "53bipagents-reboot-time" in c)
    assert "\n" + minute + "\n" in text
    assert "ExecStop=/usr/bin/python3 $lib/seat-agent.py --rebooting" in text
    assert "systemctl enable --now bipagents-reboot-notice.service" in text


def test_the_seat_agent_tells_the_guichet_of_a_reboot_only(monkeypatch):
    """At shutdown (bipagents-reboot-notice.service): a reboot under way is told to the guichet, a stop is not, and
    a guichet that doesn't answer never holds the shutdown up beyond the call's timeout."""
    agent = _seat_agent()
    calls = []
    monkeypatch.setattr(agent, "call", lambda conf, method, path, body=None: calls.append((method, path)) or {})
    jobs = {"text": "1215 reboot.target start waiting\n1216 systemd-reboot.service start waiting\n"}

    class Done:
        def __init__(self, stdout):
            self.stdout = stdout

    monkeypatch.setattr(agent.subprocess, "run", lambda *a, **k: Done(jobs["text"]))
    assert agent.rebooting({"url": "https://g", "token": "t"}) == 0 and calls == [("POST", "/v1/server/rebooting")]
    jobs["text"] = "1215 poweroff.target start waiting\n"
    assert agent.rebooting({}) == 0 and len(calls) == 1

    def unreachable(*a, **k):
        raise OSError("no network")

    jobs["text"] = "1 reboot.target start waiting\n"
    monkeypatch.setattr(agent, "call", unreachable)
    assert agent.rebooting({}) == 0


def test_the_seat_script_is_ascii_and_refuses_a_wrong_call(tmp_path):
    """add-seat.sh runs as root at first boot (the guichet's script) and for the seat agent: ASCII like user-data,
    and nothing done when called without a place number of 1 or more and an existing setup file."""
    import subprocess
    path = ROOT / "installer" / "cloud" / "add-seat.sh"
    text = path.read_text(encoding="utf-8")
    assert text.isascii() and "--cloud $setup" in text and 'ufw allow "$port/tcp"' in text
    assert '[ "$n" -eq 1 ] && user="bip"' in text
    for args in ([], ["0", str(path)], ["x", str(path)], ["2", str(tmp_path / "missing.json")]):
        assert subprocess.run(["bash", str(path), *args], capture_output=True).returncode == 2
    # A version the guichet pins is a commit's 40 characters, nothing else (checked before anything is done).
    assert 'BIPAGENTS_TO=$version' in text and "${pin}bash -s -- --cloud $setup" in text
    for version in ("main", "x" * 40, "0123abc", "a" * 40 + "; rm -rf /"):
        setup = tmp_path / "setup.json"
        setup.write_text(json.dumps({"v": 1, "port": 444, "version": version}))
        assert subprocess.run(["bash", str(path), "2", str(setup)], capture_output=True).returncode == 2


def _seat_agent():
    import importlib.util
    spec = importlib.util.spec_from_file_location("seat_agent", ROOT / "installer" / "cloud" / "seat-agent.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _signed(key, body):
    return {"id": body["order"], "body": json.dumps(body, sort_keys=True, separators=(",", ":")),
            "signature": base64.b64encode(key.sign(json.dumps(body, sort_keys=True, separators=(",", ":")).encode()))
            .decode()}


def test_the_seat_agent_does_only_signed_orders_for_its_server(tmp_path):
    """installer/cloud/seat-agent.py (root, every two minutes): an order is done only when the guichet's key signed
    it, for this server, not expired, for a place whose door matches, and not done before."""
    import shutil
    import subprocess
    import time
    import pytest
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

    agent = _seat_agent()
    assert (ROOT / "installer" / "cloud" / "seat-agent.py").read_text(encoding="utf-8").isascii()
    key, other = Ed25519PrivateKey.generate(), Ed25519PrivateKey.generate()
    pub = tmp_path / "guichet.pub"
    pub.write_bytes(key.public_key().public_bytes(serialization.Encoding.PEM,
                                                  serialization.PublicFormat.SubjectPublicKeyInfo))
    now = time.time()
    body = {"order": 7, "server": "vps-1", "seat": 2, "action": "reset", "expires": int(now + 3600),
            "setup": {"v": 1, "pair": "p" * 43, "language": "fr", "profile": "solo", "port": 444}}
    order = _signed(key, body)
    assert agent.accepted(order, "vps-1", set(), now) == body
    assert agent.accepted(order, "vps-1", {7}, now) is None                 # done before
    assert agent.accepted(order, "vps-2", set(), now) is None               # another server's
    assert agent.accepted(order, "vps-1", set(), now + 7200) is None        # expired
    for change in ({"action": "run"}, {"seat": 0}, {"seat": 3}):
        assert agent.accepted(_signed(key, {**body, **change}), "vps-1", set(), now) is None
    assert agent.accepted({**order, "id": 8}, "vps-1", set(), now) is None   # signed for another order
    # A version to bring the place to: a full commit, nothing else.
    update = {"order": 9, "server": "vps-1", "seat": 3, "action": "update", "expires": int(now + 3600),
              "commit": "0123456789abcdef0123456789abcdef01234567"}
    assert agent.accepted(_signed(key, update), "vps-1", set(), now) == update
    for change in ({"commit": "0123456"}, {"commit": "main"}, {"commit": "0123456789abcdef0123456789abcdef0123456; rm"},
                   {"commit": None}):
        assert agent.accepted(_signed(key, {**update, **change}), "vps-1", set(), now) is None
    # A place pairing again: a pairing secret, nothing else.
    again = {"order": 10, "server": "vps-1", "seat": 1, "action": "reopen", "expires": int(now + 3600), "pair": "q" * 43}
    assert agent.accepted(_signed(key, again), "vps-1", set(), now) == again
    for change in ({"pair": "short"}, {"pair": "q" * 42 + "/"}, {"pair": None}, {"pair": "a\nb" * 20}):
        assert agent.accepted(_signed(key, {**again, **change}), "vps-1", set(), now) is None
    assert agent.account(1) == "bip" and agent.account(3) == "bip3"
    probe = subprocess.run(["openssl", "pkeyutl", "-help"], capture_output=True, text=True)
    if shutil.which("openssl") is None or "-rawin" not in probe.stdout + probe.stderr:
        pytest.skip("no OpenSSL 3 here")
    assert agent.verified(order["body"], order["signature"], str(pub))
    assert not agent.verified(order["body"].replace("vps-1", "vps-2"), order["signature"], str(pub))
    assert not agent.verified(order["body"], _signed(other, body)["signature"], str(pub))
    assert not agent.verified(order["body"], "not base64!", str(pub))


def test_the_seat_agent_s_heartbeat_says_only_states_a_version_a_time_and_numbers(tmp_path, monkeypatch):
    """Each place: its door answers on the server, its Hermes runs, the BipAgents commit it runs, the time of its last
    backup that worked; the server: memory, swap, disk, load. Nothing else."""
    import os
    import pwd
    import socket
    agent = _seat_agent()
    home = tmp_path / "bip"
    (home / ".local" / "state" / "bipagents").mkdir(parents=True)
    (home / ".local" / "state" / "bipagents" / "backup.json").write_text('{"last_ok": 1790000000.5, "note": "x"}')
    (home / "BipAgents" / ".git").mkdir(parents=True)
    (home / "BipAgents" / ".git" / "HEAD").write_text("ref: refs/heads/main\n")
    (home / "BipAgents" / ".git" / "packed-refs").write_text("# pack-refs\n0123456789abcdef0123 refs/heads/main\n")
    users = {"bip": (home, 1000), "bip2": (tmp_path / "bip2", 1001)}

    def getpwnam(name):
        if name not in users:
            raise KeyError(name)
        return pwd.struct_passwd((name, "x", users[name][1], users[name][1], "", str(users[name][0]), "/bin/bash"))

    monkeypatch.setattr(agent.pwd, "getpwnam", getpwnam)
    listening = socket.socket()
    listening.bind(("127.0.0.1", 0))
    listening.listen()
    port = listening.getsockname()[1]
    monkeypatch.setattr(agent, "door_open", lambda p: p == 443)
    monkeypatch.setattr(agent, "running", lambda uid, words: uid == 1000 and words == ("hermes", "gateway"))
    (tmp_path / "bip2" / ".local" / "state" / "bipagents").mkdir(parents=True)
    (tmp_path / "bip2" / ".local" / "state" / "bipagents" / "asleep").write_text("1790000000")
    # place 2's encrypted space waits for its key (bridge-rs/src/space.rs): locked, not down
    (tmp_path / "bip2" / ".local" / "share" / "bipagents" / "space").mkdir(parents=True)
    (tmp_path / "bip2" / ".local" / "share" / "bipagents" / "space" / "locked.json").write_text(
        '{"since": 1790000000, "reason": "key service unreachable", "tries": 3}')
    assert agent.health() == {"1": {"up": True, "hermes": True, "asleep": False, "version": "0123456789ab",
                                    "backup": 1790000000.5, "locked": None},
                              "2": {"up": False, "hermes": False, "asleep": True, "version": None, "backup": None,
                                    "locked": "key service unreachable"}}
    monkeypatch.undo()
    assert agent.door_open(port) is True
    listening.close()
    assert agent.door_open(port) is False
    # What runs is found by its command line, for the account only.
    proc = tmp_path / "proc"
    (proc / "123").mkdir(parents=True)
    (proc / "123" / "cmdline").write_bytes(b"/home/bip/.hermes/venv/bin/python\0-m\0hermes\0gateway\0run\0")
    (proc / "self").mkdir()
    assert agent.running(os.getuid(), ("hermes", "gateway"), proc=str(proc))
    assert not agent.running(os.getuid(), ("bipbridge",), proc=str(proc))
    assert not agent.running(os.getuid() + 1, ("hermes",), proc=str(proc))
    meminfo = tmp_path / "meminfo"
    meminfo.write_text("MemTotal:        8000000 kB\nMemAvailable:    3000000 kB\nSwapTotal:       2097148 kB\n"
                       "SwapFree:        1048576 kB\nHugePages_Total:       0\n")
    (tmp_path / "uptime").write_text("754.31 2900.02\n")
    numbers = agent.vitals(str(meminfo), str(tmp_path), str(tmp_path / "uptime"))
    assert set(numbers) == {"memory_mb", "available_mb", "swap_mb", "swap_used_mb", "disk_gb", "disk_free_gb", "load",
                            "uptime"}
    assert numbers["uptime"] == 754 and agent.uptime(str(tmp_path / "missing")) is None
    assert numbers["memory_mb"] == 7812 and numbers["available_mb"] == 2929 and numbers["swap_used_mb"] == 1023


def _places(agent, monkeypatch, uids):
    import pwd

    def getpwnam(name):
        if name not in uids:
            raise KeyError(name)
        return pwd.struct_passwd((name, "x", uids[name], uids[name], "", "/home/" + name, "/bin/bash"))

    monkeypatch.setattr(agent.pwd, "getpwnam", getpwnam)


def test_each_place_gets_a_soft_memory_ceiling_from_the_server_s_memory(tmp_path, monkeypatch):
    """MemoryHigh (never MemoryMax) on each running place's user slice: 35 % of the server's memory, or the share the
    guichet sent; set again only when it differs."""
    agent = _seat_agent()
    _places(agent, monkeypatch, {"bip": 1000, "bip2": 1001, "bip3": 1002})
    root = tmp_path / "user.slice"
    (root / "user-1000.slice").mkdir(parents=True)
    (root / "user-1000.slice" / "memory.high").write_text("max\n")
    (root / "user-1001.slice").mkdir()
    (root / "user-1001.slice" / "memory.high").write_text("%d\n" % (2800 * 1024 * 1024))
    # bip3: no slice (its services not started): nothing set for it
    calls = []
    agent.ceiling(8000, {}, str(root), lambda *args: calls.append(args))
    assert calls == [("systemctl", "set-property", "user-1000.slice", "MemoryHigh=2800M")]
    calls.clear()
    agent.ceiling(8000, {"memory_high_share": 0.5}, str(root), lambda *args: calls.append(args))
    assert calls == [("systemctl", "set-property", "user-1000.slice", "MemoryHigh=4000M"),
                     ("systemctl", "set-property", "user-1001.slice", "MemoryHigh=4000M")]
    assert agent.ceiling_mb(8000, {"memory_high_share": 5}) == agent.ceiling_mb(8000, {"memory_high_share": True}) \
        == 2800
    text = (ROOT / "installer" / "cloud" / "seat-agent.py").read_text(encoding="utf-8")
    assert "MemoryMax" not in text.replace("never MemoryMax", "")
    # The guichet's answer: a known setting within reason is kept, anything else is not.
    settings = tmp_path / "settings.json"
    agent.keep_settings({"orders": []}, str(settings))
    agent.keep_settings({"settings": {"memory_high_share": 0.02}}, str(settings))
    agent.keep_settings({"settings": {"memory_max": 1}}, str(settings))
    assert not settings.exists()
    agent.keep_settings({"settings": {"memory_high_share": 0.4}}, str(settings))
    assert json.loads(settings.read_text()) == {"memory_high_share": 0.4}


def test_the_beat_says_each_place_s_memory_now_and_its_peak_since_the_last_beat(tmp_path, monkeypatch):
    """The contract with the guichet: {"places": {"<seat>": {"mem_mb": n, "mem_peak_mb": n}}}, in MB."""
    import shutil
    agent = _seat_agent()
    _places(agent, monkeypatch, {"bip": 1000, "bip2": 1001})
    root = tmp_path / "user.slice"
    samples = str(tmp_path / "run" / "memory.json")
    mb = 1024 * 1024
    for uid in (1000, 1001):
        (root / ("user-%d.slice" % uid)).mkdir(parents=True)

    def at(uid, current, peak=None):
        (root / ("user-%d.slice" % uid) / "memory.current").write_text("%d\n" % (current * mb))
        if peak is not None:
            (root / ("user-%d.slice" % uid) / "memory.peak").write_text("%d\n" % (peak * mb))

    at(1000, 900, 1200)
    at(1001, 300)                 # a kernel without memory.peak: the samples only
    agent.sample(str(root), samples)
    at(1000, 2500, 2600)
    at(1001, 700)
    agent.sample(str(root), samples)
    at(1000, 1100, 2600)
    at(1001, 400)
    # the first beat: the lifetime peak is not known to be recent, the samples are
    assert agent.places_memory(str(root), samples) == {"1": {"mem_mb": 1100, "mem_peak_mb": 2500},
                                                       "2": {"mem_mb": 400, "mem_peak_mb": 700}}
    # the next: a high between two samples shows in memory.peak (risen since the last beat)
    at(1000, 1000, 3100)
    assert agent.places_memory(str(root), samples) == {"1": {"mem_mb": 1000, "mem_peak_mb": 3100},
                                                       "2": {"mem_mb": 400, "mem_peak_mb": 400}}
    # and then nothing higher: the peak is now
    assert agent.places_memory(str(root), samples)["1"] == {"mem_mb": 1000, "mem_peak_mb": 1000}
    # a place whose services do not run holds nothing
    shutil.rmtree(root / "user-1001.slice")
    assert agent.places_memory(str(root), samples)["2"] == {"mem_mb": 0, "mem_peak_mb": 0}
    # the sampler's units, written once
    units, ran = tmp_path / "units", []
    units.mkdir()
    agent.sampler("/usr/local/lib/bipagents", str(units), lambda *args: ran.append(args))
    agent.sampler("/usr/local/lib/bipagents", str(units), lambda *args: ran.append(args))
    assert ran == [("systemctl", "daemon-reload"), ("systemctl", "enable", "--now", "bipagents-memory.timer")]
    assert "ExecStart=/usr/bin/python3 /usr/local/lib/bipagents/seat-agent.py --sample" in \
        (units / "bipagents-memory.service").read_text()
    assert "OnUnitActiveSec=15s" in (units / "bipagents-memory.timer").read_text()


def test_the_beat_s_body_and_its_order(tmp_path, monkeypatch):
    agent = _seat_agent()
    conf = tmp_path / "guichet.json"
    conf.write_text('{"url": "https://g", "token": "t", "server": "vps-1"}')
    monkeypatch.setattr(agent, "CONF", str(conf))
    monkeypatch.setattr(agent, "SETTINGS", str(tmp_path / "settings.json"))
    steps, sent = [], []
    monkeypatch.setattr(agent, "health", lambda: {"1": {"up": True}})
    monkeypatch.setattr(agent, "vitals", lambda: {"memory_mb": 7812})
    monkeypatch.setattr(agent, "places_memory", lambda: {"1": {"mem_mb": 812, "mem_peak_mb": 1630}})
    monkeypatch.setattr(agent, "places_activity", lambda memory: memory)
    monkeypatch.setattr(agent, "memory_settled", lambda server: steps.append(("settled", server["memory_mb"])))
    monkeypatch.setattr(agent, "zram", lambda: steps.append("zram"))

    def call(conf, method, path, body=None):
        sent.append((path, body))
        return {"orders": [], "settings": {"memory_high_share": 0.3}}

    monkeypatch.setattr(agent, "call", call)
    assert agent.main([]) == 0
    assert sent == [("/v1/server/beat", {"seats": {"1": {"up": True}}, "server": {"memory_mb": 7812},
                                         "places": {"1": {"mem_mb": 812, "mem_peak_mb": 1630}}})]
    assert steps == [("settled", 7812), ("settled", 7812), "zram"]
    assert json.loads((tmp_path / "settings.json").read_text()) == {"memory_high_share": 0.3}


def test_the_beat_says_what_each_place_s_bridge_files_say_of_it(tmp_path, monkeypatch):
    """busy, tz, last_active_at, next_routine_at: read from the bridge's and Hermes' files, left out when unknown."""
    import os
    agent = _seat_agent()
    home = tmp_path / "bip"
    assert agent.activity(str(home), now=1793000000) == {"busy": False}
    state = home / ".local" / "state" / "bipagents"
    state.mkdir(parents=True)
    (state / "busy").write_text("1793000000")
    os.utime(state / "busy", (1792999950, 1792999950))
    data = home / ".local" / "share" / "hermes-bridge"
    data.mkdir(parents=True)
    (data / "time.json").write_text('{"time_zone": "Europe/Paris", "device": "Asia/Tokyo"}')
    (data / "sleep.json").write_text('{"last": 1792990000.5, "asleep": false}')
    for agent_name, jobs in (("vie", [{"next_run_at": "2026-10-27T07:00:00+01:00", "enabled": True},
                                      {"next_run_at": "2026-10-27T05:00:00+01:00", "state": "paused"}]),
                             ("lea", {"jobs": [{"next_run_at": "2026-10-27T06:30:00+00:00"},
                                               {"next_run_at": "nonsense"}]})):
        folder = home / ".hermes" / "profiles" / agent_name / "cron"
        folder.mkdir(parents=True)
        (folder / "jobs.json").write_text(json.dumps(jobs))
    assert agent.activity(str(home), now=1793000000) == {
        "busy": True, "tz": "Europe/Paris", "last_active_at": "2026-10-26T04:46:40Z",
        "next_routine_at": "2026-10-27T06:00:00Z"}
    # a marker the bridge stopped refreshing (it stopped): not busy; a zone that is not one: left out
    (data / "time.json").write_text('{"time_zone": "", "device": "$(reboot)"}')
    assert agent.activity(str(home), now=1793000000 + 600) == {
        "busy": False, "last_active_at": "2026-10-26T04:46:40Z", "next_routine_at": "2026-10-27T06:00:00Z"}
    _places(agent, monkeypatch, {"bip": 1000})
    monkeypatch.setattr(agent.pwd, "getpwnam", lambda name: type("P", (), {"pw_dir": str(home)})()
                        if name == "bip" else (_ for _ in ()).throw(KeyError(name)))
    places = agent.places_activity({"1": {"mem_mb": 800, "mem_peak_mb": 900}, "2": {"mem_mb": 0, "mem_peak_mb": 0}})
    assert places["1"]["mem_mb"] == 800 and "next_routine_at" in places["1"]
    assert places["2"] == {"mem_mb": 0, "mem_peak_mb": 0}


def test_zram_is_set_up_on_a_server_without_it_with_user_data_s_own_settings(tmp_path):
    import os
    agent = _seat_agent()
    data = yaml.safe_load(render("x" * 43))
    line = next(c for c in data["runcmd"] if "zram-generator.conf" in c)
    assert "printf '%s'" % agent.ZRAM_CONF.replace("\n", "\\n") in line
    swaps = tmp_path / "swaps"
    swaps.write_text("Filename\tType\tSize\tUsed\tPriority\n/swapfile file 2097148 0 -2\n")
    tried, conf, ran = tmp_path / "lib" / "zram-tried", tmp_path / "zram-generator.conf", []

    def runner(*args):
        ran.append(args)
        if args[0] == "systemctl" and args[-1] == "dev-zram0.swap":
            swaps.write_text(swaps.read_text() + "/dev/zram0 partition 4000000 0 100\n")
        return 0

    assert agent.zram(str(swaps), str(tried), str(conf), runner) is True
    assert conf.read_text() == agent.ZRAM_CONF
    assert ran[:2] == [("modprobe", "zram"), ("apt-get", "install", "-y", "systemd-zram-generator")]
    # the module kept apart (Ubuntu's cloud kernels): linux-modules-extra for the kernel running, then loaded
    swaps.write_text("Filename\tType\tSize\tUsed\tPriority\n")
    tried.unlink()
    ran.clear()
    loads = []

    def without_module(*args):
        ran.append(args)
        if args[0] == "modprobe":
            loads.append(1)
            return 1 if len(loads) == 1 else 0
        return runner(*args) if args[-1] == "dev-zram0.swap" else 0

    assert agent.zram(str(swaps), str(tried), str(conf), without_module) is True
    assert ran[:4] == [("modprobe", "zram"), ("apt-get", "install", "-y", "linux-modules-extra-" + os.uname().release),
                       ("modprobe", "zram"), ("apt-get", "install", "-y", "systemd-zram-generator")]
    ran.clear()
    assert agent.zram(str(swaps), str(tried), str(conf), runner) is True and ran == []   # on: nothing more
    # not possible (apt busy): tried again an hour later, not every two minutes
    swaps.write_text("Filename\tType\tSize\tUsed\tPriority\n")
    tried.unlink()
    assert agent.zram(str(swaps), str(tried), str(conf), lambda *a: ran.append(a) or 1) is False
    assert agent.zram(str(swaps), str(tried), str(conf), lambda *a: ran.append(a) or 1) is False
    assert ran == [("modprobe", "zram"), ("apt-get", "install", "-y", "linux-modules-extra-" + os.uname().release)]


def test_each_place_s_ai_credit_state_is_written_where_its_bridge_reads_it(tmp_path):
    """The beat's answer says each place's AI credit state (the guichet's credit_pool.rs); the place's bridge pauses its
    routines, browser and background tasks while it is "full" or "stopped" (bridge-rs/src/credit.rs)."""
    import os
    agent = _seat_agent()
    uid, gid = os.getuid(), os.getgid()
    one, two, three = tmp_path / "bip", tmp_path / "bip2", tmp_path / "bip3"
    for home in (one, two, three):
        home.mkdir()
    homes = {1: (str(one), uid, gid), 2: (str(two), uid, gid), 3: (str(three), uid, gid)}
    agent.keep_credit({"orders": []}, homes)   # an older guichet: nothing written
    assert not (one / agent.CREDIT).exists()
    agent.keep_credit({"credit": {"1": {"state": "full", "resumes": 1793491200},
                                  "2": {"state": "ok", "resumes": 1793491200},
                                  "3": {"state": "$(reboot)"}}}, homes)
    assert json.loads((one / agent.CREDIT).read_text()) == {"state": "full", "resumes": 1793491200.0}
    assert json.loads((two / agent.CREDIT).read_text())["state"] == "ok"
    assert not (three / agent.CREDIT).exists()
    assert oct((one / agent.CREDIT).stat().st_mode & 0o777) == "0o644"
    # a link the place put there is replaced, never followed
    target = tmp_path / "elsewhere"
    target.write_text("keep")
    os.unlink(two / agent.CREDIT)
    os.symlink(target, two / agent.CREDIT)
    agent.keep_credit({"credit": {"2": {"state": "stopped"}}}, homes)
    assert target.read_text() == "keep"
    assert json.loads((two / agent.CREDIT).read_text()) == {"state": "stopped"}
    # a place left out (its subscription over): its file removed, ok
    assert not (one / agent.CREDIT).exists()
    assert agent.CREDIT == ".local/state/bipagents/credit.json"
