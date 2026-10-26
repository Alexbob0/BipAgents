"""Less disk per install (2026-10-26): no download cache left, a light checkout, a place's voices without PyTorch."""
import re
import subprocess

import pytest

from bipinstall import checkout, hermes, migrations, voices
from bipinstall.__main__ import venv

TORCH_UNIT = """[Unit]
Description=BipAgents voices (Pocket TTS)

[Service]
WorkingDirectory={repo}/pocket
ExecStart={repo}/pocket/.venv/bin/python {repo}/pocket/pocket_server.py --voices {repo}/voices --languages fr,en --port 8098
Restart=on-failure

[Install]
WantedBy=default.target
"""


def git(repo, *args):
    return subprocess.run(["git", "-C", str(repo), "-c", "user.name=t", "-c", "user.email=t@t", *args], check=True,
                          capture_output=True, text=True).stdout


def recording_runner():
    runner = hermes.Runner(dry=False)
    ran = []
    runner.run = lambda args, **kw: ran.append(" ".join(args)) or ""
    return runner, ran


def test_environments_are_made_without_uvs_cache():
    runner = hermes.Runner(dry=True)
    venv(checkout.Path("/r/pocket"), runner, "requirements-onnx.txt")
    assert all("--no-cache" in line for line in runner.log) and len(runner.log) == 2


def test_install_sh_clones_light_and_runs_the_installers_own_environment():
    text = (checkout.Path(__file__).resolve().parents[2] / "install.sh").read_text()
    assert "export UV_NO_CACHE=1 PIP_NO_CACHE_DIR=1" in text
    assert "git clone --depth 1 --filter=blob:none --no-checkout" in text
    listed = re.findall(r"'(/\*|!/[^']+)'", text.split("sparse-checkout set --no-cone", 1)[1].split("checkout --quiet")[0])
    assert listed == checkout.patterns(voice_states=False)          # the same list as the installer's
    assert "uv run" not in text and '"$VENV/bin/python" -m bipinstall' in text
    assert "resync()" in text and "fetch --depth 1 --quiet origin main" in text   # a rewritten history


@pytest.fixture
def public_clone(tmp_path, monkeypatch):
    """A full clone of a small « public » repository, with an App/, recordings and voice states."""
    origin = tmp_path / "origin"
    (origin / "App").mkdir(parents=True)
    (origin / "voices" / "french").mkdir(parents=True)
    (origin / "installer").mkdir()
    (origin / "App" / "Big.swift").write_text("x" * 5000)
    (origin / "voices" / "french" / "galet.safetensors").write_bytes(b"s" * 5000)
    (origin / "voices" / "french" / "galet_sample.wav").write_bytes(b"w" * 5000)
    (origin / "voices" / "french" / "voices.json").write_text("{}")
    (origin / "installer" / "a.py").write_text("print(1)\n")
    git(origin, "init", "-q", "-b", "main")
    git(origin, "config", "uploadpack.allowFilter", "true")
    git(origin, "add", ".")
    git(origin, "commit", "-qm", "one")
    (origin / "installer" / "a.py").write_text("print(2)\n")
    git(origin, "commit", "-qam", "two")
    clone = tmp_path / "BipAgents"
    subprocess.run(["git", "clone", "-q", f"file://{origin}", str(clone)], check=True)
    monkeypatch.setattr(checkout, "PUBLIC", re.compile(re.escape(str(origin))))
    return origin, clone


def test_a_full_clone_becomes_light_and_still_updates(public_clone):
    origin, clone = public_clone
    said = checkout.lighten(clone, hermes.Runner(dry=False), voice_states=False)
    assert said and "light copy" in said
    assert (clone / "installer" / "a.py").read_text() == "print(2)\n"
    assert not (clone / "App").exists() and not (clone / "voices/french/galet_sample.wav").exists()
    assert not (clone / "voices/french/galet.safetensors").exists() and (clone / "voices/french/voices.json").exists()
    assert git(clone, "rev-parse", "--is-shallow-repository").strip() == "true"
    assert git(clone, "rev-list", "--count", "HEAD").strip() == "1"
    (origin / "installer" / "a.py").write_text("print(3)\n")
    git(origin, "commit", "-qam", "three")
    git(clone, "pull", "--ff-only", "-q")                               # install.sh's update
    assert (clone / "installer" / "a.py").read_text() == "print(3)\n"
    assert checkout.narrow(clone, hermes.Runner(dry=False), voice_states=True)   # Pocket's own models: back
    assert (clone / "voices/french/galet.safetensors").read_bytes() == b"s" * 5000


