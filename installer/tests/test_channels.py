"""install.sh's channels and public repositories (2026-11-15): an install follows `stable` where the repository has it,
else `main`; `--channel pre-test` follows each publication first and moves the install to the repository that has that
channel (the BipAgents organisation's); a place of the offer goes to the commit and repository its guichet's order
names (BIPAGENTS_TO, BIPAGENTS_TO_REPO), among the known public repositories only.

The two public repositories are local ones here: git's `url.<local>.insteadOf` turns their GitHub addresses into
folders, so install.sh sees and keeps the real addresses. uv and the installer are stand-ins that only say how they
were called."""
from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

from bipinstall import public_repo

REPO = Path(__file__).resolve().parents[2]
OLD = public_repo.url_of("Alexbob0/BipAgents")
NEW = public_repo.url_of("BipAgents/BipAgents")


def git(repo, *args, env=None):
    return subprocess.run(["git", "-C", str(repo), "-c", "user.name=t", "-c", "user.email=t@t", *args], check=True,
                          capture_output=True, text=True, env=env).stdout.strip()


def commit(repo, text):
    (repo / "installer").mkdir(exist_ok=True)
    (repo / "installer" / "version.txt").write_text(text)
    git(repo, "add", "-A")
    git(repo, "commit", "-qm", text)
    return git(repo, "rev-parse", "HEAD")


class World:
    def __init__(self, tmp: Path):
        self.tmp = tmp
        self.old = tmp / "old"            # Alexbob0/BipAgents: main only
        self.new = tmp / "new"            # BipAgents/BipAgents: 0.1 on stable, then pre-test ahead
        for repo, branch in ((self.old, "main"), (self.new, "stable")):
            repo.mkdir()
            git(repo, "init", "-q", "-b", branch)
            git(repo, "config", "uploadpack.allowFilter", "true")
            (repo / ".gitignore").write_text("installer/.venv/\n")
        self.old_main = commit(self.old, "old 1")
        self.stable = commit(self.new, "0.1")
        git(self.new, "branch", "pre-test")
        git(self.new, "checkout", "-q", "pre-test")
        self.pretest = commit(self.new, "0.1 + pre-test")
        git(self.new, "checkout", "-q", "stable")
        gitconfig = tmp / "gitconfig"
        gitconfig.write_text(f'[url "file://{self.old}"]\n\tinsteadOf = {OLD}\n'
                             f'[url "file://{self.new}"]\n\tinsteadOf = {NEW}\n'
                             '[protocol "file"]\n\tallow = always\n')
        bin_ = tmp / "bin"
        bin_.mkdir()
        self.calls = tmp / "calls.txt"
        python = f'#!/bin/sh\necho "$@" >> {self.calls}\n'
        (bin_ / "uv").write_text('#!/bin/sh\nif [ "$1" = venv ]; then for a; do last="$a"; done; '
                                 f'mkdir -p "$last/bin"; printf %s \'{python}\' > "$last/bin/python"; '
                                 'chmod +x "$last/bin/python"; fi\n')
        (bin_ / "uv").chmod(0o755)
        self.env = {**os.environ, "GIT_CONFIG_GLOBAL": str(gitconfig), "GIT_CONFIG_NOSYSTEM": "1",
                    "PATH": f"{bin_}:{os.environ['PATH']}", "HOME": str(tmp), "BIPAGENTS_DIR": str(tmp / "BipAgents")}
        for name in ("BIPAGENTS_TO", "BIPAGENTS_TO_REPO", "BIPAGENTS_CHANNEL", "BIPAGENTS_REPO"):
            self.env.pop(name, None)
        self.dir = tmp / "BipAgents"

    def install_sh(self, *args, **env):
        return subprocess.run(["bash", str(REPO / "install.sh"), *args], env={**self.env, **env}, capture_output=True,
                              text=True, stdin=subprocess.DEVNULL, timeout=120)

    def installed_from(self, repo_url):
        """A light install of the time before channels (install.sh's clone then: main, upstream origin/main)."""
        subprocess.run(["git", "clone", "-q", "--depth", "1", "--filter=blob:none", repo_url, str(self.dir)],
                       env=self.env, check=True, capture_output=True)

    def head(self):
        return git(self.dir, "rev-parse", "HEAD")

    def origin(self):
        return git(self.dir, "config", "remote.origin.url")

    def followed(self):
        return git(self.dir, "config", "branch.main.merge")

    def installer_calls(self):
        return self.calls.read_text().splitlines() if self.calls.exists() else []


@pytest.fixture
def world(tmp_path):
    return World(tmp_path)


def test_an_install_on_the_first_repository_follows_main_as_before(world):
    world.installed_from(OLD)
    newer = commit(world.old, "old 2")
    done = world.install_sh("update", "--auto")
    assert done.returncode == 0, done.stderr
    assert (world.head(), world.origin(), world.followed()) == (newer, OLD, "refs/heads/main")
    assert world.installer_calls()[-1] == "-m bipinstall update --auto"


