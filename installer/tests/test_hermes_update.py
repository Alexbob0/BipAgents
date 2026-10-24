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
