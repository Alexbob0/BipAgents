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
    monkeypatch.setattr(main.subprocess, "run", lambda cmd, **kw: runs.append(cmd) or SimpleNamespace(returncode=0))
    assert main.main(["sandbox", "adopt", "wellness", "--dry-run"]) == 0
    assert runs == [["/bin/bipbridge", "sandbox", "adopt", "--agent", "wellness", "--config", str(config), "--dry-run"]]
    assert main.main(["sandbox", "adopt", "vie", "--undo"]) == 0
    assert runs[-1][-1] == "--undo" and "--dry-run" not in runs[-1]


def test_adopt_without_an_install_says_so(monkeypatch, tmp_path):
    monkeypatch.setattr(main.Path, "home", lambda: tmp_path)
    assert main.main(["sandbox", "adopt", "wellness"]) == 1
