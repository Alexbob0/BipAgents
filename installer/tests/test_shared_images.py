"""The agents' images once per server (bipinstall/shared_images.py): root's store, the accounts reading it, GHCR."""
import hashlib
import json
import subprocess
from pathlib import Path

import pytest

from bipinstall import browser_refresh, hermes, shared_images

ENGINE = "/usr/bin/podman"
ROOT = Path(__file__).resolve().parents[2]


class Podman:
    """A fake podman for root's store and the accounts: images by name, every command recorded."""

    def __init__(self, images=None, fail=(), runs=0):
        self.images = dict(images or {})   # name -> id
        self.fail, self.runs, self.ran = set(fail), runs, []
        self.registry = {shared_images.browser_ref("k1"): "b2", shared_images.SANDBOX_SOURCES[0]: "s1",
                         shared_images.SANDBOX: "s1"}

    def __call__(self, args, timeout=600):
        args = list(args)
        self.ran.append(args)
        words = args[7:] if "--root" in args else args[1:]
        if words[0] == "pull":
            source = words[-1]
            if source in self.fail or source not in self.registry:
                return 125, "manifest unknown"
            self.images[source] = self.registry[source]
            return 0, ""
        if words[:2] == ["image", "inspect"]:
            found = self.images.get(words[-1])
            return (0, "sha256:" + found) if found else (125, "no such image")
        if words[0] == "tag":
            self.images[words[2]] = self.images.get(words[1], words[1])
            return 0, ""
        if words[0] in ("untag", "rmi"):
            self.images.pop(words[-1], None)
            return 0, ""
        if words[0] == "run":
            return self.runs, "" if self.runs == 0 else "stat: /home/bip owned by 65534"
        return 0, ""

    def did(self, *words):
        return [a for a in self.ran if (a[7:] if "--root" in a else a[1:])[:len(words)] == list(words)]


@pytest.fixture
def setting(tmp_path, monkeypatch):
    path = tmp_path / "etc" / "shared-images.json"
    monkeypatch.setattr(shared_images, "SETTING", path)
    monkeypatch.setattr(shared_images, "browser_key", lambda repo=None: "k1")
    return path


def test_the_browser_key_is_the_workflow_hash_of_the_browser_folder():
    tree = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD:browser"], capture_output=True, text=True)
    if tree.returncode != 0:
        pytest.skip("not a git checkout")
    assert shared_images.browser_key(ROOT) == hashlib.sha256(tree.stdout.strip().encode()).hexdigest()[:12]
    assert shared_images.browser_ref("abc") == "ghcr.io/alexbob0/bipagents-browser:abc"
    workflow = (ROOT / ".github" / "workflows" / "server-images.yml").read_text(encoding="utf-8")
    assert "git rev-parse HEAD:browser" in workflow and "sha256sum | cut -c1-12" in workflow


def test_root_pulls_both_images_once_under_the_places_names_and_keeps_the_one_before(tmp_path, setting):
    podman = Podman(images={shared_images.BROWSER: "b1"})
    store = tmp_path / "store"
    assert shared_images.pull(store=store, run=podman) == 0
    assert podman.images[shared_images.BROWSER] == "b2" and podman.images[shared_images.SANDBOX] == "s1"
    assert podman.images["localhost/bipagents-browser:previous"] == "b1"   # a running browser may be on it
    assert shared_images.browser_ref("k1") not in podman.images                # only the place's name holds it
    assert podman.did("image", "prune", "--force")
    assert all(a[:7] == shared_images.store_podman(store) for a in podman.ran if a[0] == "podman")
    data = json.loads(setting.read_text())
    assert data["store"] == str(store) and data["browser"] == {"key": "k1", "id": "b2"} and data["sandbox"]["id"] == "s1"
    assert data["views"] == str(shared_images.VIEWS)
    assert oct(setting.stat().st_mode & 0o777) == "0o644"


def test_the_sandbox_comes_from_docker_hub_when_the_mirror_fails_and_a_failed_pull_keeps_the_store(tmp_path, setting):
    podman = Podman(fail={shared_images.SANDBOX_SOURCES[0]})
    assert shared_images.pull(store=tmp_path, run=podman) == 0
    assert podman.did("pull", "--quiet", shared_images.SANDBOX)
    podman = Podman(images=dict(podman.images), fail={shared_images.browser_ref("k1")})
    assert shared_images.pull(store=tmp_path, run=podman) == 0   # last night's browser is still there
    assert json.loads(setting.read_text())["browser"]["id"] == "b2"
    setting.unlink()
    assert shared_images.pull(store=tmp_path, run=Podman(fail={shared_images.browser_ref("k1")})) == 1


