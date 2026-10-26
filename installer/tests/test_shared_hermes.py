import json
import os
from pathlib import Path
from types import SimpleNamespace

from bipinstall import backup, hermes, shared_hermes


def patches(tmp_path, *texts):
    folder = tmp_path / "patches"
    folder.mkdir(exist_ok=True)
    for i, text in enumerate(texts):
        (folder / f"000{i + 1}-x.patch").write_text(text)
    return folder


def fake_build(target: Path) -> Path:
    """What Hermes' installer leaves in a HERMES_HOME used as the build's: checkout, store, a committed venv."""
    checkout = target / "hermes-agent"
    (checkout / ".git").mkdir(parents=True)
    (checkout / "hermes_cli").mkdir()
    (checkout / "hermes_cli" / "main.py").write_text("x = 1\n")
    (checkout / "install-stamp.json").write_text(json.dumps({"commit": "517b5e10f", "updateMechanism": "self"}))
    python = target / "tools" / "python-3.14.7-linux-x64" / "bin" / "python3"
    python.parent.mkdir(parents=True)
    python.symlink_to("/usr/bin/true")
    generation = target / "installs" / "abc" / "environments" / "gen1"
    (generation / "venv").mkdir(parents=True)
    (generation / ".lease-managed").write_text("")
    (generation / ".leases").mkdir()
    (target / "installs" / "abc" / "facts.json").write_text(json.dumps(
        {"packages": {"venv": {"environment": str(generation / "venv")}}}))
    (target / ".build-home" / ".npm").mkdir(parents=True)
    return generation


def test_a_version_is_hermes_commit_and_patches(tmp_path):
    one = shared_hermes.version_id("517b5e10f", patches(tmp_path, "a"))
    assert one.startswith("517b5e10f-") and one == shared_hermes.version_id("517b5e10f", tmp_path / "patches")
    (tmp_path / "patches" / "0001-x.patch").write_text("b")
    assert shared_hermes.version_id("517b5e10f", tmp_path / "patches") != one
    assert shared_hermes.version_id("abc", tmp_path / "patches").startswith("abc-")


def test_off_unless_root_turned_it_on(tmp_path, monkeypatch):
    setting = tmp_path / "shared-hermes.json"
    monkeypatch.setattr(shared_hermes, "SETTING", setting)
    assert shared_hermes.root() is None
    setting.write_text(json.dumps({"enabled": True, "root": str(tmp_path / "opt")}))
    assert shared_hermes.root() == tmp_path / "opt"
    setting.write_text(json.dumps({"enabled": False}))
    assert shared_hermes.root() is None


def test_the_build_runs_hermes_installer_unprivileged_then_seals_it_for_root(tmp_path):
    runner = hermes.Runner(dry=True)
    target = shared_hermes.build(runner, tmp_path / "opt", "v1", patches=patches(tmp_path, "a", "b"), commit="517b5e10f")
    log = runner.log
    at = lambda text: next(i for i, line in enumerate(log) if text in line)
    assert target == tmp_path / "opt" / "v1"
    assert "useradd --system" in log[0]
    install = log[at("install.sh --non-interactive")]
    assert install.startswith(f"cd {target} && sudo -u bipagents-hermes env -i") and "--commit 517b5e10f" in install
    assert f"HERMES_HOME={target}" in install and "--skip-browser" not in install
    assert at("install.sh --non") < at("0001-x.patch") < at("0002-x.patch") < at("stage_manager_runtime") \
        < at(f"seal {target}") < at("chown -R root:root") < at("chmod -R a+rX,go-w") < at(".complete")
    assert all("sudo -u bipagents-hermes" in line for line in log if "git -C" in line)
    assert not shared_hermes.ready(tmp_path / "opt", "v1")


def test_a_finished_version_is_not_built_again(tmp_path):
    (tmp_path / "opt" / "v1").mkdir(parents=True)
    (tmp_path / "opt" / "v1" / shared_hermes.COMPLETE).write_text("{}")
    runner = hermes.Runner(dry=True)
    shared_hermes.build(runner, tmp_path / "opt", "v1", patches=patches(tmp_path, "a"))
    assert runner.log == []


def test_sealing_makes_it_a_payload_hermes_leaves_alone(tmp_path):
    target = tmp_path / "v1"
    generation = fake_build(target)
    shared_hermes.seal(target, version="v1", commit="517b5e10f")
    manifest = json.loads((target / "manifest.json").read_text())
    assert manifest == {"repo": "hermes-agent", "venv": "installs/abc/environments/gen1/venv", "store": "tools",
                        "runtime": {"commands": {"hermes": "hermes-agent/.hermes/bin/hermes"}}}
    assert not (target / "hermes-agent" / ".git").exists()
    stamp = json.loads((target / "hermes-agent" / "install-stamp.json").read_text())
    assert stamp["updateMechanism"] == "external" and stamp["distribution"] == "bipagents" and stamp["commit"] == "517b5e10f"
    assert not (generation / ".lease-managed").exists() and not (generation / ".leases").exists()
    assert os.readlink(target / "bin" / "python3") == "../tools/python-3.14.7-linux-x64/bin/python3"
    assert not (target / ".build-home" / ".npm").exists()


def test_a_place_points_at_a_version_and_moves_in_one_step(tmp_path):
    home, base = tmp_path / "home", tmp_path / "opt"
    hermes_home = home / ".hermes"
    shared_hermes.point(hermes.Runner(), base, "v1", home, hermes_home)
    assert os.readlink(hermes_home / "shared") == str(base / "v1")
    assert os.readlink(hermes_home / "hermes-agent") == "shared/hermes-agent"
    launcher = (home / ".local" / "bin" / "hermes").read_text()
    assert f'exec "{hermes_home}/shared/hermes-agent/.hermes/bin/hermes" "$@"' in launcher
    assert os.access(home / ".local" / "bin" / "hermes-acp", os.X_OK)
    shared_hermes.point(hermes.Runner(), base, "v2", home, hermes_home)   # the way back is the same move
    assert shared_hermes.pointed(hermes_home) == "v2"
    assert not list(hermes_home.glob(".*.new"))


