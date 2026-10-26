#!/usr/bin/env bash
# BipAgents — install for the person running it, in their own account (no sudo):
#   curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash
# Options are passed on to the installer: --dry-run, --relay-url URL --relay-key KEY, --ports 9200, --languages fr,en
# Later: `bash ~/BipAgents/install.sh update` brings it up to date (and offers to do it each night).
# An install exported on another machine (`bipagents export --moving`), moved here: `… | bash -s -- import <file>`.
# One's agents (or a Hermes alone) sent to a place of the all-included offer: `… | bash -s -- send <address> <code>`.
set -euo pipefail

REPO_URL="${BIPAGENTS_REPO:-https://github.com/Alexbob0/BipAgents.git}"
DIR="${BIPAGENTS_DIR:-$HOME/BipAgents}"

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
  git -C "$DIR" fetch --depth 1 --quiet origin "$BIPAGENTS_TO" && git -C "$DIR" checkout --quiet -B main FETCH_HEAD
}

# The published history rewritten (it was once, when the guichet left this repository): the latest version is taken
# as it is, but only when nothing in this copy was changed by hand (a change is never overwritten).
resync() {
  git -C "$DIR" diff --quiet HEAD -- 2>/dev/null || return 1
  git -C "$DIR" fetch --depth 1 --quiet origin main && git -C "$DIR" checkout --quiet -B main FETCH_HEAD
}

if [ -d "$DIR/.git" ]; then
  # The version before, for `install.sh update` (what changed); a pull that cannot go on is explained by it.
  export BIPAGENTS_BEFORE="$(git -C "$DIR" rev-parse HEAD)"
  if [ -n "${BIPAGENTS_TO:-}" ]; then
    if ! to_version; then
      [ "${1:-}" = "update" ] || exit 1
      export BIPAGENTS_PULL_FAILED=1
    fi
  elif ! pulled="$(git -C "$DIR" pull --ff-only --quiet 2>&1)" && ! resync; then
    [ -n "$pulled" ] && echo "$pulled" >&2
    [ "${1:-}" = "update" ] || exit 1
    export BIPAGENTS_PULL_FAILED=1
  fi
else
  # A light copy (installer/bipinstall/checkout.py): the latest commit, the contents of the files checked out only,
  # without the apps' sources, the site, the voices' recordings and Pocket's voice states (the installer adds them
  # when this account runs Pocket's own models).
  echo "Downloading BipAgents into $DIR…"
  git clone --depth 1 --filter=blob:none --no-checkout --quiet "$REPO_URL" "$DIR"
  git -C "$DIR" sparse-checkout set --no-cone '/*' '!/App/' '!/android/' '!/site/' '!/voices/**/*.wav' \
    '!/voices/**/*.safetensors'
  git -C "$DIR" checkout --quiet main
  if [ -n "${BIPAGENTS_TO:-}" ]; then to_version; fi
fi

cd "$DIR"
# The installer's own environment (installer/requirements.txt), the one its timers and the bridge run it with too.
VENV="$DIR/installer/.venv"
[ -x "$VENV/bin/python" ] || uv venv --quiet --python 3.12 "$VENV"
uv pip install --quiet --python "$VENV/bin/python" -r "$DIR/installer/requirements.txt"
# Questions are asked on the terminal even when this script comes through a pipe (curl … | bash).
run() { env PYTHONPATH="$DIR/installer" "$VENV/bin/python" -m bipinstall "$@"; }
if [ ! -t 0 ] && (exec </dev/tty) 2>/dev/null; then
  run "$@" </dev/tty
else
  run "$@"
fi
