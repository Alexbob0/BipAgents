"""The agents' own podman volumes (adopt's working folders) in `bipagents export` / import and in the hourly backup /
restore, with a fake podman whose volumes are folders of the test."""
import json
import os
import shutil
import sys
import tarfile
from pathlib import Path
from types import SimpleNamespace

import pytest

from bipinstall import backup, moving, sealed, volumes

FAKE = r'''import os, sys, tarfile
from pathlib import Path
store = Path(os.environ["FAKE_VOLUMES"])
args = sys.argv[1:]
if args[:2] == ["volume", "export"]:
    folder = store / args[2]
    if not folder.is_dir():
        sys.exit(125)
    with tarfile.open(args[args.index("--output") + 1], "w") as tar:
        for item in sorted(folder.rglob("*")):
            tar.add(item, item.relative_to(folder).as_posix(), recursive=False)
elif args[:2] == ["volume", "create"]:
    (store / args[-1]).mkdir(parents=True, exist_ok=True)
elif args[:2] == ["volume", "import"]:
    folder = store / args[2]
    if not folder.is_dir():
        sys.exit(125)
    with tarfile.open(args[3]) as tar:
        tar.extractall(folder, filter="tar")
'''

ADOPTED = {"adopted": {"vie": {"volumes": [
    {"name": "bipagents-vie-notes", "kind": "work", "mountpoint": "/x/notes"},
    {"name": "bipagents-vie-skills", "kind": "code", "mountpoint": "/x/skills"}]}},
    "shares": {"vie": {"folder": "/x/share"}}, "agents": {"vie": {"files": "/x/f", "outbox": "/x/o"}}}


@pytest.fixture
def podman(tmp_path, monkeypatch):
    """A fake `podman` first on the PATH; returns its volumes' folder."""
    bin_dir, store = tmp_path / "bin", tmp_path / "podman-volumes"
    bin_dir.mkdir()
    store.mkdir()
    script = bin_dir / "podman"
    script.write_text(f"#!{sys.executable}\n{FAKE}")
    script.chmod(0o755)
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")
    monkeypatch.setenv("FAKE_VOLUMES", str(store))
    notes = store / "bipagents-vie-notes"
    (notes / "plans").mkdir(parents=True)
    (notes / "plans" / "semaine.md").write_text("Lundi : piscine.")
    (store / "bipagents-vie-skills").mkdir()
    (store / "bipagents-vie-skills" / "run.sh").write_text("echo")
    return store


def test_only_the_volumes_whose_content_lives_nowhere_else_are_kept(tmp_path):
    (tmp_path / "sandbox.json").write_text(json.dumps(ADOPTED))
    assert volumes.kept(tmp_path) == ["bipagents-vie-notes"]       # not code, shares, uploads' copy or outbox
    linked = json.loads(json.dumps(ADOPTED))
    linked["adopted"]["wellness"] = {"volumes": [{"name": "bipagents-wellness-data", "kind": "link",
                                                  "mountpoint": "/x/data", "host": "/h/data"}]}
    (tmp_path / "sandbox.json").write_text(json.dumps(linked))
    assert volumes.kept(tmp_path) == ["bipagents-vie-notes", "bipagents-wellness-data"]   # a linked folder's, too
    (tmp_path / "sandbox.json").write_text("{not json")
    assert volumes.kept(tmp_path) == [] and volumes.kept(tmp_path / "none") == []


def a_home(root: Path) -> tuple:
    home = root / "home" / "alex"
    data = home / ".local" / "share" / "bipagents"
    config = home / ".config" / "bipagents" / "bridge.toml"
    config.parent.mkdir(parents=True)
    config.write_text(f'bridge_key = "k"\ndata_dir = "{data}"\n\n[hermes]\nhome = "{home / ".hermes"}"\n')
    (home / ".hermes").mkdir(parents=True)
    data.mkdir(parents=True)
    (data / "sandbox.json").write_text(json.dumps(ADOPTED))
    return home, config


def test_an_export_takes_the_agents_volumes_and_an_import_puts_them_back(tmp_path, podman, monkeypatch):
    from bipinstall import hermes
    monkeypatch.setattr(hermes, "hermes_checkout", lambda: tmp_path / "no-git")
    home, config = a_home(tmp_path)
    phrase = sealed.new_passphrase()
    out = tmp_path / "export.bipagents"
    inv = moving.read_inventory(config, home)
    info, tally = moving.export(inv, out, phrase, SimpleNamespace(system="linux", tailnet_name=None, tailscale=None))
    assert info["volumes"] == ["bipagents-vie-notes"] and not tally.skipped
    shutil.rmtree(podman)                                   # another machine: no volume yet
    podman.mkdir()
    new = tmp_path / "home" / "lea"
    target = moving.Target(home=new, repo=moving.REPO, tailnet_name=None, lan_address=None)
    tar, opened, handle = moving.open_archive(out, phrase)
    moving.unpack(tar, opened, target, new / ".config" / "bipagents" / "bridge.toml")
    tar.close()
    handle.close()
    assert (podman / "bipagents-vie-notes" / "plans" / "semaine.md").read_text() == "Lundi : piscine."
    assert not (podman / "bipagents-vie-skills").exists()   # refreshed from its folder by the bridge
    assert not list((new / moving.VOLUMES_STAGE).glob("*.tar"))


def test_without_podman_the_export_says_what_it_left_out(tmp_path, monkeypatch):
    from bipinstall import hermes
    monkeypatch.setattr(hermes, "hermes_checkout", lambda: tmp_path / "no-git")
    monkeypatch.setattr(volumes, "podman", lambda: None)
    home, config = a_home(tmp_path)
    inv = moving.read_inventory(config, home)
    _, tally = moving.export(inv, tmp_path / "e.bipagents", "pass phrase here",
                             SimpleNamespace(system="linux", tailnet_name=None, tailscale=None))
    assert tally.skipped == ["podman volume bipagents-vie-notes"]


def test_the_volumes_tars_follow_the_kept_list(tmp_path, podman):
    folder = tmp_path / "stage"
    made = volumes.export_all(["bipagents-vie-notes"], folder, volumes.podman())
    assert [p.name for p in made] == ["bipagents-vie-notes.tar"]
    with tarfile.open(made[0]) as tar:
        assert "plans/semaine.md" in tar.getnames()
    assert volumes.export_all(["bipagents-gone-x"], folder, volumes.podman()) == []   # removed by hand
    assert not list(folder.glob("*.tar"))


@pytest.mark.skipif(shutil.which("restic") is None, reason="no restic here")
def test_the_hourly_backup_takes_the_volumes_and_a_restore_puts_them_back(tmp_path, podman, monkeypatch):
    home, _ = a_home(tmp_path)
    monkeypatch.setenv("HOME", str(home))
    key = backup.configure(str(tmp_path / "repo"), "seat-token")["password"]
    done = backup.run(home, now=1_790_000_000.0)
    tar_size = (home / backup.STAGE / "volumes" / "bipagents-vie-notes.tar").stat().st_size
    assert done["bytes"] >= tar_size                       # counted in the person's used space
    shutil.rmtree(podman)
    podman.mkdir()
    fetched = backup.fetch(home, key)
    info = backup.put_back(fetched, home, tmp_path / "checkout", None)
    assert info["volumes"] == ["bipagents-vie-notes"] and info["volumes_restored"] == ["bipagents-vie-notes"]
    assert (podman / "bipagents-vie-notes" / "plans" / "semaine.md").read_text() == "Lundi : piscine."
