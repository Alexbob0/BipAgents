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

if [ -d "$DIR/.git" ]; then
  # The version before, for `install.sh update` (what changed); a pull that cannot go on is explained by it.
  export BIPAGENTS_BEFORE="$(git -C "$DIR" rev-parse HEAD)"
  if [ -n "${BIPAGENTS_TO:-}" ]; then
    if ! to_version; then
      [ "${1:-}" = "update" ] || exit 1
      export BIPAGENTS_PULL_FAILED=1
    fi
  elif ! git -C "$DIR" pull --ff-only --quiet; then
    [ "${1:-}" = "update" ] || exit 1
    export BIPAGENTS_PULL_FAILED=1
  fi
else
  echo "Downloading BipAgents into $DIR…"
  git clone --depth 1 --quiet "$REPO_URL" "$DIR"
  if [ -n "${BIPAGENTS_TO:-}" ]; then to_version; fi
fi

cd "$DIR"
# Questions are asked on the terminal even when this script comes through a pipe (curl … | bash).
run() { env PYTHONPATH="$DIR/installer" uv run --quiet --python 3.12 --with httpx --with pyyaml --with cryptography python -m bipinstall "$@"; }
if [ ! -t 0 ] && (exec </dev/tty) 2>/dev/null; then
  run "$@" </dev/tty
else
  run "$@"
fi
