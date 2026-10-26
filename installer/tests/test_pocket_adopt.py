"""A Pocket service set up by hand moved to the installer's, and Pocket never restarted on voice states made for
another Pocket (2026-10-25, aibox: a 3.3.0 Pocket restarted on the 4.0.1 states)."""
import json
import subprocess

from bipinstall import hermes, pocketunit, upgrade, voices

HAND_MADE = """[Unit]
Description=Pocket TTS (by hand)

[Service]
WorkingDirectory=%h/pocket-tts
ExecStart=%h/pocket-tts/.venv/bin/python pocket_server.py --voices %h/BipAgents/voices --languages fr,en --port 8099 --threads 2 --quantize
Restart=on-failure

[Install]
WantedBy=default.target
"""


def hand_made(home):
    units = home / ".config/systemd/user"
    units.mkdir(parents=True, exist_ok=True)
    path = units / "pocket-tts.service"
    path.write_text(HAND_MADE)
    return path


def recording_runner():
    runner = hermes.Runner(dry=False)
    ran = []
    runner.run = lambda args, **kw: ran.append(" ".join(args)) or ""
    return runner, ran


def write_voices(folder, engine, content=b"state"):
    for language in ("french", "english"):
        (folder / language).mkdir(parents=True, exist_ok=True)
        (folder / language / "voices.json").write_text(json.dumps({"engine": f"pocket-tts {engine}"}))
        (folder / language / "galet.safetensors").write_bytes(content + language.encode())


def git(repo, *args):
    subprocess.run(["git", "-C", str(repo), "-c", "user.name=t", "-c", "user.email=t@t", *args], check=True,
                   capture_output=True)


def test_a_hand_made_service_is_read_from_its_unit(tmp_path):
    hand_made(tmp_path)
    unit = pocketunit.find(tmp_path)
    assert unit.name == "pocket-tts" and unit.python == tmp_path / "pocket-tts/.venv/bin/python"
    assert unit.script == tmp_path / "pocket-tts/pocket_server.py"
    assert (unit.port, unit.languages, unit.voices) == (8099, "fr,en", tmp_path / "BipAgents/voices")
    assert unit.extra_flags() == ["--threads", "2", "--quantize"] and not unit.light
    assert not unit.standard(tmp_path / "BipAgents") and unit.url == "http://127.0.0.1:8099"


def test_adopting_it_writes_bipagents_pocket_with_the_same_port_languages_and_voices(tmp_path, monkeypatch):
    old = hand_made(tmp_path)
    repo = tmp_path / "BipAgents"
    (repo / "pocket").mkdir(parents=True)
    (repo / "pocket" / "requirements.txt").write_text("pocket-tts==4.0.1\n")
    asked = []
    monkeypatch.setattr(voices, "engine_at", lambda url, wait=0: asked.append(url) or "4.0.1")
    made, said = [], []
    venv = lambda folder, runner, requirements="requirements.txt": made.append((folder, requirements))  # noqa: E731
    runner, ran = recording_runner()
    unit = pocketunit.find(tmp_path)
    assert pocketunit.adopt(unit, repo, tmp_path, runner, venv, said.append, wait=0)
    assert made == [(repo / "pocket", "requirements.txt")]           # PyTorch's Pocket, never the light one
    new = (old.parent / "bipagents-pocket.service").read_text()
    assert (f"ExecStart={repo}/pocket/.venv/bin/python {repo}/pocket/pocket_server.py --voices {repo}/voices "
            "--languages fr,en --port 8099 --threads 2 --quantize") in new
    assert not old.exists() and (old.parent / "pocket-tts.service.disabled").read_text() == HAND_MADE
    assert ran == ["systemctl --user stop pocket-tts", "systemctl --user disable pocket-tts",
                   "systemctl --user daemon-reload", "systemctl --user enable --now bipagents-pocket"]
    assert asked == ["http://127.0.0.1:8099"] and any("5 GB" in line for line in said)
    found = pocketunit.find(tmp_path)
    assert found.name == "bipagents-pocket" and found.standard(repo)    # the next update: nothing to adopt