def test_a_place_with_its_own_copy_keeps_it_until_it_runs_the_shared_one(tmp_path):
    home = tmp_path / "home"
    hermes_home = home / ".hermes"
    for name in ("hermes-agent", "tools", "installs", "skills", "memories"):
        (hermes_home / name).mkdir(parents=True)
    space = home / ".local" / "share" / "bipagents" / "spaces" / "vie"
    (space / "installs").mkdir(parents=True)
    (home / ".npm" / "_cacache").mkdir(parents=True)
    (home / ".npm" / "_npx").mkdir()   # the MCP servers' packages stay
    shared_hermes.point(hermes.Runner(), tmp_path / "opt", "v1", home, hermes_home)
    assert (hermes_home / "hermes-agent").is_dir() and not (hermes_home / "hermes-agent").is_symlink()
    own = shared_hermes.own_copies(home, hermes_home, shared_hermes.spaces_of(home))
    assert sorted(str(p.relative_to(home)) for p in own) == [
        ".hermes/hermes-agent", ".hermes/installs", ".hermes/tools", ".local/share/bipagents/spaces/vie/installs",
        ".npm/_cacache"]
    runner = hermes.Runner()
    shared_hermes.remove_own_copy(runner, own)
    assert (hermes_home / "skills").is_dir() and (hermes_home / "memories").is_dir()
    assert not (hermes_home / "tools").exists() and not (space / "installs").exists()
    assert (home / ".npm" / "_npx").is_dir() and not (home / ".npm" / "_cacache").exists()


def test_the_bridge_finds_the_shared_python(tmp_path):
    config = tmp_path / "bridge.toml"
    config.write_text('port = 1\n\n[hermes]\nbin = "/h/.local/bin/hermes"\nhome = "/h/.hermes"\n\n[lan]\nenabled = false\n')
    assert shared_hermes.bridge_config(config, Path("/h/.hermes"))
    text = config.read_text()
    assert ('[hermes]\nbin = "/h/.local/bin/hermes"\nhome = "/h/.hermes"\npython = ["/h/.hermes/shared/bin/python3", '
            '"-I"]\nroot = "/h/.hermes/shared/hermes-agent"\n\n[lan]\nenabled = false\n') in text
    assert not shared_hermes.bridge_config(config, Path("/h/.hermes"))


def test_gc_keeps_what_a_place_points_at_or_a_process_runs(tmp_path):
    base = tmp_path / "opt"
    for name in ("old", "pointed", "running", "pinned", "half"):
        (base / name).mkdir(parents=True)
    home = tmp_path / "home" / "bip"
    (home / ".hermes").mkdir(parents=True)
    (home / ".hermes" / "shared").symlink_to(base / "pointed")
    proc = tmp_path / "proc"
    (proc / "42").mkdir(parents=True)
    (proc / "42" / "maps").write_text(f"7f00-7f01 r-xp 0 08:01 1 {base}/running/tools/python/lib/libpython3.so\n")
    removed = shared_hermes.gc(hermes.Runner(), base, [home], keep=["pinned"], proc=proc)
    assert removed == ["half", "old"]
    assert sorted(p.name for p in base.iterdir()) == ["pinned", "pointed", "running"]


def test_install_points_at_the_shared_hermes_when_root_built_it(tmp_path, monkeypatch):
    base = tmp_path / "opt"
    version = shared_hermes.version_id()
    (base / version).mkdir(parents=True)
    (base / version / shared_hermes.COMPLETE).write_text("{}")
    setting = tmp_path / "setting.json"
    setting.write_text(json.dumps({"root": str(base)}))
    monkeypatch.setattr(shared_hermes, "SETTING", setting)
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    monkeypatch.setenv("HERMES_HOME", str(tmp_path / "home" / ".hermes"))
    monkeypatch.setattr(hermes, "hermes_bin", lambda: str(tmp_path / "home" / ".local" / "bin" / "hermes"))
    runner = hermes.Runner(dry=True)
    hermes.install(runner, browser=True)
    assert runner.log == [f"link {tmp_path / 'home' / '.hermes' / 'shared'} -> {base / version}"]
    (base / version / shared_hermes.COMPLETE).unlink()   # not built yet: its own copy, as before
    runner = hermes.Runner(dry=True)
    hermes.install(runner, browser=True)
    assert "install.sh" in runner.log[0]


def test_what_agents_install_for_themselves_counts_in_the_5_gb(tmp_path):
    home = tmp_path
    (home / ".npm" / "_npx").mkdir(parents=True)
    (home / ".npm" / "_npx" / "pkg.js").write_bytes(b"x" * 1000)
    (home / ".cache" / "uv").mkdir(parents=True)
    (home / ".cache" / "uv" / "wheel").write_bytes(b"x" * 500)
    hermes_home = home / ".hermes"
    (hermes_home / "tools").mkdir(parents=True)
    (hermes_home / "tools" / "agent-browser").write_bytes(b"x" * 200)
    inv = SimpleNamespace(hermes_home=hermes_home, hermes_homes=lambda: [hermes_home])
    assert backup.personal_runtime_bytes(inv, home) == 1500   # its own Hermes: ~/.hermes/tools is that Hermes
    (hermes_home / "shared").symlink_to(tmp_path / "opt" / "v1")
    assert backup.personal_runtime_bytes(inv, home) == 1700
