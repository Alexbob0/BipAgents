import hashlib
import io
import subprocess
import tarfile

import pytest

from bipinstall import rustbridge


def archive(files):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w:gz") as tar:
        for name, data in files.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    return raw.getvalue()


def test_the_release_key_is_the_workflows_hash_of_the_sources(tmp_path):
    repo = tmp_path / "repo"
    (repo / "bridge-rs" / "src").mkdir(parents=True)
    (repo / "bridge-rs" / "assets").mkdir(parents=True)
    (repo / "bridge-rs" / "src" / "main.rs").write_text("fn main() {}\n")
    (repo / "bridge-rs" / "assets" / "agent_templates.yaml").write_text("[]\n")
    (repo / "bridge").mkdir()
    (repo / "bridge" / "README.md").write_text("docs\n")   # outside bridge-rs: not in the key
    git = lambda *a: subprocess.run(["git", "-C", str(repo), *a], check=True, capture_output=True, text=True).stdout
    git("init", "-q"); git("add", "."); git("-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "x")
    tree = git("rev-parse", "HEAD:bridge-rs").strip()
    # The workflow: printf '%s' "$(git rev-parse HEAD:bridge-rs)" | sha256sum | cut -c1-12
    assert rustbridge.key(repo) == hashlib.sha256(tree.encode()).hexdigest()[:12]
    shell = subprocess.run(["sh", "-c", "printf '%s' \"$(git -C \"$1\" rev-parse HEAD:bridge-rs)\" | shasum -a 256 2>/dev/null "
                            "|| printf '%s' \"$(git -C \"$1\" rev-parse HEAD:bridge-rs)\" | sha256sum", "sh", str(repo)],
                           capture_output=True, text=True).stdout
    assert shell[:12] == rustbridge.key(repo)
    assert rustbridge.key(tmp_path) is None   # not a checkout with bridge-rs


def test_each_machine_gets_its_archive():
    assert rustbridge.asset("Linux", "x86_64") == "bipbridge-linux-x86_64.tar.gz"   # servers, and Windows' WSL2
    assert rustbridge.asset("Linux", "aarch64") == "bipbridge-linux-aarch64.tar.gz"
    assert rustbridge.asset("Darwin", "arm64") == "bipbridge-macos-universal.tar.gz"
    assert rustbridge.asset("Windows", "AMD64") is None


def test_binaries_are_checked_against_the_release_then_kept_two_versions(tmp_path):
    name = "bipbridge-linux-x86_64.tar.gz"
    good = archive({"bipbridge": b"\x7fELF bridge", "bip-browser-mcp": b"\x7fELF mcp", "../evil": b"no"})
    asked = []

    def opener(url):
        asked.append(url)
        if url.endswith("SHA256SUMS"):
            return f"{hashlib.sha256(good).hexdigest()}  {name}\n".encode()
        return good

    path = rustbridge.fetch(tmp_path, "abc123def456", name, opener)
    assert path.read_bytes() == b"\x7fELF bridge" and path.stat().st_mode & 0o111
    assert (path.parent / "bip-browser-mcp").exists() and not (tmp_path / "evil").exists()
    assert asked[0] == "https://github.com/Alexbob0/BipAgents/releases/download/bridge-abc123def456/SHA256SUMS"
    with pytest.raises(rustbridge.RustBridgeError, match="doesn't match"):
        rustbridge.fetch(tmp_path, "000000000002", name, lambda url: b"0" * 64 + f"  {name}".encode()
                         if url.endswith("SUMS") else good)
    with pytest.raises(rustbridge.RustBridgeError, match="isn't out yet"):
        rustbridge.fetch(tmp_path, "000000000003", name, lambda url: (_ for _ in ()).throw(OSError("404")))
    for release in ("000000000004", "000000000005"):
        rustbridge.fetch(tmp_path, release, name, opener)
    kept = sorted(p.name for p in (tmp_path / rustbridge.BINARIES).iterdir())
    assert len(kept) == rustbridge.KEEP and "000000000005" in kept


def test_the_service_runs_this_checkout_s_binary(tmp_path, monkeypatch):
    monkeypatch.setattr(rustbridge, "key", lambda repo=None: "abc123def456")
    repo = tmp_path / "BipAgents"
    binary = rustbridge.binary(tmp_path, "abc123def456")
    assert rustbridge.program(tmp_path, repo) == [str(binary), "serve"]
    assert rustbridge.cli(tmp_path, repo) == [str(binary)]
    unit = tmp_path / ".config" / "systemd" / "user" / "bipagents-bridge.service"
    unit.parent.mkdir(parents=True)
    unit.write_text("[Service]\nWorkingDirectory=/x\nExecStart=/x/.venv/bin/python -m bipbridge serve\nRestart=on-failure\n")
    monkeypatch.setattr(rustbridge.sys, "platform", "linux")
    assert rustbridge.python_service(tmp_path)   # an install made before 2026-10-21
    assert rustbridge.rewrite_service(tmp_path, rustbridge.program(tmp_path, repo)) == unit
    assert not rustbridge.python_service(tmp_path)
    assert f"ExecStart={binary} serve\n" in unit.read_text() and "Restart=on-failure" in unit.read_text()


def test_an_update_before_the_release_is_seen_as_missed(tmp_path, monkeypatch):
    monkeypatch.setattr(rustbridge, "key", lambda repo=None: "abc123def456")
    assert rustbridge.missed(tmp_path, tmp_path)   # `bipagents update` fetches it at its next run, even with no news
    binary = rustbridge.binary(tmp_path, "abc123def456")
    binary.parent.mkdir(parents=True)
    binary.write_text("")
    assert not rustbridge.missed(tmp_path, tmp_path)
    monkeypatch.setattr(rustbridge, "key", lambda repo=None: None)
    assert not rustbridge.missed(tmp_path, tmp_path)


def test_ensure_fetches_or_says_why_not(tmp_path, monkeypatch):
    from bipinstall import hermes
    monkeypatch.setattr(rustbridge, "key", lambda repo=None: "abc123def456")
    monkeypatch.delenv(rustbridge.BUILD, raising=False)
    runner = hermes.Runner(dry=True)
    assert rustbridge.ensure(tmp_path, tmp_path, runner) == rustbridge.binary(tmp_path, "abc123def456")
    assert runner.log[-1].startswith("fetch the bridge abc123def456 from https://github.com/")   # a dry run: said only
    monkeypatch.setattr(rustbridge, "asset", lambda: None)
    with pytest.raises(rustbridge.RustBridgeError, match="ready-made builds"):
        rustbridge.ensure(tmp_path, tmp_path)
    built = tmp_path / "target" / "release"
    built.mkdir(parents=True)
    (built / "bipbridge").write_bytes(b"bridge")
    (built / "bip-browser-mcp").write_bytes(b"mcp")
    monkeypatch.setenv(rustbridge.BUILD, str(built))   # a machine with no build of ours: one made on it
    path = rustbridge.ensure(tmp_path, tmp_path)
    assert path.read_bytes() == b"bridge" and (path.parent / "bip-browser-mcp").read_bytes() == b"mcp"
    monkeypatch.setattr(rustbridge, "key", lambda repo=None: None)
    with pytest.raises(rustbridge.RustBridgeError, match="git checkout"):
        rustbridge.ensure(tmp_path, tmp_path)


def test_an_update_moves_a_python_bridge_to_rust_once_its_binaries_are_out(tmp_path, monkeypatch):
    from bipinstall import __main__ as cli, hermes, migrations
    monkeypatch.setattr(rustbridge, "key", lambda repo=None: "abc123def456")
    monkeypatch.setattr(rustbridge.sys, "platform", "linux")
    monkeypatch.setattr(migrations, "REPO", tmp_path / "BipAgents")
    monkeypatch.setattr(cli, "INSTALLER_PYTHON", tmp_path / "BipAgents" / "installer" / ".venv" / "bin" / "python")
    units = tmp_path / ".config" / "systemd" / "user"
    units.mkdir(parents=True)
    unit = units / "bipagents-bridge.service"
    unit.write_text("[Service]\nExecStart=/r/bridge/.venv/bin/python -m bipbridge serve\n")
    (units / "bipagents-backup.service").write_text("ExecStart=/r/bridge/.venv/bin/python -m bipinstall backup\n")
    (tmp_path / rustbridge.ENGINE).parent.mkdir(parents=True)
    (tmp_path / rustbridge.ENGINE).write_text("python\n")
    old = tmp_path / "BipAgents" / "bridge" / ".venv"
    old.mkdir(parents=True)
    runner = hermes.Runner()
    ran = []
    runner.run = lambda args, **kw: ran.append(list(args)) or ""
    monkeypatch.setattr(cli, "venv", lambda folder, runner, requirements="requirements.txt": ran.append(["venv", str(folder)]))
    restarted = []
    monkeypatch.setattr(rustbridge, "restart", lambda home: restarted.append(home))
    monkeypatch.setattr(rustbridge, "browsers_follow", lambda home: restarted.append("browsers"))

    def not_out(home, repo=None, runner=None):
        raise rustbridge.RustBridgeError("the bridge of this version isn't out yet (HTTPError)")
    monkeypatch.setattr(rustbridge, "ensure", not_out)
    with pytest.raises(migrations.NotYet, match="stays on Python"):
        migrations.rust_bridge(runner, tmp_path)
    assert "python -m bipbridge" in unit.read_text() and old.exists() and not restarted   # nothing changed

    monkeypatch.setattr(rustbridge, "ensure", lambda home, repo=None, runner=None: rustbridge.binary(home, "abc123def456"))
    said = migrations.rust_bridge(runner, tmp_path)
    assert f"ExecStart={rustbridge.binary(tmp_path, 'abc123def456')} serve" in unit.read_text()
    assert restarted == [tmp_path, "browsers"] and "now runs in Rust (abc123def456)" in said
    assert ["venv", str(tmp_path / "BipAgents" / "installer")] in ran
    assert "installer/.venv/bin/python -m bipinstall backup" in (units / "bipagents-backup.service").read_text()
    assert not old.exists() and not (tmp_path / rustbridge.ENGINE).exists()
    assert "already ran in Rust" in migrations.rust_bridge(runner, tmp_path)   # again: harmless
