"""The BipAgents checkout of an install (~/BipAgents), kept light: only the latest commit (`--depth 1`), file contents
fetched only for what is checked out (`--filter=blob:none`), and what a server never reads left out (sparse
checkout): the apps' sources, the site, the voices' sample recordings, and the Bips' Pocket voice states
(voices/<language>/*.safetensors, about 200 MB) unless this account runs Pocket's own models. install.sh clones it
that way, without the voice states (`narrow` adds them for Pocket's own models: their contents are fetched then);
`lighten` brings an older full clone there (migration light-disk).

`git pull --ff-only` keeps working (new commits come with the contents of the files checked out only), and a
rewritten public history is taken as it is by install.sh's resync (`fetch --depth 1` then `checkout -B main`).
Contents left out are fetched from GitHub when something asks for them (`git show <commit>:<path>`)."""
from __future__ import annotations

import re
import subprocess
from pathlib import Path
from typing import List, Optional

from . import hermes

# The public repository (https or ssh), not the private ones next to it (bipagents-private, -guichet).
PUBLIC = re.compile(r"github\.com[/:]Alexbob0/BipAgents(\.git)?/?$")
# What every install checks out (gitignore syntax, non-cone): all but the apps, the site and the recordings.
# install.sh writes the same list, with VOICE_STATES.
BASE = ["/*", "!/App/", "!/android/", "!/site/", "!/voices/**/*.wav"]
# Pocket's own voice states: only where pocket_server.py runs with them (--voices), not with the light voices
# (--onnx) or when another account's Pocket is shared.
VOICE_STATES = "!/voices/**/*.safetensors"


def patterns(voice_states: bool) -> List[str]:
    return BASE + ([] if voice_states else [VOICE_STATES])


def git(repo: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True)


def is_install_clone(repo: Path) -> bool:
    """An install's checkout of the public repository, nothing committed on it (a developer's clone is never
    touched)."""
    if not (repo / ".git").is_dir():
        return False
    url = git(repo, "remote", "get-url", "origin").stdout.strip()
    if not PUBLIC.search(url):
        return False
    ahead = git(repo, "rev-list", "--count", "@{u}..HEAD")
    return ahead.returncode != 0 or ahead.stdout.strip() in ("", "0")


def is_sparse(repo: Path) -> bool:
    return git(repo, "config", "--bool", "core.sparseCheckout").stdout.strip() == "true"


def narrow(repo: Path, runner: hermes.Runner, voice_states: bool) -> bool:
    """A light checkout's files set for what this account runs (Pocket's voice states or not). Whether it applied
    (False: not a light install checkout, left as it is)."""
    if not runner.dry and not (is_install_clone(repo) and is_sparse(repo)):
        return False
    runner.run(["git", "-C", str(repo), "sparse-checkout", "set", "--no-cone", *patterns(voice_states)])
    return True


def lighten(repo: Path, runner: hermes.Runner, voice_states: bool) -> Optional[str]:
    """An install's full clone made light in place: a partial clone of main only, sparse, shallow at its commit, and
    the objects it no longer needs dropped. None when it isn't an install's checkout (left as it is)."""
    if not runner.dry and not is_install_clone(repo):
        return None
    before = 0 if runner.dry else size(repo / ".git")
    head = git(repo, "rev-parse", "HEAD").stdout.strip()
    g = ["git", "-C", str(repo)]
    runner.run(g + ["config", "remote.origin.promisor", "true"])
    runner.run(g + ["config", "remote.origin.partialclonefilter", "blob:none"])
    runner.run(g + ["config", "remote.origin.fetch", "+refs/heads/main:refs/remotes/origin/main"])
    runner.run(g + ["sparse-checkout", "set", "--no-cone", *patterns(voice_states)])
    # Shallow at the commit checked out (the update has just brought it), other branches and tags forgotten.
    runner.run(g + ["fetch", "--quiet", "--depth", "1", "--filter=blob:none", "origin", head or "main"], check=False)
    for ref in ([] if runner.dry else git(repo, "for-each-ref", "--format=%(refname)", "refs/remotes", "refs/tags")
                .stdout.split()):
        if ref not in ("refs/remotes/origin/main", "refs/remotes/origin/HEAD"):
            runner.run(g + ["update-ref", "-d", ref], check=False)
    runner.run(g + ["reflog", "expire", "--expire=now", "--all"], check=False)
    # Only the contents of the files checked out stay (the others come back from GitHub if ever asked for).
    repacked = runner.run(g + ["repack", "-a", "-d", "-q", "--filter=blob:none"], check=False) if supports_filter(
        runner) else None
    if repacked is None:
        runner.run(g + ["gc", "--quiet", "--prune=now"], check=False)
    else:
        runner.run(g + ["prune", "--expire=now"], check=False)
    after = 0 if runner.dry else size(repo / ".git")
    return f"{repo} is now a light copy ({before // 2**20} MB of history -> {after // 2**20} MB)"


def supports_filter(runner: hermes.Runner) -> bool:
    """`git repack --filter` (git 2.43, Ubuntu 24.04's)."""
    if runner.dry:
        return True
    out = subprocess.run(["git", "repack", "-h"], capture_output=True, text=True)
    return "--filter" in out.stdout + out.stderr


def size(path: Path) -> int:
    total = 0
    for item in path.rglob("*"):
        try:
            if item.is_file() and not item.is_symlink():
                total += item.stat().st_size
        except OSError:
            pass
    return total