def test_a_developers_clone_is_never_touched(public_clone, monkeypatch):
    _, clone = public_clone
    monkeypatch.setattr(checkout, "PUBLIC", re.compile("github.com[/:]Alexbob0/BipAgents"))
    assert checkout.lighten(clone, hermes.Runner(dry=False), voice_states=False) is None
    assert not checkout.narrow(clone, hermes.Runner(dry=False), voice_states=False)
    assert (clone / "App" / "Big.swift").exists()


def place(tmp_path, monkeypatch, answers):
    repo = tmp_path / "BipAgents"
    (repo / "pocket" / ".venv" / "bin").mkdir(parents=True)
    (repo / "pocket" / ".venv" / "torch").write_text("5.5 GB")
    units = tmp_path / ".config/systemd/user"
    units.mkdir(parents=True)
    unit = units / "bipagents-pocket.service"
    unit.write_text(TORCH_UNIT.format(repo=repo))
    (tmp_path / ".config/bipagents").mkdir()
    (tmp_path / ".config/bipagents/setup.json").write_text("{}")   # a place of the offer
    weights = tmp_path / ".cache/huggingface/hub/models--kyutai--pocket-tts-without-voice-cloning"
    weights.mkdir(parents=True)
    monkeypatch.setattr(migrations, "REPO", repo)
    monkeypatch.setattr(voices, "source", lambda home: ("https://guichet.test/v1/voices/", "token"))
    asked = []

    def install(languages, home, runner, say, **kw):
        asked.append(languages)
        return tmp_path / "packages"

    monkeypatch.setattr(voices, "install", install)
    monkeypatch.setattr(voices, "engine_at", lambda url, wait=0: answers.pop(0))

    def run(args, **kw):
        ran.append(" ".join(args))
        if args[:2] == ["uv", "venv"]:
            (repo / "pocket" / ".venv-light" / "bin").mkdir(parents=True)
        return ""

    runner, ran = hermes.Runner(dry=False), []
    runner.run = run
    return repo, unit, weights, runner, ran, asked


def test_a_places_voices_move_off_pytorch_then_its_environment_goes(tmp_path, monkeypatch):
    repo, unit, weights, runner, ran, asked = place(tmp_path, monkeypatch, ["4.0.1"])
    said = migrations.light_voices(runner, tmp_path, wait=0)
    assert "without PyTorch" in said and asked == [["fr", "en"]]
    text = unit.read_text()
    assert f"--onnx {tmp_path}/packages --keep none --idle-unload 120 --threads 2 --port 8098" in text
    assert not (repo / "pocket" / ".venv" / "torch").exists() and (repo / "pocket" / ".venv" / "bin").is_dir()
    assert not (repo / "pocket" / ".venv-torch").exists() and not weights.exists()
    assert any("requirements-onnx.txt" in line and "--no-cache" in line for line in ran)
    assert ran[-1] == "systemctl --user restart bipagents-pocket"


def test_light_voices_that_dont_answer_put_pytorchs_back(tmp_path, monkeypatch):
    repo, unit, weights, runner, ran, _ = place(tmp_path, monkeypatch, [None])
    with pytest.raises(migrations.NotYet):
        migrations.light_voices(runner, tmp_path, wait=0)
    assert unit.read_text() == TORCH_UNIT.format(repo=repo)
    assert (repo / "pocket" / ".venv" / "torch").exists() and not (repo / "pocket" / ".venv-light").exists()
    assert weights.exists() and ran[-1] == "systemctl --user restart bipagents-pocket"


def test_without_a_guichet_pockets_own_models_stay(tmp_path, monkeypatch):
    repo, unit, _, runner, ran, _ = place(tmp_path, monkeypatch, [])
    monkeypatch.setattr(voices, "source", lambda home: (None, None))
    assert "keep Pocket's own models" in migrations.light_voices(runner, tmp_path)
    assert ran == [] and (repo / "pocket" / ".venv" / "torch").exists()


def test_the_update_empties_a_places_caches(tmp_path, monkeypatch):
    repo, unit, weights, runner, ran, _ = place(tmp_path, monkeypatch, ["4.0.1"])
    monkeypatch.setattr(migrations.Path, "home", lambda: tmp_path)
    monkeypatch.setattr(hermes, "sandbox_runtime", lambda: None)
    import bipinstall.__main__ as main
    monkeypatch.setattr(main.shutil, "which", lambda name: "/bin/sh")
    said = migrations.light_disk(runner, tmp_path)
    assert "download caches emptied" in said
    assert any(line.endswith("cache clean") for line in ran)
