#!/usr/bin/env bash
# BipAgents — install for the person running it, in their own account (no sudo):
#   curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash
# Options are passed on to the installer: --dry-run, --ports 9200, --languages fr,en
# Later: `bash ~/BipAgents/install.sh update` brings it up to date (and offers to do it each night).
# Channels: an install follows `stable` (what was promoted after the pre-test) where the repository has it, else `main`;
# `bipagents update --channel pre-test` (or BIPAGENTS_CHANNEL=pre-test at install) follows each publication first, and
# moves the install to the public repository that has that channel (the BipAgents organisation's); `--channel stable`
# back. Remembered in the checkout (git config bipagents.channel).
# An install exported on another machine (`bipagents export --moving`), moved here: `… | bash -s -- import <file>`.
# One's agents (or a Hermes alone) sent to a place of the all-included offer: `… | bash -s -- send <address> <code>`.
set -euo pipefail

REPO_URL="${BIPAGENTS_REPO:-https://github.com/Alexbob0/BipAgents.git}"
DIR="${BIPAGENTS_DIR:-$HOME/BipAgents}"
# Every public repository an install may follow or be moved to (installer/bipinstall/public_repo.py: KNOWN), newest last.
KNOWN_REPOS="https://github.com/Alexbob0/BipAgents.git https://github.com/BipAgents/BipAgents.git"  # known-public-repos

# --channel <stable|pre-test|main> is this script's (taken out of what goes on to the installer).
channel="${BIPAGENTS_CHANNEL:-}"
args=()
while [ $# -gt 0 ]; do
  case "$1" in
    --channel) channel="${2:-}"; shift; [ $# -gt 0 ] && shift ;;
    --channel=*) channel="${1#--channel=}"; shift ;;
    *) args+=("$1"); shift ;;
  esac
done
set -- ${args[@]+"${args[@]}"}
case "$channel" in
  ""|stable|pre-test|main) ;;
  *) echo "BipAgents: --channel stable, pre-test or main" >&2; exit 2 ;;
esac
# `update --check` only says what is new: nothing here moves the checkout (no pull, no other channel or repository;
# review R8), the installer fetches and compares
check_only=""
if [ "${1:-}" = update ]; then
  for arg in "$@"; do [ "$arg" = --check ] && check_only=1; done
fi
if [ -n "$check_only" ] && [ -n "$channel" ]; then
  echo "BipAgents: --check says what is new on the branch followed now; --channel takes effect at the update" >&2
  channel=""
fi
known() { case " $KNOWN_REPOS " in *" ${1%.git}.git "*) return 0 ;; esac; return 1; }
# BIPAGENTS_TO_REPO=<address>: the public repository BIPAGENTS_TO's commit lives in (the guichet's update orders name
# it: the switch to the organisation's repository, and its way back); one of KNOWN_REPOS only.
if [ -n "${BIPAGENTS_TO_REPO:-}" ]; then
  known "$BIPAGENTS_TO_REPO" || { echo "BipAgents: $BIPAGENTS_TO_REPO is not one of BipAgents' public repositories" >&2; exit 2; }
  REPO_URL="${BIPAGENTS_TO_REPO%.git}.git"
fi

need() { command -v "$1" >/dev/null 2>&1 || { echo "BipAgents needs $1. $2" >&2; exit 1; }; }
need git "macOS: xcode-select --install · Debian/Ubuntu: sudo apt install git · Fedora: sudo dnf install git"
need curl ""

# What only serves the install (downloaded packages) never stays on disk: uv and pip use a temporary cache and remove
# it. An update downloads again what it needs.
export UV_NO_CACHE=1 PIP_NO_CACHE_DIR=1

if ! command -v uv >/dev/null 2>&1; then
  echo "Installing uv (Python manager, in ~/.local/bin)…"
  curl -LsSf https://astral.sh/uv/install.sh | sh
  export PATH="$HOME/.local/bin:$PATH"
fi

# BIPAGENTS_TO=<commit> (its 40 characters): that version rather than the latest, ahead or back (a server of the
# all-included offer, by the guichet's waves: guichet/bipguichet/waves.py).
to_version() {
  if [ -n "${BIPAGENTS_TO_REPO:-}" ] && install_clone && [ "$(origin)" != "$REPO_URL" ]; then
    git -C "$DIR" remote set-url origin "$REPO_URL"   # the commit lives there (its way back: the one before)
  fi
  git -C "$DIR" fetch --depth 1 --quiet origin "$BIPAGENTS_TO" && git -C "$DIR" checkout --quiet -B main FETCH_HEAD
}

