import subprocess
from types import SimpleNamespace

from bipinstall import hermes, update


def repo_with_patches(tmp_path):
    """A Hermes checkout with two files to change, and two BipAgents patches; the first one already on."""
    checkout = tmp_path / "hermes-agent"
    checkout.mkdir()
    git = lambda *a: subprocess.run(["git", "-C", str(checkout), *a], check=True, capture_output=True, text=True).stdout
    (checkout / "api.py").write_text("a = 1\n")
    (checkout / "cron.py").write_text("b = 1\n")
    git("init", "-q"); git("add", "."); git("-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "x")
    patches = tmp_path / "patches"
    patches.mkdir()
    for name, file, new in [("0001-a.patch", "api.py", "a = 2\n"), ("0008-b.patch", "cron.py", "b = 2\n")]:
        (checkout / file).write_text(new)
        (patches / name).write_text(git("diff"))
        git("checkout", "--", file)
    git("apply", str(patches / "0001-a.patch"))
    return checkout, patches


def test_a_new_fix_is_put_on_without_updating_hermes(tmp_path, monkeypatch):
    checkout, patches = repo_with_patches(tmp_path)
    monkeypatch.setenv("HERMES_INSTALL_DIR", str(checkout))
    monkeypatch.setattr(update, "PATCHES", patches)
    mine = update.applied_now()
    assert [p.name for p in mine] == ["0001-a.patch"]
    missing = update.missing_fixes(update.patches(), mine)
    assert [p.name for p in missing] == ["0008-b.patch"]
    applied, skipped = update.put_on(hermes.Runner(), missing)
    assert skipped == [] and [p.split("/")[-1] for p in applied] == ["0008-b.patch"]
    assert (checkout / "cron.py").read_text() == "b = 2\n"
    assert update.missing_fixes(update.patches(), update.applied_now()) == []


def test_fixes_only_restarts_and_checks_once_the_fix_is_on(tmp_path, monkeypatch):
    checkout, patches = repo_with_patches(tmp_path)
    monkeypatch.setenv("HERMES_INSTALL_DIR", str(checkout))
    monkeypatch.setattr(update, "PATCHES", patches)
    restarted = []
    monkeypatch.setattr(update, "restart", lambda runner: restarted.append(True))
    monkeypatch.setattr(update, "check_agents", lambda agents, expect_clarify: {"bip": ""})
    mine = update.applied_now()
    code = update.fixes_only(SimpleNamespace(yes=True), [{"id": "bip", "name": "Bip"}], mine,
                             update.missing_fixes(update.patches(), mine), None)
    assert code == 0 and restarted == [True]
    assert (checkout / "cron.py").read_text() == "b = 2\n"


def test_a_fix_that_does_not_fit_changes_nothing(tmp_path, monkeypatch):
    checkout, patches = repo_with_patches(tmp_path)
    (checkout / "cron.py").write_text("b = 7\n")   # someone changed that line: 0008 no longer fits
    monkeypatch.setenv("HERMES_INSTALL_DIR", str(checkout))
    monkeypatch.setattr(update, "PATCHES", patches)
    monkeypatch.setattr(update, "restart", lambda runner: (_ for _ in ()).throw(AssertionError("no restart")))
    mine = update.applied_now()
    code = update.fixes_only(SimpleNamespace(yes=True), [], mine, update.missing_fixes(update.patches(), mine), None)
    assert code == 1
    assert (checkout / "cron.py").read_text() == "b = 7\n"


def update_run(tmp_path, monkeypatch, argv):
    """`hermes-update` on a Hermes 12 commits behind, with what it would run recorded instead of run."""
    checkout, patches = repo_with_patches(tmp_path)
    monkeypatch.setenv("HERMES_INSTALL_DIR", str(checkout))
    monkeypatch.setattr(update, "PATCHES", patches)
    bin_ = tmp_path / "hermes"
    bin_.write_text("")
    monkeypatch.setattr(hermes, "hermes_bin", lambda: str(bin_))
    monkeypatch.setattr(hermes, "hermes_home", lambda: tmp_path / "home")
    monkeypatch.setattr(update, "find_config", lambda path: None)
    monkeypatch.setattr(update, "agents_of", lambda config: [])
    monkeypatch.setattr(update, "preflight", lambda branch, wanted: ("abc", {}))
    ran = []
    real_run = subprocess.run

    def run(cmd, *a, **kw):
        if cmd[0] == str(bin_):
            ran.append(cmd[1:])
            return SimpleNamespace(stdout="Update channel: stable\n12 commits behind\n", returncode=0)
        return real_run(cmd, *a, **kw)
    monkeypatch.setattr(update.subprocess, "run", run)
    monkeypatch.setattr(hermes.Runner, "run", lambda self, cmd, **kw: ran.append(cmd))
    return update.main(argv), ran


def test_an_update_follows_hermes_and_parks_what_stays_changed(tmp_path, monkeypatch):
    """No hold since v0.22.5 is the pin (2026-11-01): `hermes update` runs, with --keep-stash so an older BipAgents'
    leftover never comes back on the new version, and our patches go back on after."""
    monkeypatch.setattr(update, "restart", lambda runner: None)
    monkeypatch.setattr(update, "check_agents", lambda agents, expect_clarify: {})
    code, ran = update_run(tmp_path, monkeypatch, ["--yes"])
    assert ["update", "--yes", "--no-gateway-restart", "--keep-stash"] in [c[1:] for c in ran if isinstance(c, list)]
    assert code == 0


def test_without_yes_it_asks_first(tmp_path, monkeypatch):
    asked = []
    monkeypatch.setattr(update, "confirm", lambda question: asked.append(question) or False)
    code, ran = update_run(tmp_path, monkeypatch, [])
    assert code == 1 and len(asked) == 1
    assert not [c for c in ran if isinstance(c, list) and "--keep-stash" in c]


def check_run(tmp_path, monkeypatch, capsys, pinned):
    """`hermes-update --check` with no existing-hermes.json (a fictional home), on a Hermes that is or isn't the pin."""
    monkeypatch.setenv("HOME", str(tmp_path / "account"))   # no ~/.config/bipagents/existing-hermes.json there
    monkeypatch.setattr(update, "check_agents", lambda agents, expect_clarify: {})
    checkout = tmp_path / "hermes-agent"
    if pinned:   # the pin is what the checkout is on (repo_with_patches makes it inside update_run)
        monkeypatch.setattr(hermes, "HERMES_COMMIT", "HEAD")
    else:        # a version this checkout has never seen, like Alex's own 517b5e10f against bf0203a794
        monkeypatch.setattr(hermes, "HERMES_COMMIT", "bf0203a794")
    code, ran = update_run(tmp_path, monkeypatch, ["--check"])
    assert code == 0 and not [c for c in ran if isinstance(c, list) and "--keep-stash" in c]
    assert checkout.exists()
    return capsys.readouterr().out


def test_check_offers_the_move_on_a_foreign_hermes_without_the_record(tmp_path, monkeypatch, capsys):
    out = check_run(tmp_path, monkeypatch, capsys, pinned=False)
    assert "is NOT the version BipAgents is built and tested on" in out
    assert "bipagents hermes-update --to-bipagents" in out
    assert "backs up" in out and "goes back as it was by itself" in out


def test_check_says_nothing_of_the_move_on_bipagents_version(tmp_path, monkeypatch, capsys):
    out = check_run(tmp_path, monkeypatch, capsys, pinned=True)
    assert "--to-bipagents" not in out and "NOT the version" not in out
