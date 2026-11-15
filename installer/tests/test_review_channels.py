"""Independent review of pretest-prod (15/11), install.sh's channels: failing tests (briefs/review-pretest-prod.md).
The author's harness (tests/test_channels.py): local repositories for the two public ones, stand-ins for uv and the
installer."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from test_channels import NEW, OLD, World, commit, git  # noqa: E402


# R7 — `publish-public.sh --stable-back` (the way back of a promoted version gone wrong) moves stable back, and its
# comment says « the self-hosted installs follow it back (install.sh) ». They don't: an install that already took the
# bad version has the older commit in its history, `git pull --ff-only` says « Already up to date » and stays on it.
def test_r7_a_stable_install_follows_stable_back(tmp_path):
    world = World(tmp_path)
    done = world.install_sh(BIPAGENTS_REPO=NEW)
    assert done.returncode == 0, done.stderr
    assert world.head() == world.stable
    # promote: stable = the pre-tested commit; the nightly update takes it
    git(world.new, "merge", "-q", "--ff-only", "pre-test")
    assert world.install_sh("update", "--auto").returncode == 0
    assert world.head() == world.pretest
    # it goes wrong: stable back to the version before (publish-public.sh --stable-back: a forced push)
    git(world.new, "reset", "-q", "--hard", world.stable)
    done = world.install_sh("update", "--auto")
    assert done.returncode == 0, done.stderr
    assert world.head() == world.stable, (
        f"still on {world.head()[:12]}, the version stable went back from (stable is {world.stable[:12]})")


# R8 — `bipagents update --check` goes through install.sh, which moves the checkout before the installer is asked to
# « only say what is new »; with --channel it also moves the install to another repository and branch. The next real
# update then starts from the moved HEAD and sees nothing to do for what only `touched()` triggers (Pocket, voices,
# the browser image, Hermes' patches).
def test_r8_update_check_changes_nothing(tmp_path):
    world = World(tmp_path)
    world.installed_from(OLD)
    before = (world.head(), world.origin(), world.followed())
    commit(world.old, "old 2")
    done = world.install_sh("update", "--check", "--channel", "pre-test")
    assert done.returncode == 0, done.stderr
    assert (world.head(), world.origin(), world.followed()) == before, "a check moved the install"