origin() { git -C "$DIR" config --get remote.origin.url 2>/dev/null | sed 's|\.git$||; s|$|.git|'; }
# A checkout of one of the public repositories (a developer's clone of the private one is never moved).
install_clone() { known "$(origin)"; }
has_branch() {   # <repository or remote> <branch>
  if [ -d "$DIR/.git" ]; then git -C "$DIR" ls-remote --exit-code --heads "$1" "$2"
  else git ls-remote --exit-code --heads "$1" "$2"; fi >/dev/null 2>&1
}

# The branch this checkout follows: the channel asked for (remembered), else the one remembered, else stable; main on
# a repository that has no such branch. A channel asked for that the origin lacks moves the install to the newest
# public repository that has it (the switch to the organisation's repository: `bipagents update --channel pre-test`).
pick_channel() {
  local wanted="${channel:-$(git -C "$DIR" config bipagents.channel || true)}" repo
  wanted="${wanted:-stable}"
  [ "$wanted" = main ] && { echo main; return; }
  has_branch origin "$wanted" && { echo "$wanted"; return; }
  if [ -n "$channel" ]; then
    for repo in $(echo "$KNOWN_REPOS" | tr ' ' '\n' | sed -n '1!G;h;$p'); do
      if [ "$repo" != "$(origin)" ] && has_branch "$repo" "$wanted"; then
        echo "BipAgents now comes from $repo ($wanted)." >&2
        git -C "$DIR" remote set-url origin "$repo"
        echo "$wanted"; return
      fi
    done
    echo "BipAgents: no $wanted channel on $(origin) yet: main as before." >&2
  fi
  echo main
}

# This checkout follows branch $1 of its origin: `git pull` brings it (a light clone fetches that one branch).
follow() {
  if ! git -C "$DIR" config --get-all remote.origin.fetch | grep -q 'refs/heads/\*'; then
    git -C "$DIR" config remote.origin.fetch "+refs/heads/$1:refs/remotes/origin/$1"
  fi
  git -C "$DIR" config branch.main.remote origin
  git -C "$DIR" config branch.main.merge "refs/heads/$1"
}

# The published history rewritten (it was once, when the guichet left this repository; and the switch to the
# organisation's, whose history starts at 0.1): the latest version of the branch followed is taken as it is, but only
# when nothing in this copy was changed by hand (a change is never overwritten).
resync() {
  git -C "$DIR" diff --quiet HEAD -- 2>/dev/null || return 1
  local branch
  branch="$(git -C "$DIR" config branch.main.merge || echo refs/heads/main)"
  git -C "$DIR" fetch --depth 1 --quiet origin "${branch#refs/heads/}" && git -C "$DIR" checkout --quiet -B main FETCH_HEAD
}

# What this checkout last took from its origin (an install's own commits are never rolled back: below).
remember() { git -C "$DIR" config bipagents.pulled "$(git -C "$DIR" rev-parse HEAD)"; }

if [ -n "$check_only" ]; then
  [ -d "$DIR/.git" ] || { echo "BipAgents isn't installed in $DIR" >&2; exit 1; }
  # the installer's --check fetches and compares, nothing moves
