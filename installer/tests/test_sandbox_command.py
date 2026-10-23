"""`bipagents sandbox adopt <agent>`: the bridge's `sandbox adopt` with this account's config."""
from types import SimpleNamespace

import bipinstall.__main__ as main


def test_adopt_runs_the_bridge_with_the_accounts_config(monkeypatch, tmp_path):
    config = tmp_path / ".config" / "bipagents" / "bridge.toml"
    config.parent.mkdir(parents=True)
    config.write_text("")
    runs = []
    monkeypatch.setattr(main.Path, "home", lambda: tmp_path)
    monkeypatch.setattr(main.rustbridge, "cli", lambda home, *a: ["/bin/bipbridge"])
    fetched = []
    monkeypatch.setattr(main.rustbridge, "ensure", lambda home, *a: fetched.append(home))
    monkeypatch.setattr(main.subprocess, "run", lambda cmd, **kw: runs.append(cmd) or SimpleNamespace(returncode=0))
    assert main.main(["sandbox", "adopt", "wellness", "--dry-run"]) == 0
    assert runs == [["/bin/bipbridge", "sandbox", "adopt", "--agent", "wellness", "--config", str(config), "--dry-run"]]
    assert main.main(["sandbox", "adopt", "vie", "--undo"]) == 0
    assert runs[-1][-1] == "--undo" and "--dry-run" not in runs[-1]
    assert main.main(["sandbox", "adopt", "wellness", "--link", "~/.garmin-mcp", "--link", "/x/data",
                      "--allow-sites"]) == 0
    assert runs[-1][-5:] == ["--link", "~/.garmin-mcp", "--link", "/x/data", "--allow-sites"]
    assert fetched == [tmp_path, tmp_path, tmp_path]   # the bridge of this checkout fetched first (an update before its release)


def test_adopt_says_when_the_bridge_is_not_out_yet(monkeypatch, tmp_path):
    config = tmp_path / ".config" / "bipagents" / "bridge.toml"
    config.parent.mkdir(parents=True)
    config.write_text("")
    monkeypatch.setattr(main.Path, "home", lambda: tmp_path)

    def missing(home, *a):
        raise main.rustbridge.RustBridgeError("no bridge 1234 for this machine yet")
    monkeypatch.setattr(main.rustbridge, "ensure", missing)
    monkeypatch.setattr(main.subprocess, "run", lambda cmd, **kw: (_ for _ in ()).throw(AssertionError("ran")))
    assert main.main(["sandbox", "adopt", "wellness", "--dry-run"]) == 1


def test_adopt_without_an_install_says_so(monkeypatch, tmp_path):
    monkeypatch.setattr(main.Path, "home", lambda: tmp_path)
    assert main.main(["sandbox", "adopt", "wellness"]) == 1


def test_archive_runs_the_bridge_with_the_accounts_config(monkeypatch, tmp_path):
    config = tmp_path / ".config" / "bipagents" / "bridge.toml"
    config.parent.mkdir(parents=True)
    config.write_text("")
    runs = []
    monkeypatch.setattr(main.Path, "home", lambda: tmp_path)
    monkeypatch.setattr(main.rustbridge, "cli", lambda home, *a: ["/bin/bipbridge"])
    monkeypatch.setattr(main.rustbridge, "ensure", lambda home, *a: None)
    monkeypatch.setattr(main.subprocess, "run", lambda cmd, **kw: runs.append(cmd) or SimpleNamespace(returncode=0))
    assert main.main(["archive", "add", "vie", "/home/alex/Public/emails-memoire", "--name", "Mails"]) == 0
    assert runs == [["/bin/bipbridge", "archive", "add", "--agent", "vie", "--config", str(config),
                     "--folder", "/home/alex/Public/emails-memoire", "--name", "Mails"]]
    assert main.main(["archive", "refresh", "vie", "--id", "mails"]) == 0
    assert runs[-1][-2:] == ["--id", "mails"]
    assert main.main(["archive", "add", "vie"]) == 2          # no folder