def test_an_adoption_that_doesnt_answer_puts_the_old_service_back(tmp_path, monkeypatch):
    old = hand_made(tmp_path)
    repo = tmp_path / "BipAgents"
    (repo / "pocket").mkdir(parents=True)
    monkeypatch.setattr(voices, "engine_at", lambda url, wait=0: "3.3.0")
    runner, ran = recording_runner()
    venv = lambda folder, runner, requirements="requirements.txt": None  # noqa: E731
    assert not pocketunit.adopt(pocketunit.find(tmp_path), repo, tmp_path, runner, venv, lambda line: None, wait=0)
    assert old.read_text() == HAND_MADE and not (old.parent / "bipagents-pocket.service").exists()
    assert not (old.parent / "pocket-tts.service.disabled").exists()
    assert ran[-4:] == ["systemctl --user disable --now bipagents-pocket", "systemctl --user daemon-reload",
                        "systemctl --user enable pocket-tts", "systemctl --user restart pocket-tts"]

    def failing(folder, runner, requirements="requirements.txt"):
        raise RuntimeError("uv pip install failed (1): no space left")

    ran.clear()
    assert not pocketunit.adopt(pocketunit.find(tmp_path), repo, tmp_path, runner, failing, lambda line: None)
    assert ran == [] and old.read_text() == HAND_MADE                  # a failed install: nothing touched


def test_a_pocket_that_cant_move_keeps_its_own_states_from_git(tmp_path):
    repo = tmp_path / "BipAgents"
    write_voices(repo / "voices", "3.3.0", b"old-")
    git(tmp_path, "init", "-q", str(repo))
    git(repo, "add", "-A")
    git(repo, "commit", "-qm", "3.3.0 states")
    write_voices(repo / "voices", "4.0.1", b"new-")
    git(repo, "commit", "-qam", "4.0.1 states")
    hand_made(tmp_path)
    unit = pocketunit.find(tmp_path)
    runner, ran = recording_runner()
    said = []
    # Pocket 3.3.0 on the 4.0.1 states: its service now names its own states, kept from git; Pocket may restart.
    assert pocketunit.serve_matching(unit, repo, tmp_path, runner, said.append, "3.3.0") is True
    kept = tmp_path / ".local/share/bipagents/voices/pocket-3.3.0"
    assert (kept / "french" / "galet.safetensors").read_bytes() == b"old-french"
    assert (kept / "english" / "galet.safetensors").read_bytes() == b"old-english"
    unit = pocketunit.find(tmp_path)
    assert unit.voices == kept and ran == ["systemctl --user daemon-reload"] and "3.3.0" in said[-1]
    assert pocketunit.serve_matching(unit, repo, tmp_path, runner, said.append, "3.3.0") is False   # already so
    # Once it runs 4.0.1: back on the repository's voices.
    assert pocketunit.serve_matching(unit, repo, tmp_path, runner, said.append, "4.0.1") is True
    assert pocketunit.find(tmp_path).voices == repo / "voices"


def test_without_its_states_in_git_pocket_isnt_restarted_on_others(tmp_path):
    repo = tmp_path / "BipAgents"
    write_voices(repo / "voices", "4.0.1")
    git(tmp_path, "init", "-q", str(repo))
    git(repo, "add", "-A")
    git(repo, "commit", "-qm", "4.0.1 only (a shallow clone)")
    hand_made(tmp_path)
    runner, ran = recording_runner()
    said = []
    unit = pocketunit.find(tmp_path)
    assert pocketunit.serve_matching(unit, repo, tmp_path, runner, said.append, "3.3.0") is None
    assert ran == [] and "not restarted" in said[-1] and pocketunit.find(tmp_path).voices == repo / "voices"
    # Voices of one's own (another folder): left alone.
    other = hermes.Runner(dry=False)
    unit.program[unit.program.index("--voices") + 1] = str(tmp_path / "mine")
    assert pocketunit.serve_matching(unit, repo, tmp_path, other, said.append, "3.3.0") is False


