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
    (repo / "bridge" / "bipbridge").mkdir(parents=True)
    (repo / "bridge-rs" / "src" / "main.rs").write_text("fn main() {}\n")
    (repo / "bridge" / "bipbridge" / "app.py").write_text("app = None\n")
    git = lambda *a: subprocess.run(["git", "-C", str(repo), *a], check=True, capture_output=True, text=True).stdout
    git("init", "-q"); git("add", "."); git("-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "x")
    trees = " ".join(git("rev-parse", f"HEAD:{p}").strip() for p in ("bridge-rs", "bridge/bipbridge"))
    # The workflow: printf '%s %s' <bridge-rs tree> <bridge/bipbridge tree> | sha256sum | cut -c1-12
    assert rustbridge.key(repo) == hashlib.sha256(trees.encode()).hexdigest()[:12]
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


def test_the_service_runs_the_chosen_engine(tmp_path, monkeypatch):
    monkeypatch.setattr(rustbridge, "key", lambda repo=None: "abc123def456")
    repo = tmp_path / "BipAgents"
    assert rustbridge.engine(tmp_path) == "python"
    assert rustbridge.program(tmp_path, repo)[-3:] == ["-m", "bipbridge", "serve"]
    rustbridge.set_engine(tmp_path, "rust")
    assert rustbridge.program(tmp_path, repo)[-1] == "serve" and "python" in rustbridge.program(tmp_path, repo)[0]   # not downloaded yet
    binary = rustbridge.binary(tmp_path, "abc123def456")
    binary.parent.mkdir(parents=True)
    binary.write_bytes(b"x")
    assert rustbridge.program(tmp_path, repo) == [str(binary), "serve"]
    assert rustbridge.cli(tmp_path, repo) == [str(binary)]
    unit = tmp_path / ".config" / "systemd" / "user" / "bipagents-bridge.service"
    unit.parent.mkdir(parents=True)
    unit.write_text("[Service]\nWorkingDirectory=/x\nExecStart=/x/.venv/bin/python -m bipbridge serve\nRestart=on-failure\n")
    monkeypatch.setattr(rustbridge.sys, "platform", "linux")
    assert rustbridge.rewrite_service(tmp_path, rustbridge.program(tmp_path, repo)) == unit
    assert f"ExecStart={binary} serve\n" in unit.read_text() and "Restart=on-failure" in unit.read_text()