elif [ -d "$DIR/.git" ]; then
  # The version before, for `install.sh update` (what changed); a pull that cannot go on is explained by it.
  export BIPAGENTS_BEFORE="$(git -C "$DIR" rev-parse HEAD)"
  if [ -n "${BIPAGENTS_TO:-}" ]; then
    if ! to_version; then
      [ "${1:-}" = "update" ] || exit 1
      export BIPAGENTS_PULL_FAILED=1
    else
      remember
    fi
  else
    moved=""
    if install_clone; then
      before_follow="$(origin) $(git -C "$DIR" config branch.main.merge || true)"
      followed="$(pick_channel)"
      follow "$followed"
      [ -n "$channel" ] && git -C "$DIR" config bipagents.channel "$channel"
      # another branch or repository than before: its latest taken as it is (stable may be behind pre-test)
      [ "$before_follow" = "$(origin) refs/heads/$followed" ] || moved=1
    fi
    if [ -n "$moved" ] && resync; then
      remember
    elif ! pulled="$(git -C "$DIR" pull --ff-only --quiet 2>&1)" && ! resync; then
      [ -n "$pulled" ] && echo "$pulled" >&2
      [ "${1:-}" = "update" ] || exit 1
      export BIPAGENTS_PULL_FAILED=1
    else
      # The branch followed moved back (a promoted version gone wrong: publish-public.sh --stable-back): `pull`
      # leaves this checkout ahead of it. When HEAD is what it last took from its origin (no commit of its own) and
      # nothing was changed by hand, it goes back with it (review R7).
      if install_clone && [ "$BIPAGENTS_BEFORE" = "$(git -C "$DIR" rev-parse HEAD)" ] \
         && [ "$(git -C "$DIR" config bipagents.pulled || true)" = "$BIPAGENTS_BEFORE" ] \
         && git -C "$DIR" diff --quiet HEAD -- 2>/dev/null; then
        upstream="$(git -C "$DIR" rev-parse --verify -q "refs/remotes/origin/${followed:-main}" || true)"
        if [ -n "$upstream" ] && [ "$upstream" != "$BIPAGENTS_BEFORE" ] \
           && git -C "$DIR" merge-base --is-ancestor "$upstream" HEAD 2>/dev/null; then
          echo "BipAgents: ${followed:-main} went back to ${upstream:0:12}: this install goes back with it." >&2
          git -C "$DIR" checkout --quiet -B main "$upstream"
        fi
      fi
      remember
    fi
  fi
else
  # A light copy (installer/bipinstall/checkout.py): the latest commit, the contents of the files checked out only,
  # without the apps' sources, the site, the voices' recordings and Pocket's voice states (the installer adds them
  # when this account runs Pocket's own models).
  echo "Downloading BipAgents into ${DIR}…"
  followed="${channel:-stable}"
  if [ "$followed" != main ] && ! has_branch "$REPO_URL" "$followed"; then followed=main; fi
  git clone --depth 1 --filter=blob:none --no-checkout --quiet -b "$followed" "$REPO_URL" "$DIR"
  git -C "$DIR" sparse-checkout set --no-cone '/*' '!/App/' '!/android/' '!/site/' '!/voices/**/*.wav' \
    '!/voices/**/*.safetensors'
  git -C "$DIR" checkout --quiet -B main "origin/$followed"
  [ "$followed" = main ] || git -C "$DIR" branch --quiet -D "$followed"
  follow "$followed"
  [ -n "$channel" ] && git -C "$DIR" config bipagents.channel "$channel"
  if [ -n "${BIPAGENTS_TO:-}" ]; then to_version; fi
  remember
fi

cd "$DIR"
# The installer's own environment (installer/requirements.txt), the one its timers and the bridge run it with too.
VENV="$DIR/installer/.venv"
[ -x "$VENV/bin/python" ] || uv venv --quiet --python 3.12 "$VENV"
uv pip install --quiet --python "$VENV/bin/python" -r "$DIR/installer/requirements.txt"
# Questions are asked on the terminal even when this script comes through a pipe (curl … | bash).
run() { env PYTHONPATH="$DIR/installer" "$VENV/bin/python" -m bipinstall "$@"; }
# A new install (no subcommand): when the bridge of this version isn't out yet (GitHub Actions builds it after each
# change, or its build failed), the version before whose bridge is out (installer/bipinstall/rustbridge.py
# pick_released). An update keeps its current bridge instead (rustbridge.keep_in_step). A server of the offer
# (--cloud) installed just after a public push waits for its version's bridge first (up to 35 min): the server's
# shared Hermes was made from that same version.
case "${1:-}" in
  ""|-*)
    picked="$(git -C "$DIR" rev-parse HEAD)"
    bridge_wait=""
    for arg in "$@"; do case "$arg" in --cloud) bridge_wait="--wait" ;; esac; done
    run bridge released-version $bridge_wait || true
    if [ "$(git -C "$DIR" rev-parse HEAD)" != "$picked" ]; then
      uv pip install --quiet --python "$VENV/bin/python" -r "$DIR/installer/requirements.txt"
    fi
    ;;
esac
if [ ! -t 0 ] && (exec </dev/tty) 2>/dev/null; then
  run "$@" </dev/tty
else
  run "$@"
fi
