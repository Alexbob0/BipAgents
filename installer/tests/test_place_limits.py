"""The places sharing one server of the offer kept from each other (installer/cloud/seat-agent.py, 2026-10-30): each
on a disk of its own of 5 GB, watched (a note to its agents at 80 %, Kerfy told and, without its own disk, its
agents' workspaces paused at 95 %). The memory and CPU shares: test_cloud_init.py."""
import importlib.util
import json
import os
import pwd
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[2]
GB = 1024 ** 3


def _seat_agent():
    spec = importlib.util.spec_from_file_location("seat_agent", ROOT / "installer" / "cloud" / "seat-agent.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _account(agent, monkeypatch, name, home):
    def getpwnam(wanted):
        if wanted != name:
            raise KeyError(wanted)
        return pwd.struct_passwd((name, "x", os.getuid(), os.getgid(), "", str(home), "/bin/bash"))
    monkeypatch.setattr(agent.pwd, "getpwnam", getpwnam)


def test_the_disk_levels_and_their_way_back():
    agent = _seat_agent()
    level = agent.disk_level
    assert level(3.9, 5, None) == "ok" and level(4.0, 5, "ok") == "low" and level(4.75, 5, "low") == "full"
    # left only once clearly under: full until 90 %, low until 75 %
    assert level(4.6, 5, "full") == "full" and level(4.4, 5, "full") == "low"
    assert level(3.8, 5, "low") == "low" and level(3.7, 5, "low") == "ok" and level(1, 0, None) == "ok"


def test_each_place_s_disk_is_watched_and_its_bridge_told(tmp_path, capsys):
    """Place 1 on its own disk (statvfs), place 2 not yet (du against 5 GB): levels for the beat, the file each
    bridge reads (a pause only for a place without its own disk), measured every 5 minutes, a full disk logged once."""
    agent = _seat_agent()
    one, two = tmp_path / "bip", tmp_path / "bip2"
    one.mkdir()
    two.mkdir()
    mountinfo = tmp_path / "mountinfo"
    mountinfo.write_text("22 1 8:1 / / rw - ext4 /dev/sda1 rw\n"
                         "90 22 7:0 / %s rw,nosuid,nodev - ext4 /dev/loop0 rw\n" % one)
    homes = {"1": (str(one), os.getuid(), os.getgid()), "2": (str(two), os.getuid(), os.getgid())}
    used = {"1": 4.1 * GB, "2": 1 * GB}

    def statvfs(path):
        assert path == str(one)
        return SimpleNamespace(f_blocks=5 * GB // 4096, f_frsize=4096, f_bfree=int((5 * GB - used["1"]) // 4096))

    state = tmp_path / "disk.json"

    def watch(now):
        return agent.disk_watch({}, now, homes, str(state), str(mountinfo), statvfs, lambda home: used["2"],
                                folder=str(tmp_path / "run"), make_one=lambda home: False)

    found = watch(1000)
    assert found["1"]["disk"] == "low" and found["1"]["own_disk"] is True and found["1"]["disk_size_mb"] == 5120
    assert found["2"] == {"disk_mb": 1024, "disk_size_mb": 5120, "disk": "ok", "own_disk": False}
    bridge = json.loads((one / ".local/state/bipagents/disk.json").read_text())
    assert bridge == {"state": "low", "pause": False, "at": 1000}
    # within 5 minutes: nothing measured again
    used["2"] = 5 * GB
    assert watch(1200) == found
    # place 2 over 95 % of its 5 GB without a disk of its own: paused; place 1 full on its own disk: told, not paused
    used["1"] = 4.9 * GB
    found = watch(1400)
    assert found["2"]["disk"] == "full" and found["1"]["disk"] == "full"
    assert json.loads((two / ".local/state/bipagents/disk.json").read_text())["pause"] is True
    assert json.loads((one / ".local/state/bipagents/disk.json").read_text())["pause"] is False
    watch(1800)
    out = capsys.readouterr().out
    assert out.count("place 2: disk full") == 1 and "workspaces paused" in out and out.count("place 1: disk full") == 1
    # back under 90 %: resumed (the bridge reads pause false), logged once
    used["2"] = 4.4 * GB
    found = watch(2200)
    assert found["2"]["disk"] == "low"
    assert json.loads((two / ".local/state/bipagents/disk.json").read_text()) == {"state": "low", "pause": False,
                                                                                  "at": 2200}
    assert capsys.readouterr().out.count("place 2: room on its disk again") == 1
    # the beat's places get each one's disk
    assert agent.with_disk({"1": {"mem_mb": 3}}, {"1": {"disk": "ok"}, "2": {"disk": "low"}}) == \
        {"1": {"mem_mb": 3, "disk": "ok"}, "2": {"disk": "low"}}


class Root:
    """The commands own_disk runs, a mount as a line in mountinfo, and the copy done by hand."""

    def __init__(self, mountinfo, home, fail=()):
        self.mountinfo, self.home, self.fail, self.calls = mountinfo, home, fail, []

    def __call__(self, *args):
        self.calls.append(args)
        if args[0] in self.fail or args[:2] in self.fail:
            return 1
        if args[0] == "cp":
            os.system("cp -a '%s' '%s'" % (args[3], args[4]))
        if args[:3] == ("systemctl", "enable", "--now"):
            with open(self.mountinfo, "a") as fh:
                fh.write("91 22 7:1 / %s rw - ext4 /dev/loop1 rw\n" % self.home)
        return 0


def test_a_place_gets_a_disk_of_its_own_with_its_files(tmp_path, monkeypatch):
    agent = _seat_agent()
    home = tmp_path / "home" / "bip2"
    (home / ".config").mkdir(parents=True)
    (home / ".config" / "kept.txt").write_text("mine")
    _account(agent, monkeypatch, "bip2", home)
    mountinfo = tmp_path / "mountinfo"
    mountinfo.write_text("22 1 8:1 / / rw - ext4 /dev/sda1 rw\n")
    units, disks = tmp_path / "units", tmp_path / "places"
    units.mkdir()
    root = Root(str(mountinfo), str(home))
    assert agent.own_disk("bip2", {}, str(disks), str(units), str(mountinfo), root, lambda h: 10 * 1024 * 1024,
                          now=1000, staging_root=str(tmp_path / "run"))
    image = disks / "bip2.img"
    assert image.stat().st_size == 5 * GB and image.stat().st_blocks * 512 < 1024 * 1024   # sparse
    mkfs = next(c for c in root.calls if c[0] == "mkfs.ext4")
    assert mkfs == ("mkfs.ext4", "-q", "-F", "-m", "0", "-i", "8192", "-E",
                    "root_owner=%d:%d" % (os.getuid(), os.getgid()), str(image))
    # its services stopped before the copy, started again only when they ran by themselves (lingering)
    names = [c[:2] for c in root.calls]
    assert names.index(("systemctl", "stop")) < names.index(("cp", "-a"))
    # its old home kept aside, its files copied, an empty home where the disk is mounted, at every boot
    assert (disks / "bip2.before-1000" / ".config" / "kept.txt").read_text() == "mine"
    assert (tmp_path / "run" / "disk-bip2" / ".config" / "kept.txt").exists()
    unit_name = str(home).strip("/").replace("/", "-") + ".mount"
    unit = (units / unit_name).read_text()
    assert "What=%s\n" % image in unit and "Where=%s\n" % home in unit and "Options=loop,discard,nosuid,nodev" in unit
    assert (units / ("user@%d.service.d" % os.getuid()) / "50-bipagents-disk.conf").read_text() == \
        "[Unit]\nRequiresMountsFor=%s\n" % home
    assert home.is_dir() and not any(home.iterdir())
    # done again: nothing more
    root.calls.clear()
    assert agent.own_disk("bip2", {}, str(disks), str(units), str(mountinfo), root, lambda h: 0) and not root.calls
    # the old home goes after 7 days, not before
    agent.old_homes(str(disks), now=1000 + 6 * 86400)
    assert (disks / "bip2.before-1000").exists()
    agent.old_homes(str(disks), now=1000 + 8 * 86400)
    assert not (disks / "bip2.before-1000").exists() and image.exists()
    # its account removed: its disk goes with it
    root.calls.clear()
    agent.drop_disk("bip2", str(disks), str(units), root)
    assert ("systemctl", "disable", "--now", unit_name) in root.calls
    assert not image.exists() and not (units / unit_name).exists()


def test_a_place_whose_files_don_t_fit_or_whose_disk_won_t_mount_stays_as_it_was(tmp_path, monkeypatch):
    agent = _seat_agent()
    home = tmp_path / "home" / "bip3"
    home.mkdir(parents=True)
    (home / "notes.txt").write_text("mine")
    _account(agent, monkeypatch, "bip3", home)
    mountinfo = tmp_path / "mountinfo"
    mountinfo.write_text("")
    units, disks = tmp_path / "units", tmp_path / "places"
    units.mkdir()
    root = Root(str(mountinfo), str(home))
    assert not agent.own_disk("bip3", {}, str(disks), str(units), str(mountinfo), root, lambda h: 4.6 * GB)
    assert not root.calls and not disks.exists()
    # a bigger disk from the guichet's settings: it fits
    assert agent.disk_gib({"place_disk_gib": 8}) == 8 and agent.disk_gib({"place_disk_gib": True}) == 5
    # the mount fails: its files back where they were, its services started again, no image left
    root = Root(str(mountinfo), str(home), fail=(("systemctl", "enable"),))
    assert not agent.own_disk("bip3", {}, str(disks), str(units), str(mountinfo), root, lambda h: 0, now=5,
                              staging_root=str(tmp_path / "run"))
    assert (home / "notes.txt").read_text() == "mine" and not (disks / "bip3.img").exists()
    assert not (disks / "bip3.before-5").exists() and not list(units.glob("*.mount"))
    assert root.calls[-1] == ("systemctl", "start", "user@%d.service" % os.getuid())
    # the copy fails: the same
    root = Root(str(mountinfo), str(home), fail=("cp",))
    assert not agent.own_disk("bip3", {}, str(disks), str(units), str(mountinfo), root, lambda h: 0, now=6,
                              staging_root=str(tmp_path / "run"))
    assert (home / "notes.txt").exists() and not (disks / "bip3.img").exists()


def test_an_update_brings_the_seat_scripts_from_root_s_checkout(tmp_path):
    agent = _seat_agent()
    repo, lib = tmp_path / "repo", tmp_path / "lib"
    (repo / "installer" / "cloud").mkdir(parents=True)
    lib.mkdir()
    (repo / "installer" / "cloud" / "seat-agent.py").write_text("new agent")
    (repo / "installer" / "cloud" / "add-seat.sh").write_text("same")
    (lib / "seat-agent.py").write_text("old agent")
    (lib / "add-seat.sh").write_text("same")
    agent.refresh_tools(str(repo), str(lib))
    assert (lib / "seat-agent.py").read_text() == "new agent" and (lib / "add-seat.sh").read_text() == "same"
    assert not (lib / "place-move.py").exists() and os.access(lib / "seat-agent.py", os.X_OK)


def test_new_places_get_their_own_disk_before_their_install():
    seat = (ROOT / "installer" / "cloud" / "add-seat.sh").read_text(encoding="utf-8")
    assert seat.index("--own-disk") < seat.index("enable-linger") < seat.index("install.sh")
    data = (ROOT / "installer" / "cloud" / "user-data.yaml").read_text(encoding="utf-8")
    assert data.index("--own-disk 1") < data.index("loginctl enable-linger bip") < data.index("--cloud /etc/bipagents")


MB = 1024 ** 2


class Disk:
    """A place's own disk as statvfs tells it, with root's reserve in it."""

    def __init__(self, size, used, reserve):
        self.size, self.used, self.reserve = size, used, reserve

    def statvfs(self, path):
        taken = self.used + self.reserve
        free = (self.size - taken) // 4096
        return SimpleNamespace(f_blocks=self.size // 4096, f_frsize=4096, f_bfree=free, f_bavail=free)


def test_a_full_own_disk_gives_its_reserve_back_and_says_so_at_once(tmp_path, capsys):
    """Real-server test of 2026-10-31: at 100 % Hermes answered 500 and the bridge stayed on « low » (the seat agent
    could not write « full » in the place). The reserve is given back within 15 s, the level is told in memory as well,
    and a place that fills that room too has its workspaces paused before Hermes runs dry."""
    agent = _seat_agent()
    home = tmp_path / "bip"
    home.mkdir()
    mountinfo = tmp_path / "mountinfo"
    mountinfo.write_text("90 22 7:0 / %s rw - ext4 /dev/loop0 rw\n" % home)
    homes = {"1": (str(home), os.getuid(), os.getgid())}
    run, state = tmp_path / "run", tmp_path / "disk.json"
    disk = Disk(5 * GB, 3 * GB, 0)
    agent.reserve_bytes = lambda path, owner=0: disk.reserve   # root's file, as statvfs counts it
    made, given = [], []

    def make_one(path):
        made.append(path)
        disk.reserve = agent.RESERVE_MB * MB
        return True

    def release(path):
        held, disk.reserve = disk.reserve, 0
        given.append(path)
        return held

    def watch(now):
        return agent.disk_watch({}, now, homes, str(state), str(mountinfo), disk.statvfs, None, folder=str(run),
                                release=release, make_one=make_one)

    def quick(now):
        agent.disk_quick(now, homes, str(state), str(mountinfo), disk.statvfs, folder=str(run), release=release)

    def told():
        return json.loads((run / ("%d.json" % os.getuid())).read_text())

    # the round takes the reserve once there is room for it twice; the levels leave it out
    assert watch(1000)["1"]["disk"] == "ok" and made == [str(home)]
    assert watch(1400)["1"]["disk_mb"] == 3 * 1024
    assert told() == {"state": "ok", "pause": False, "at": 1400}
    # filled to the brim between two rounds: the next 15 s sample gives the reserve back and tells « full » at once
    disk.used = 5 * GB - agent.RESERVE_MB * MB
    quick(1410)
    assert given == [str(home)] and disk.reserve == 0
    assert told() == {"state": "full", "pause": False, "at": 1410}
    assert json.loads((home / ".local/state/bipagents/disk.json").read_text())["state"] == "full"
    assert json.loads(state.read_text())["places"]["1"]["disk"] == "full"
    out = capsys.readouterr().out
    assert "reserve given back (192 MB)" in out and "disk full" in out
    # the same again: nothing new said, the in-memory word kept fresh
    quick(1425)
    assert told()["at"] == 1425 and capsys.readouterr().out == ""
    # it fills that room too: its workspaces paused while Hermes keeps the rest; no reserve taken while full
    disk.used = 5 * GB - 50 * MB
    quick(1440)
    assert told() == {"state": "full", "pause": True, "at": 1440}
    assert watch(1800)["1"]["disk"] == "full" and len(made) == 1 and told()["pause"] is True
    # some room made: still paused until it holds more than a reserve's worth, then lifted
    disk.used = 5 * GB - 150 * MB
    quick(1815)
    assert told()["pause"] is True
    disk.used = 5 * GB - 250 * MB
    quick(1830)
    assert told() == {"state": "full", "pause": False, "at": 1830}
    # under 90 %: out of « full », and the round takes the reserve again
    disk.used = 4 * GB
    assert watch(2200)["1"]["disk"] == "low" and len(made) == 2 and disk.reserve


def test_the_reserve_is_root_s_and_given_back_only_as_root_s(tmp_path):
    agent = _seat_agent()
    home = tmp_path / "bip"
    home.mkdir()
    reserve = home / agent.RESERVE
    reserve.write_bytes(b"\0" * 8192)
    # not root's (a file of the place under that name): neither counted nor removed
    assert agent.reserve_bytes(str(home)) == 0 and agent.release_reserve(str(home)) == 0 and reserve.exists()
    owner = os.getuid()
    assert agent.reserve_bytes(str(home), owner) >= 8192
    assert agent.release_reserve(str(home), owner) >= 8192 and not reserve.exists()
    assert agent.release_reserve(str(home), owner) == 0
    # a link under that name is never followed
    target = tmp_path / "elsewhere"
    target.write_bytes(b"x" * 4096)
    reserve.symlink_to(target)
    assert agent.release_reserve(str(home), owner) == 0 and target.read_bytes() == b"x" * 4096
    assert not agent.make_reserve(str(home))   # the name taken: not made
