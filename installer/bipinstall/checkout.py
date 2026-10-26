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
    """An install's older clone made light: a light clone of the same commit made beside it, what git doesn't track
    moved over (the environments, the person's own files), then the two folders swapped and the old one removed.
    The paths stay the same, so the services find their environments where they were. Git can't drop the contents
    of a clone's history in place (`repack --filter` keeps them in another pack), hence the new clone. None when it
    isn't an install's checkout, is light already, or has files changed by hand (left as it is)."""
    import shutil
    if not runner.dry and (not is_install_clone(repo) or is_sparse(repo)):
        return None
    if not runner.dry and git(repo, "status", "--porcelain", "--untracked-files=no").stdout.strip():
        return f"{repo} kept as it is: files changed by hand"
    url = git(repo, "remote", "get-url", "origin").stdout.strip()
    head = git(repo, "rev-parse", "HEAD").stdout.strip()
    before = 0 if runner.dry else size(repo)
    new, old = repo.with_name(repo.name + ".light"), repo.with_name(repo.name + ".full")
    g = ["git", "-C", str(new)]
    runner.log.append(f"remove {new}")
    if not runner.dry:
        shutil.rmtree(new, ignore_errors=True)
    try:
        runner.run(["git", "clone", "--quiet", "--depth", "1", "--filter=blob:none", "--no-checkout", url, str(new)])
        runner.run(g + ["sparse-checkout", "set", "--no-cone", *patterns(voice_states)])
        runner.run(g + ["fetch", "--quiet", "--depth", "1", "origin", head or "main"])
        runner.run(g + ["checkout", "--quiet", "-B", "main", "FETCH_HEAD"])
        runner.run(g + ["branch", "--quiet", "--set-upstream-to", "origin/main", "main"], check=False)
    except RuntimeError as exc:
        if not runner.dry:
            shutil.rmtree(new, ignore_errors=True)
        return f"{repo} kept as it is for now ({str(exc)[-120:]})"
    # What git doesn't track (pocket/.venv, installer/.venv, bridge-rs/target, the person's own files), moved over.
    others = [] if runner.dry else [line[3:] for line in git(repo, "status", "--porcelain", "--ignored").stdout
                                   .splitlines() if line[:2] in ("??", "!!")]
    runner.log.append(f"move {len(others)} untracked item(s) of {repo} into {new}; swap the folders; remove {old}")
    if runner.dry:
        return f"{repo} would become a light copy"
    for path in others:
        source, target = repo / path.rstrip("/"), new / path.rstrip("/")
        if target.exists() or not (source.exists() or source.is_symlink()):
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        source.rename(target)
    shutil.rmtree(old, ignore_errors=True)
    repo.rename(old)
    new.rename(repo)
    shutil.rmtree(old, ignore_errors=True)
    return f"{repo} is now a light copy ({before // 2**20} MB -> {size(repo) // 2**20} MB with its environments)"


def size(path: Path) -> int:
    total = 0
    for item in path.rglob("*"):
        try:
            if item.is_file() and not item.is_symlink():
                total += item.stat().st_size
        except OSError:
            pass
    return total
