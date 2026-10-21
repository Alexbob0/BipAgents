"""`bipinstall console`: its own bridge's link, or, from an account without an install, the chosen account's."""
from types import SimpleNamespace

from bipinstall import console


def test_an_account_without_install_asks_which_one(monkeypatch):
    runs = []
    monkeypatch.setattr(console, "own_config", lambda given: None)
    monkeypatch.setattr(console, "installed_accounts", lambda: ["alex", "chi"])
    monkeypatch.setattr(console.subprocess, "run", lambda cmd, **kw: runs.append(cmd) or SimpleNamespace(returncode=0))
    monkeypatch.setattr("builtins.input", lambda prompt: "2")
    assert console.main([]) == 0
    assert runs[0] == ["sudo", "-v"]
    assert runs[1][:2] == ["bash", "-c"] and "sudo -u chi" in runs[1][2]
    assert ('"$(ls -td $HOME/.local/share/bipagents/bridge/*/ | head -1)bipbridge" console --config '
            "/home/chi/.config/bipagents/bridge.toml") in runs[1][2]


def test_an_install_makes_its_own_link(monkeypatch, tmp_path):
    runs = []
    config = tmp_path / "bridge.toml"
    monkeypatch.setattr(console, "own_config", lambda given: config)
    monkeypatch.setattr(console, "REPO", tmp_path)
    monkeypatch.setenv("HOME", str(tmp_path))
    from bipinstall import rustbridge
    monkeypatch.setattr(rustbridge, "key", lambda repo=None: "abc123def456")
    binary = rustbridge.binary(tmp_path, "abc123def456")
    binary.parent.mkdir(parents=True)
    binary.touch()
    monkeypatch.setattr(console.subprocess, "run", lambda cmd, **kw: runs.append(cmd) or SimpleNamespace(returncode=0))
    assert console.main([]) == 0
    assert runs == [[str(binary), "console", "--config", str(config)]]
