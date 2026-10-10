#!/usr/bin/env bash
# BipAgents — install for the person running it, in their own account (no sudo):
#   curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash
# Options are passed on to the installer: --dry-run, --relay-url URL --relay-key KEY, --ports 9200, --languages fr,en
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

if [ -d "$DIR/.git" ]; then
  git -C "$DIR" pull --ff-only --quiet
else
  echo "Downloading BipAgents into $DIR…"
  git clone --depth 1 --quiet "$REPO_URL" "$DIR"
fi

cd "$DIR"
# Questions are asked on the terminal even when this script comes through a pipe.
exec env PYTHONPATH="$DIR/installer" uv run --quiet --python 3.12 --with httpx --with pyyaml \
  python -m bipinstall "$@" </dev/tty