def test_root_pulls_again_each_night_before_the_places_refresh(tmp_path):
    units = dict(shared_images.units(Path("/usr/local/lib/bipagents/repo")))
    assert "ExecStart=/usr/bin/python3 -m bipinstall.shared_images pull" in units["bipagents-images.service"]
    assert "PYTHONPATH=/usr/local/lib/bipagents/repo/installer" in units["bipagents-images.service"]
    assert "OnCalendar=*-*-* 00:50:00" in units["bipagents-images.timer"]
    ran = []
    shared_images.install_timer(Path("/r"), tmp_path, run=lambda args, timeout=0: ran.append(args) or (0, ""))
    assert (tmp_path / "bipagents-images.timer").is_file()
    assert ["systemctl", "enable", "bipagents-images.timer", "bipagents-image-views.service"] in ran
    # The views again at each boot, before the places' own services start.
    views = units["bipagents-image-views.service"]
    assert "ExecStart=/usr/bin/python3 -m bipinstall.shared_images views" in views
    assert "Before=systemd-user-sessions.service" in views and "WantedBy=multi-user.target" in views


@pytest.fixture
def view(tmp_path, monkeypatch):
    """root mounted this account's view, fuse-overlayfs is there."""
    fuse = tmp_path / "fuse-overlayfs"
    fuse.write_text("")
    monkeypatch.setattr(shared_images, "FUSE_OVERLAYFS", str(fuse))
    mounted = {"/var/lib/bipagents/views/bip2"}
    monkeypatch.setattr(shared_images, "is_mounted", lambda p: str(p) in mounted)
    return mounted


SHARED = {"store": "/var/lib/bipagents/images", "views": "/var/lib/bipagents/views",
          "browser": {"key": "k1", "id": "b2"}, "sandbox": {"id": "s1"}}


def test_an_account_reads_its_view_of_the_store_when_both_images_run_right_from_it(tmp_path, setting, view):
    setting.parent.mkdir(parents=True)
    setting.write_text(json.dumps(SHARED))
    home = tmp_path / "home"
    podman = Podman()
    assert shared_images.use_shared(hermes.Runner(), ENGINE, home, run=podman, say=lambda t: None, user="bip2")
    conf = (home / shared_images.STORAGE_CONF).read_text()
    assert 'additionalimagestores = ["/var/lib/bipagents/views/bip2"]' in conf and 'driver = "overlay"' in conf
    # The browser's keep-id shifted as fuse-overlayfs reads it: the kernel's overlay would copy the whole image.
    assert f'[storage.options.overlay]\nmount_program = "{shared_images.FUSE_OVERLAYFS}"' in conf
    checks = podman.did("run")
    assert [c[-3] for c in checks] == [shared_images.SANDBOX, shared_images.BROWSER]
    assert all("--pull=never" in c and "--network=none" in c for c in checks)
    assert "--userns=keep-id:uid=1000,gid=1000" in checks[1] and not any("--userns" in a for a in checks[0])
    assert 'stat -c %u /home/bip)" = "$(id -u)"' in checks[1][-1]
    # Its browser then needs nothing: no pull, no build.
    runner = hermes.Runner()
    assert shared_images.browser(runner, ENGINE, home=home, run=podman, say=lambda t: None) == "shared"
    assert not podman.did("pull") and not any(" build " in line for line in runner.log)
    assert shared_images.source_of(home) == {"source": "shared", "key": "k1"}


def test_without_a_view_or_fuse_overlayfs_the_account_keeps_its_own_images(tmp_path, setting, view, monkeypatch):
    setting.parent.mkdir(parents=True)
    home, said = tmp_path / "home", []
    old = dict(SHARED)
    del old["views"]                   # pulled by an older BipAgents: root's store as it is, unusable rootless
    setting.write_text(json.dumps(old))
    assert not shared_images.use_shared(hermes.Runner(), ENGINE, home, run=Podman(), say=said.append, user="bip2")
    assert "no view" in said[-1] and not (home / shared_images.STORAGE_CONF).exists()
    setting.write_text(json.dumps(SHARED))
    assert not shared_images.use_shared(hermes.Runner(), ENGINE, home, run=Podman(), say=said.append, user="bip3")
    assert "no view" in said[-1]       # root mounted none for this account (an older kernel, say)
    monkeypatch.setattr(shared_images, "FUSE_OVERLAYFS", str(tmp_path / "missing"))
    assert not shared_images.use_shared(hermes.Runner(), ENGINE, home, run=Podman(), say=said.append, user="bip2")
    assert "fuse-overlayfs" in said[-1] and not (home / shared_images.STORAGE_CONF).exists()


