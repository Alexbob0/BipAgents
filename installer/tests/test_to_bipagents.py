"""`bipagents hermes-update --to-bipagents`: its plan, its settings check, its backup and its roll back (fictional
data only), and the new fixes offered on a person's own Hermes."""
import json
import sqlite3
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from bipinstall import hermes, to_bipagents as tb, update

CONFIG = """_config_version: 50
model:
  provider: custom
  default: qwen3-30b-local
  base_url: http://192.168.50.10:8080/v1
display:
  skin: default
agent:
  max_turns: 60
  reasoning_overrides: {}
"""


def git(folder, *args):
    return subprocess.run(["git", "-C", str(folder), *args], check=True, capture_output=True, text=True).stdout.strip()


def commit(folder, name, text):
    (folder / name).write_text(text)
    git(folder, "add", ".")
    git(folder, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", name)
    return git(folder, "rev-parse", "HEAD")


def snap(config=CONFIG, env="API_SERVER_KEY=abc\n", memory="Fictional garden: 3 tomatoes.\n"):
    return {"/h/.hermes": {"config.yaml": config, ".env": env, "memories/MEMORY.md": memory}}


# -- the settings check ------------------------------------------------------------------------------------------------

def test_hermes_own_migration_is_no_change_of_settings():
    after = CONFIG.replace("_config_version: 50", "_config_version: 51").replace("  reasoning_overrides: {}\n", "")
    after += "new_feature:\n  enabled: true\n"   # a default the newer Hermes adds
    assert tb.settings_problems(snap(), snap(config=after)) == []


def test_a_changed_model_or_key_or_memory_is_a_problem():
    moved = CONFIG.replace("192.168.50.10:8080", "localhost:9999")
    assert any("model" in p for p in tb.settings_problems(snap(), snap(config=moved)))
    assert tb.settings_problems(snap(), snap(env="API_SERVER_KEY=other\n")) == [".hermes: .env changed"]
    assert tb.settings_problems(snap(), snap(memory="")) == [".hermes: memories/MEMORY.md changed"]
    lost = CONFIG.replace("  max_turns: 60\n", "")
    assert tb.settings_problems(snap(), snap(config=lost)) == [".hermes: setting agent.max_turns is gone"]


def test_the_model_section_is_compared_as_written():
    reordered = CONFIG.replace("  default: qwen3-30b-local\n  base_url: http://192.168.50.10:8080/v1\n",
                               "  base_url: http://192.168.50.10:8080/v1\n  default: qwen3-30b-local\n")
    assert tb._section(CONFIG, "model").startswith("model:\n  provider: custom")
    assert tb.settings_problems(snap(), snap(config=reordered)) == [".hermes: config.yaml's « model » changed"]


def test_lost_conversations_are_a_problem():
    assert tb.conversation_problems({"/h/.hermes": 6}, {"/h/.hermes": 7}) == []
    assert tb.conversation_problems({"/h/.hermes": 6}, {"/h/.hermes": 4}) == [".hermes: 2 message(s) missing"]
    assert tb.conversation_problems({"/h/.hermes": 6}, {"/h/.hermes": -1}) == [".hermes: its conversations don't read"]


# -- the plan ----------------------------------------------------------------------------------------------------------

def a_plan(**kw):
    values = dict(checkout="/h/.hermes/hermes-agent", hermes_home="/h/.hermes", current="a" * 40, target="b" * 40,
                  relation="behind", behind=6911, applied=[], fixes=["0005-x.patch"], homes=[("Hermes", "/h/.hermes")],
                  units=[("systemd", "hermes-gateway")], gateway="service", code_bytes=2 * 1024 ** 3,
                  free_bytes=40 * 1024 ** 3, backup="/h/.local/state/bipagents/hermes-move/x")
    values.update(kw)
    return tb.Plan(**values)


def test_a_clean_hermes_behind_bipagents_can_move():
    assert tb.blockers(a_plan()) == []


@pytest.mark.parametrize("change, words", [
    (dict(relation="ahead", behind=None), "newer than BipAgents"),
    (dict(relation="diverged", behind=None), "isn't on the line"),
    (dict(relation="unknown", behind=None), "can't fetch"),
    (dict(foreign=["tools/mine.py"]), "aren't BipAgents' fixes"),
    (dict(gateway="manual"), "runs by hand"),
    (dict(free_bytes=1 * 1024 ** 3), "Not enough free disk"),
])
def test_what_stops_the_move_before_anything_changes(change, words):
    found = tb.blockers(a_plan(**change))
    assert len(found) == 1 and words in found[0]


def test_where_the_checkout_stands_against_bipagents_version(tmp_path):
    repo = tmp_path / "hermes-agent"
    repo.mkdir()
    git(repo, "init", "-q")
    old = commit(repo, "a.py", "1\n")
    new = commit(repo, "a.py", "2\n")
    commit(repo, "a.py", "3\n")
    assert tb.relation(repo, old, new) == ("behind", 1)
    assert tb.relation(repo, new, old) == ("ahead", None)
    assert tb.relation(repo, new, new) == ("same", 0)
    git(repo, "checkout", "-q", old)
    fork = commit(repo, "b.py", "mine\n")
    assert tb.relation(repo, fork, new) == ("diverged", None)
    assert tb.relation(repo, old, "f" * 40) == ("unknown", None)


def test_only_the_persons_own_changes_stop_it(tmp_path):
    patch = tmp_path / "0005-x.patch"
    patch.write_text("diff --git a/tools/browser_vault_tool.py b/tools/browser_vault_tool.py\n--- a\n+++ b\n")
    ours = frozenset({"tools/browser_vault_tool.py", "gateway/platforms/api_server.py"}) | tb.EARLIER_FIX_FILES
    changed = ["tools/browser_vault_tool.py", "notes.txt", "gateway/platforms/api_server.py", "agent/image_store.py"]
    theirs, older = tb.sort_changes(changed, [patch], ours)
    assert theirs == ["notes.txt"]   # a blocker: never parked without asking
    assert older == ["gateway/platforms/api_server.py", "agent/image_store.py"]   # an older BipAgents' fixes


def test_older_fixes_are_parked_before_hermes_installer_runs(tmp_path, monkeypatch):
    checkout = tmp_path / "hermes-agent"
    checkout.mkdir()
    git(checkout, "init", "-q")
    commit(checkout, "api.py", "a = 1\n")
    (checkout / "api.py").write_text("a = 'older fix'\n")
    p = a_plan(checkout=str(checkout), older=["api.py"], backup=str(tmp_path / "b"))
    (tmp_path / "b").mkdir()
    with pytest.raises(RuntimeError, match="fetch"):   # parked, then the target fetched (no origin here)
        tb.move_code(p, hermes.Runner(env={"GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t",
                                           "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"}))
    assert (checkout / "api.py").read_text() == "a = 1\n"
    assert "bipagents-move-b: older BipAgents fixes" in git(checkout, "stash", "list")
    (checkout / "mine.txt").write_text("the person's own\n")
    with pytest.raises(tb.MoveFailed, match="mine.txt"):
        tb.move_code(a_plan(checkout=str(checkout), backup=str(tmp_path / "b")), hermes.Runner())


def test_the_target_is_read_whole_from_base(tmp_path):
    base = tmp_path / "BASE"
    base.write_text("bf0203a794a07aac3ff764972ab9087baa4e4f4e v0.22.5\n")
    assert tb.full_target(tmp_path, "bf0203a794", base) == "bf0203a794a07aac3ff764972ab9087baa4e4f4e"


# -- backup and roll back ----------------------------------------------------------------------------------------------

def fake_install(home: Path):
    """A person's own Hermes, fictional: its checkout, environment, commands, two profiles, conversations, a service."""
    hh = home / ".hermes"
    checkout = hh / "hermes-agent"
    checkout.mkdir(parents=True)
    git(checkout, "init", "-q")
    old = commit(checkout, "hermes_cli.py", "VERSION = 'old'\n")
    for part in ("installs/f613/venv", "tools/python-3.14.7"):
        (hh / part).mkdir(parents=True)
        (hh / part / "marker").write_text("old\n")
    (home / ".local" / "bin").mkdir(parents=True)
    (home / ".local" / "bin" / "hermes").write_text("#!/bin/sh\nexec old \"$@\"\n")
    for folder in (hh, hh / "profiles" / "coach"):
        (folder / "memories").mkdir(parents=True)
        (folder / "config.yaml").write_text(CONFIG)
        (folder / ".env").write_text("API_SERVER_KEY=fictional\n")
        (folder / "memories" / "MEMORY.md").write_text("Fictional memory.\n")
        db = sqlite3.connect(folder / "state.db")
        db.execute("CREATE TABLE messages (id INTEGER PRIMARY KEY, content TEXT)")
        db.executemany("INSERT INTO messages (content) VALUES (?)", [("Fictional question",), ("Fictional answer",)])
        db.commit()
        db.close()
    units = home / ".config" / "systemd" / "user"
    units.mkdir(parents=True)
    (units / "hermes-gateway.service").write_text("[Service]\nExecStart=old gateway run\n")
    record = home / ".config" / "bipagents" / "existing-hermes.json"
    record.write_text(json.dumps({"preexisting": True, "patched": ["/r/0005-x.patch"]}))
    return hh, checkout, old, record


def moved_on(hh: Path, checkout: Path, home: Path):
    """What a move leaves: new code, new environment, Hermes' migration, conversations written, a rewritten service."""
    commit(checkout, "hermes_cli.py", "VERSION = 'new'\n")
    (hh / "tools" / "python-3.14.9").mkdir()
    (hh / "installs" / "f613" / "venv" / "marker").write_text("new\n")
    (home / ".local" / "bin" / "hermes").write_text("#!/bin/sh\nexec new \"$@\"\n")
    (hh / "config.yaml").write_text(CONFIG.replace("_config_version: 50", "_config_version: 51"))
    db = sqlite3.connect(hh / "state.db")
    db.execute("ALTER TABLE messages ADD COLUMN v2 TEXT")
    db.execute("INSERT INTO messages (content) VALUES ('after the move')")
    db.commit()
    db.close()
    (home / ".config" / "systemd" / "user" / "hermes-gateway.service").write_text("[Service]\nExecStart=new\n")


@pytest.fixture
def account(tmp_path, monkeypatch):
    home = tmp_path / "home"
    (home / ".config" / "bipagents").mkdir(parents=True)
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
    monkeypatch.setenv("HERMES_HOME", str(home / ".hermes"))
    monkeypatch.delenv("HERMES_INSTALL_DIR", raising=False)
    monkeypatch.delenv("BIPAGENTS_MOVE_FAIL_AT", raising=False)
    return home


def plan_for(home, hh, checkout, old):
    return a_plan(checkout=str(checkout), hermes_home=str(hh), current=old, homes=[("Hermes", str(hh))],
                  backup=str(home / tb.MOVES / "20261107-150000"), units=[("systemd", "hermes-gateway")])


def test_a_roll_back_puts_code_data_services_and_record_back(account):
    hh, checkout, old, record = fake_install(account)
    p = plan_for(account, hh, checkout, old)
    runner = hermes.Runner(dry=True)
    journal = tb.back_up(p, None, record, runner)
    assert set(journal["code"]) == {str(checkout), str(hh / "installs"), str(hh / "tools"),
                                    str(account / ".local" / "bin" / "hermes")}
    assert journal["messages"] == {str(hh): 2, str(hh / "profiles" / "coach"): 2}
    folder = Path(p.backup)
    assert oct(folder.stat().st_mode)[-3:] == "700" and oct((folder / tb.DATA).stat().st_mode)[-3:] == "600"
    before = (hh / "config.yaml").read_bytes()

    moved_on(hh, checkout, account)
    record.write_text(json.dumps({"preexisting": True, "patched": ["all 20"]}))
    assert tb.roll_back(folder, runner, check=lambda: True)

    assert git(checkout, "rev-parse", "HEAD") == old
    assert (checkout / "hermes_cli.py").read_text() == "VERSION = 'old'\n"
    assert (hh / "installs" / "f613" / "venv" / "marker").read_text() == "old\n"
    assert not (hh / "tools" / "python-3.14.9").exists()
    assert "old" in (account / ".local" / "bin" / "hermes").read_text()
    assert (hh / "config.yaml").read_bytes() == before
    assert tb.conversations([hh])[str(hh)] == 2
    assert "old" in (account / ".config" / "systemd" / "user" / "hermes-gateway.service").read_text()
    assert json.loads(record.read_text())["patched"] == ["/r/0005-x.patch"]
    assert tb.read_journal(folder)["step"] == "rolled-back"
    assert not list(hh.glob(".*.bipagents-moved"))   # the moved code doesn't stay beside it
    assert any("systemctl --user start hermes-gateway" in line for line in runner.log)


class FakeDrain:
    def __init__(self):
        self.calls = []

    def until_quiet(self, what):
        self.calls.append(what)
        return "No turn going on."

    def done(self):
        self.calls.append("done")


def run_move(account, monkeypatch, *, move=None, fixes=None, answers=True):
    hh, checkout, old, record = fake_install(account)
    p = plan_for(account, hh, checkout, old)
    monkeypatch.setattr(tb, "plan", lambda *a, **k: p)
    for name in ("stop_units", "start_units"):
        monkeypatch.setattr(tb, name, lambda units, runner: None)
    monkeypatch.setattr(tb, "active_units", lambda units, runner: list(units))
    monkeypatch.setattr(tb, "move_code", move or (lambda p, runner: commit(checkout, "hermes_cli.py", "new\n")))
    monkeypatch.setattr(tb, "put_fixes_on", fixes or (lambda p, d, runner: ["/r/0005-x.patch", "/r/0026-y.patch"]))
    monkeypatch.setattr(update, "report", lambda agents, problems: not any(problems.values()))
    remembered = []
    drain = FakeDrain()
    code = tb.run(SimpleNamespace(yes=True), None, [{"id": "vie", "name": "Vie"}], [Path("/r/0005-x.patch")],
                  Path("/r/patches"), lambda agents, expect_clarify: {"vie": "" if answers else "isn't answering"},
                  remembered.append, drain_module=drain)
    return code, p, remembered, drain, hh, checkout, old


def test_a_move_that_goes_through_records_every_fix(account, monkeypatch):
    code, p, remembered, drain, hh, checkout, old = run_move(account, monkeypatch)
    assert code == 0
    assert remembered == [["/r/0005-x.patch", "/r/0026-y.patch"]]
    assert drain.calls == ["gateways", "done"]
    journal = tb.read_journal(Path(p.backup))
    assert journal["step"] == "done" and (Path(p.backup) / "code").is_dir()   # kept for --undo-move
    assert "settings" not in journal and "fictional" not in (Path(p.backup) / tb.JOURNAL).read_text()


@pytest.mark.parametrize("where", ["install", "fixes", "check"])
def test_a_failure_anywhere_rolls_back_by_itself(account, monkeypatch, where):
    monkeypatch.setenv("BIPAGENTS_MOVE_FAIL_AT", where)
    code, p, remembered, drain, hh, checkout, old = run_move(account, monkeypatch)
    assert code == 1 and remembered == []
    assert git(checkout, "rev-parse", "HEAD") == old
    assert tb.read_journal(Path(p.backup))["step"] == "rolled-back"
    assert drain.calls[-1] == "done"


def test_a_changed_model_rolls_back(account, monkeypatch):
    def bad_move(p, runner):
        cfg = Path(p.hermes_home) / "config.yaml"
        cfg.write_text(cfg.read_text().replace("qwen3-30b-local", "something-else"))
    code, p, remembered, drain, hh, checkout, old = run_move(account, monkeypatch, move=bad_move)
    assert code == 1 and remembered == []
    assert "qwen3-30b-local" in (hh / "config.yaml").read_text()


def test_an_agent_not_answering_rolls_back(account, monkeypatch):
    code, p, remembered, drain, hh, checkout, old = run_move(account, monkeypatch, answers=False)
    assert code == 1 and git(checkout, "rev-parse", "HEAD") == old


def test_a_vault_out_of_reach_after_the_move_rolls_back(account, monkeypatch):
    """Alex's move of 7/11: every agent answered, but the spaces' vault Python was the one before (3.14.7 kept,
    Hermes on 3.14.9): every login given to them was refused. The move now checks the vault too."""
    monkeypatch.setattr(tb, "vault_problems", lambda config: ["kerfy-studio"])
    code, p, remembered, drain, hh, checkout, old = run_move(account, monkeypatch)
    assert code == 1 and remembered == [] and git(checkout, "rev-parse", "HEAD") == old
    assert tb.read_journal(Path(p.backup))["step"] == "rolled-back"


def test_the_vault_check_reads_the_bridges_answer(tmp_path, monkeypatch):
    config = tmp_path / "bridge.toml"
    assert tb.vault_problems(None) == [] and tb.vault_problems(config) == []   # no bridge here
    config.write_text("[agents.vie]\n")
    monkeypatch.setattr("bipinstall.rustbridge.cli", lambda home, repo=None: ["bipbridge"])

    def answer(code, out):
        return lambda args, **kw: subprocess.CompletedProcess(args, code, out, "")
    out = "vie: vault OK\nkerfy-studio: vault out of reach (kerfy-studio: the agent's vault could not be reached)\n"
    assert tb.vault_problems(config, run=answer(1, out)) == ["kerfy-studio"]
    assert tb.vault_problems(config, run=answer(0, "vie: vault OK\n")) == []
    # a bridge from before vault-check: argparse's « invalid choice » (2), not a problem of the move
    assert tb.vault_problems(config, run=answer(2, "")) == []
    seen = []
    tb.vault_problems(config, run=lambda args, **kw: seen.append(args) or subprocess.CompletedProcess(args, 0, "", ""))
    assert seen == [["bipbridge", "vault-check", "--config", str(config)]]


def test_ctrl_c_mid_way_rolls_back(account, monkeypatch):
    def interrupted(p, runner):
        commit(Path(p.checkout), "hermes_cli.py", "half\n")
        raise KeyboardInterrupt
    code, p, remembered, drain, hh, checkout, old = run_move(account, monkeypatch, move=interrupted)
    assert code == 1 and git(checkout, "rev-parse", "HEAD") == old


def test_undo_takes_back_a_move_that_went_through(account, monkeypatch):
    code, p, remembered, drain, hh, checkout, old = run_move(account, monkeypatch)
    assert code == 0 and git(checkout, "rev-parse", "HEAD") != old
    monkeypatch.setattr(tb, "stop_units", lambda units, runner: None)
    monkeypatch.setattr(tb, "start_units", lambda units, runner: None)
    drain = FakeDrain()
    assert tb.undo(SimpleNamespace(yes=True), [], lambda agents, expect_clarify: {}, drain_module=drain) == 0
    assert git(checkout, "rev-parse", "HEAD") == old and drain.calls == ["gateways", "done"]
    assert tb.undo(SimpleNamespace(yes=True), [], lambda agents, expect_clarify: {}, drain_module=drain) == 1


# -- new fixes offered on the person's own Hermes ------------------------------------------------------------------------

def test_new_fixes_are_those_neither_chosen_nor_on_nor_refused(tmp_path, monkeypatch):
    from test_hermes_update import repo_with_patches
    checkout, patches = repo_with_patches(tmp_path)
    monkeypatch.setenv("HERMES_INSTALL_DIR", str(checkout))
    monkeypatch.setattr(update, "PATCHES", patches)
    mine = update.applied_now()
    assert [p.name for p in update.new_fixes([], mine, [], False)] == ["0008-b.patch"]
    assert update.new_fixes([], mine, ["0008-b.patch"], False) == []
    assert [p.name for p in update.new_fixes([], mine, ["0008-b.patch"], True)] == ["0008-b.patch"]
    assert update.new_fixes(None, mine, [], False) == []   # a Hermes that came with BipAgents gets them all anyway
    assert [p.name for p in update.fitting(update.new_fixes([], mine, [], False), mine)] == ["0008-b.patch"]
    assert (checkout / "cron.py").read_text() == "b = 1\n"   # tried on a copy only


def test_a_new_fix_that_does_not_fit_is_not_offered(tmp_path, monkeypatch):
    from test_hermes_update import repo_with_patches
    checkout, patches = repo_with_patches(tmp_path)
    (checkout / "cron.py").write_text("b = 7\n")
    git(checkout, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qam", "theirs")
    monkeypatch.setenv("HERMES_INSTALL_DIR", str(checkout))
    monkeypatch.setattr(update, "PATCHES", patches)
    mine = update.applied_now()
    assert update.fitting(update.new_fixes([], mine, [], False), mine) == []


def test_the_answer_picks_all_none_or_some():
    offered = [Path("0005-a.patch"), Path("0017-b.patch"), Path("0024-c.patch")]
    assert update.pick("y", offered) == offered
    assert update.pick("", offered) == [] and update.pick("n", offered) == []
    assert update.pick("0005, 24", offered) == [offered[0], offered[2]]


def test_offered_fixes_go_on_when_taken_and_are_remembered_when_refused(tmp_path, monkeypatch):
    from test_hermes_update import repo_with_patches
    checkout, patches = repo_with_patches(tmp_path)
    home = tmp_path / "home"
    record = home / ".config" / "bipagents" / "existing-hermes.json"
    record.parent.mkdir(parents=True)
    record.write_text(json.dumps({"preexisting": True, "patched": []}))
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
    monkeypatch.setenv("HERMES_INSTALL_DIR", str(checkout))
    monkeypatch.setattr(update, "PATCHES", patches)
    monkeypatch.setattr(update, "restart", lambda runner: None)
    monkeypatch.setattr(update, "check_agents", lambda agents, expect_clarify: {})
    mine = update.applied_now()
    offered = update.fitting(update.new_fixes([], mine, [], False), mine)
    assert update.offer_new(None, [], mine, [], offered, ask=lambda q: "n") == 1
    assert json.loads(record.read_text())["declined"] == ["0008-b.patch"]
    assert update.offer_new(None, [], mine, [], offered, ask=lambda q: "8") == 0
    assert (checkout / "cron.py").read_text() == "b = 2\n"
    assert [Path(p).name for p in json.loads(record.read_text())["patched"]] == ["0008-b.patch"]


def test_a_fix_stacked_under_a_later_one_still_counts_as_on(tmp_path, monkeypatch):
    """0018 under 0019 and 0024 on v0.22.5: alone it no longer reverse-applies, yet the whole set is there."""
    checkout = tmp_path / "hermes-agent"
    checkout.mkdir()
    git(checkout, "init", "-q")
    commit(checkout, "api.py", "a = 1\n")
    patches = tmp_path / "patches"
    patches.mkdir()
    for name, new in [("0018-a.patch", "a = 2\n"), ("0019-b.patch", "a = 3\n")]:
        (checkout / "api.py").write_text(new)
        (patches / name).write_text(subprocess.run(["git", "-C", str(checkout), "diff"], capture_output=True,
                                                   text=True).stdout)
        git(checkout, "add", "api.py")   # the next patch is made on top of this one
    git(checkout, "reset", "-q")
    monkeypatch.setenv("HERMES_INSTALL_DIR", str(checkout))
    monkeypatch.setattr(update, "PATCHES", patches)
    assert subprocess.run(["git", "-C", str(checkout), "apply", "--reverse", "--check",
                           str(patches / "0018-a.patch")], capture_output=True).returncode != 0
    assert [p.name for p in update.applied_now()] == ["0018-a.patch", "0019-b.patch"]
    (checkout / "api.py").write_text("a = 1\n")   # none on
    assert update.applied_now() == []