def test_the_update_adopts_then_never_restarts_on_mismatched_states(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    hand_made(tmp_path)
    repo = tmp_path / "BipAgents"
    write_voices(repo / "voices", "4.0.1")
    monkeypatch.setattr(upgrade, "REPO", repo)
    runner, ran = recording_runner()
    venv = lambda folder, runner, requirements="requirements.txt": None  # noqa: E731
    touched = lambda *prefixes: True  # noqa: E731
    adopted = []
    monkeypatch.setattr(pocketunit, "adopt", lambda unit, *a, **k: adopted.append(unit.name) or True)
    assert not upgrade.pocket_step(runner, touched, venv) and adopted == ["pocket-tts"]   # started there: no restart
    # The adoption failed (the old one back), its Pocket 3.3.0 and no 3.3.0 states to be had: no restart.
    monkeypatch.setattr(pocketunit, "adopt", lambda unit, *a, **k: False)
    monkeypatch.setattr(pocketunit, "engine_of", lambda unit: "3.3.0")
    monkeypatch.setattr(pocketunit, "old_states", lambda *a: None)
    assert not upgrade.pocket_step(runner, touched, venv)
    # A light (--onnx) service set up by hand is never adopted (PyTorch never comes for it).
    (tmp_path / ".config/systemd/user/pocket-tts.service").write_text(
        "ExecStart=/x/python /x/pocket_server.py --onnx /x/packages --port 8098\n")
    monkeypatch.setattr(pocketunit, "adopt", lambda *a, **k: (_ for _ in ()).throw(AssertionError("adopted")))
    monkeypatch.setattr(voices, "refresh", lambda home, runner, say: False)
    assert upgrade.pocket_step(runner, touched, venv)


def test_a_pocket_with_every_language_gets_dutch_once_its_states_are_there(tmp_path):
    # 2026-10-26: units written with « --languages fr,en,es,de » get nl when voices/dutch/ fits their Pocket.
    repo = tmp_path / "BipAgents"
    units = tmp_path / ".config/systemd/user"
    units.mkdir(parents=True)
    unit_path = units / "bipagents-pocket.service"
    python, script = repo / "pocket/.venv/bin/python", repo / "pocket/pocket_server.py"
    unit_path.write_text(f"ExecStart={python} {script} --voices {repo}/voices --languages de,fr,en,es --port 8098\n")
    write_voices(repo / "voices", "4.0.1")
    runner, ran = recording_runner()
    said = []
    unit = pocketunit.find(tmp_path)
    assert not pocketunit.serve_new_languages(unit, runner, said.append, "4.0.1")      # no Dutch states yet
    (repo / "voices/dutch").mkdir()
    (repo / "voices/dutch/voices.json").write_text(json.dumps({"engine": "pocket-tts 4.0.1"}))
    assert not pocketunit.serve_new_languages(unit, runner, said.append, "3.3.0")      # made for another Pocket
    assert not pocketunit.serve_new_languages(unit, runner, said.append, None)         # its Pocket unknown
    assert pocketunit.serve_new_languages(unit, runner, said.append, "4.0.1")
    unit = pocketunit.find(tmp_path)
    assert unit.languages == "de,fr,en,es,nl" and unit.port == 8098 and unit.voices == repo / "voices"
    assert ran == ["systemctl --user daemon-reload"] and "Dutch" in said[-1]
    assert not pocketunit.serve_new_languages(unit, runner, said.append, "4.0.1")      # already so
    # A Pocket that chose fewer languages (« mini »: the person's own) keeps its choice.
    unit_path.write_text(f"ExecStart={python} {script} --voices {repo}/voices --languages fr --port 8098\n")
    assert not pocketunit.serve_new_languages(pocketunit.find(tmp_path), runner, said.append, "4.0.1")
    assert voices.with_new_languages(["fr", "en", "es", "de"]) == ["fr", "en", "es", "de", "nl"]
    assert voices.with_new_languages(["fr"]) == ["fr"]


def test_the_migration_names_the_account_and_the_command(monkeypatch):
    from bipinstall import migrations
    monkeypatch.setenv("USER", "aibox")
    said = migrations.pocket_not_yet("http://127.0.0.1:8098", "3.3.0", "4.0.1", owner="aibox")
    assert "still Pocket 3.3.0" in said and "`bipagents update` from the account « aibox » (this one)" in said
    assert "Pocket 4.0.1" in said and "bipagents-pocket" in said
    assert "« alex »" in migrations.pocket_not_yet("u", None, "4.0.1", owner="alex")