def test_channel_pre_test_moves_the_install_to_the_organisations_repository_and_stays(world):
    world.installed_from(OLD)
    done = world.install_sh("update", "--channel", "pre-test")
    assert done.returncode == 0, done.stderr
    assert "BipAgents now comes from https://github.com/BipAgents/BipAgents.git (pre-test)." in done.stderr
    assert (world.head(), world.origin(), world.followed()) == (world.pretest, NEW, "refs/heads/pre-test")
    assert git(world.dir, "config", "bipagents.channel") == "pre-test"
    assert world.installer_calls()[-1] == "-m bipinstall update", "--channel is install.sh's own"
    # the next publication: `bipagents update` alone follows it
    git(world.new, "checkout", "-q", "pre-test")
    later = commit(world.new, "pre-test 2")
    assert world.install_sh("update").returncode == 0
    assert (world.head(), world.followed()) == (later, "refs/heads/pre-test")
    # back to stable, which is behind: taken as it is
    done = world.install_sh("update", "--channel=stable")
    assert done.returncode == 0, done.stderr
    assert (world.head(), world.followed()) == (world.stable, "refs/heads/stable")
    # stable promoted: it follows
    git(world.new, "checkout", "-q", "stable")
    git(world.new, "merge", "-q", "--ff-only", "pre-test")
    assert world.install_sh("update").returncode == 0
    assert world.head() == later


def test_a_channel_nobody_publishes_yet_keeps_main(world):
    world.installed_from(OLD)
    (world.tmp / "gitconfig").write_text((world.tmp / "gitconfig").read_text().replace(str(world.new), "/nowhere"))
    done = world.install_sh("update", "--channel", "pre-test")
    assert done.returncode == 0, done.stderr
    assert "no pre-test channel on https://github.com/Alexbob0/BipAgents.git yet: main as before." in done.stderr
    assert (world.origin(), world.followed()) == (OLD, "refs/heads/main")


def test_a_new_install_from_the_organisations_repository_follows_stable(world):
    done = world.install_sh(BIPAGENTS_REPO=NEW)
    assert done.returncode == 0, done.stderr
    assert (world.head(), world.origin(), world.followed()) == (world.stable, NEW, "refs/heads/stable")
    assert git(world.dir, "branch", "--format=%(refname:short)") == "main"
    assert git(world.dir, "rev-parse", "--is-shallow-repository") == "true"
    done = world.install_sh(BIPAGENTS_REPO=NEW, BIPAGENTS_CHANNEL="pre-test", BIPAGENTS_DIR=str(world.tmp / "B2"))
    assert done.returncode == 0, done.stderr
    assert git(world.tmp / "B2", "rev-parse", "HEAD") == world.pretest


def test_a_place_goes_to_the_commit_and_repository_its_order_names_and_back(world):
    world.installed_from(OLD)
    to_new = world.install_sh("update", "--auto", BIPAGENTS_TO=world.pretest, BIPAGENTS_TO_REPO=NEW)
    assert to_new.returncode == 0, to_new.stderr
    assert (world.head(), world.origin()) == (world.pretest, NEW)
    back = world.install_sh("update", "--auto", BIPAGENTS_TO=world.old_main, BIPAGENTS_TO_REPO=OLD)
    assert back.returncode == 0, back.stderr
    assert (world.head(), world.origin()) == (world.old_main, OLD)


def test_an_order_naming_an_unknown_repository_changes_nothing(world):
    world.installed_from(OLD)
    done = world.install_sh("update", "--auto", BIPAGENTS_TO=world.pretest,
                            BIPAGENTS_TO_REPO="https://github.com/someone/BipAgents.git")
    assert done.returncode == 2 and "is not one of BipAgents' public repositories" in done.stderr
    assert (world.head(), world.origin()) == (world.old_main, OLD)
    assert world.install_sh("update", "--channel", "nightly").returncode == 2


def test_a_developers_clone_follows_what_it_follows(world):
    private = world.tmp / "private"
    subprocess.run(["git", "clone", "-q", f"file://{world.old}", str(private)], check=True, capture_output=True)
    git(world.old, "checkout", "-q", "-b", "topic")
    commit(world.old, "topic")
    git(private, "fetch", "-q")
    git(private, "checkout", "-q", "-b", "topic", "origin/topic")
    fetch = git(private, "config", "--get-all", "remote.origin.fetch")
    done = world.install_sh("update", "--channel", "pre-test", BIPAGENTS_DIR=str(private))
    assert done.returncode == 0, done.stderr
    assert git(private, "config", "--get-all", "remote.origin.fetch") == fetch
    assert git(private, "config", "remote.origin.url") == f"file://{world.old}"