def test_an_account_where_the_store_does_not_run_right_takes_the_setting_back(tmp_path, setting, view):
    setting.parent.mkdir(parents=True)
    setting.write_text(json.dumps({"store": "/var/lib/bipagents/images", "views": "/var/lib/bipagents/views",
                                   "sandbox": {"id": "s1"}}))
    home = tmp_path / "home"
    said = []
    assert not shared_images.use_shared(hermes.Runner(), ENGINE, home, run=Podman(runs=1), say=said.append, user="bip2")
    assert not (home / shared_images.STORAGE_CONF).exists() and "gets its own" in said[-1]
    own = home / shared_images.STORAGE_CONF
    own.parent.mkdir(parents=True, exist_ok=True)
    own.write_text("[storage]\ndriver = \"vfs\"\n")
    assert not shared_images.use_shared(hermes.Runner(), ENGINE, home, run=Podman(), say=said.append, user="bip2")
    assert own.read_text() == "[storage]\ndriver = \"vfs\"\n"   # someone's own setting stays
    assert not shared_images.use_shared(hermes.Runner(), "/usr/bin/docker", home, run=Podman(), say=said.append)
    setting.unlink()
    assert not shared_images.use_shared(hermes.Runner(), ENGINE, tmp_path / "other", run=Podman(), say=said.append)


def test_each_account_s_view_maps_the_store_s_ids_as_its_podman_does():
    # The store's root is the account, its ids 1, 2... the account's subordinate ids in /etc/subuid's order.
    assert shared_images.idmap(1001, 1001, [(300000, 65536)], [(300000, 65536)]) == (
        "u:0:1001:1 u:1:300000:65536 g:0:1001:1 g:1:300000:65536")
    assert shared_images.idmap(1002, 1005, [(165536, 65536), (400000, 65536)], [(400000, 10)]) == (
        "u:0:1002:1 u:1:165536:65536 u:65537:400000:65536 g:0:1005:1 g:1:400000:10")


def test_a_view_is_mounted_when_mountinfo_has_it_bind_mounts_of_the_same_disk_too(tmp_path):
    info = tmp_path / "mountinfo"
    info.write_text("29 1 252:1 / / rw,relatime shared:1 - ext4 /dev/vda1 rw\n"
                    "312 29 252:1 /var/lib/bipagents/images /var/lib/bipagents/views/bip2 ro,relatime,idmapped "
                    "shared:1 - ext4 /dev/vda1 rw\n"
                    "313 29 252:1 /x /srv/a\\040b rw shared:1 - ext4 /dev/vda1 rw\n")
    assert shared_images.is_mounted(Path("/var/lib/bipagents/views/bip2"), info)
    assert shared_images.is_mounted(Path("/var/lib/bipagents/views/bip2/"), info)
    assert not shared_images.is_mounted(Path("/var/lib/bipagents/views/bip3"), info)
    assert shared_images.is_mounted(Path("/srv/a b"), info)
    assert not shared_images.is_mounted(Path("/x"), tmp_path / "missing")


def test_root_mounts_one_read_only_view_per_account_once(tmp_path, monkeypatch):
    store, folder = tmp_path / "lib" / "images", tmp_path / "lib" / "views"
    (store / "overlay-images").mkdir(parents=True)
    subuid, subgid = tmp_path / "subuid", tmp_path / "subgid"
    subuid.write_text("lab:100000:65536\nbip:200000:65536\nbip2:400000:65536\nbip2:900000:10\ngone:1:2\nbad line\n")
    subgid.write_text("bip:200000:65536\nbip2:400000:65536\n")
    accounts = {"lab": (999, 999), "bip": (1000, 1000), "bip2": (1001, 1001)}

    def account(name):
        if name not in accounts:
            raise KeyError(name)
        return type("P", (), {"pw_uid": accounts[name][0], "pw_gid": accounts[name][1]})()
    ran, mounted = [], {str(folder / "bip")}
    got = shared_images.views(store, folder, run=lambda a, t=0: ran.append(a) or (0, ""), files=(subuid, subgid),
                              account=account, mounted_at=lambda p: str(p) in mounted)
    assert got == ["bip", "bip2"]          # bip's was mounted already (its browser may be reading it): left as it is
    assert len(ran) == 1 and ran[0][:3] == ["mount", "--bind", "-o"]
    assert ran[0][3] == ("ro,X-mount.idmap=u:0:1001:1 u:1:400000:65536 u:65537:900000:10 "
                         "g:0:1001:1 g:1:400000:65536") and ran[0][4:] == [str(store), str(folder / "bip2")]
    assert oct(store.stat().st_mode & 0o777) == "0o700" and oct(store.parent.stat().st_mode & 0o777) == "0o711"
    assert oct(folder.stat().st_mode & 0o777) == "0o711"
    # Under the mount point, an empty store: an account whose view isn't mounted keeps a podman that works.
    for name in shared_images.SKELETON:
        assert (folder / "bip2" / name).is_file()
    said = []
    monkeypatch.setattr(shared_images, "say", said.append)
    assert shared_images.views(store, folder, run=lambda a, t=0: (32, "mount: bad option"), files=(subuid, subgid),
                               account=account, mounted_at=lambda p: False) == []
    assert "bad option" in said[-1]
    assert shared_images.views(tmp_path / "empty", folder, run=lambda a, t=0: (0, "")) == []   # nothing pulled yet


class Engine(hermes.Runner):
    """A real-looking run of the account's podman: a pull that fails when asked to, everything noted."""

    def __init__(self, pull_fails=False):
        super().__init__()
        self.pull_fails = pull_fails

    def run(self, args, **kwargs):
        self.log.append(" ".join(args))
        if args[1] == "pull" and self.pull_fails:
            raise RuntimeError("podman pull failed (125): manifest unknown")
        return ""


def test_without_the_store_the_browser_is_pulled_from_ghcr_else_built(tmp_path, setting):
    home = tmp_path / "home"
    runner = Engine()
    assert shared_images.browser(runner, ENGINE, home=home, run=Podman(), say=lambda t: None) == "registry"
    assert runner.log[:3] == [f"{ENGINE} pull --quiet ghcr.io/alexbob0/bipagents-browser:k1",
                              f"{ENGINE} tag ghcr.io/alexbob0/bipagents-browser:k1 bipagents-browser",
                              f"{ENGINE} rmi ghcr.io/alexbob0/bipagents-browser:k1"]
    assert shared_images.source_of(home)["source"] == "registry"
    runner, said = Engine(pull_fails=True), []
    assert shared_images.browser(runner, ENGINE, home=home, run=Podman(), say=said.append) == "built"
    assert f"{ENGINE} build --pull=newer" in runner.log[-1] and "building it" in said[-1]
    assert shared_images.source_of(home) == {"source": "built"}
    dry = hermes.Runner(dry=True)
    assert shared_images.browser(dry, ENGINE, home=home) == "registry"
    assert dry.log[-1].startswith("when the registry has none: ") and " build " in dry.log[-1]


def test_the_nightly_refresh_follows_where_the_browser_came_from(tmp_path, monkeypatch):
    monkeypatch.setattr(shared_images, "browser_key", lambda repo=None: "k1")
    home = tmp_path
    shared_images.remember(home, "shared", key="k1")
    podman = Podman(images={"bipagents-browser": "b1"})
    assert browser_refresh.refresh(ENGINE, Path("/repo"), home, run=podman) == 0
    assert not podman.did("build") and not podman.did("pull")   # root pulls it for the whole server
    shared_images.remember(home, "registry", key="k1")
    podman = Podman(images={"bipagents-browser": "b1"})
    podman.runs = 0

    def answer(args, timeout=600):
        if args[1] == "run":   # the smoke test of the new image
            podman.ran.append(list(args))
            return 0, "Google Chrome 156.0.8100.12"
        return podman(args, timeout)
    assert browser_refresh.refresh(ENGINE, Path("/repo"), home, run=answer) == 0
    assert podman.did("pull", "--quiet", "ghcr.io/alexbob0/bipagents-browser:k1")
    assert podman.images["bipagents-browser"] == "b2" and not podman.did("build")
