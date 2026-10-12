#!/usr/bin/env bash
# BipAgents — install for the person running it, in their own account (no sudo):
#   curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash
# Options are passed on to the installer: --dry-run, --relay-url URL --relay-key KEY, --ports 9200, --languages fr,en,
# --lang fr|en (the installer's language; default: the system's)
set -euo pipefail

# French or English, like the installer: --lang, else BIPAGENTS_LANG, else the system's locale.
LANG_CODE=""
for value in "${BIPAGENTS_LANG:-}" "${LC_ALL:-}" "${LC_MESSAGES:-}" "${LANG:-}"; do
  case "$value" in ""|C|POSIX|C.UTF-8) ;; *) LANG_CODE="$value"; break ;; esac
done
prev=""
for arg in "$@"; do
  case "$arg" in --lang=*) LANG_CODE="${arg#--lang=}" ;; esac
  [ "$prev" = "--lang" ] && LANG_CODE="$arg"
  prev="$arg"
done
case "$LANG_CODE" in fr*|FR*) FR=1 ;; *) FR=0 ;; esac
say() { if [ "$FR" = 1 ]; then echo "$1"; else echo "$2"; fi; }

REPO_URL="${BIPAGENTS_REPO:-https://github.com/Alexbob0/BipAgents.git}"
DIR="${BIPAGENTS_DIR:-$HOME/BipAgents}"

need() { command -v "$1" >/dev/null 2>&1 || { say "BipAgents a besoin de $1. $2" "BipAgents needs $1. $2" >&2; exit 1; }; }
need git "macOS: xcode-select --install · Debian/Ubuntu: sudo apt install git · Fedora: sudo dnf install git"
need curl ""

if ! command -v uv >/dev/null 2>&1; then
  say "J'installe uv (gestionnaire Python, dans ~/.local/bin)…" "Installing uv (Python manager, in ~/.local/bin)…"
  curl -LsSf https://astral.sh/uv/install.sh | sh
  export PATH="$HOME/.local/bin:$PATH"
fi

if [ -d "$DIR/.git" ]; then
  git -C "$DIR" pull --ff-only --quiet
else
  say "Je télécharge BipAgents dans $DIR…" "Downloading BipAgents into $DIR…"
  git clone --depth 1 --quiet "$REPO_URL" "$DIR"
fi

cd "$DIR"
# Questions are asked on the terminal even when this script comes through a pipe (curl … | bash).
run() { env PYTHONPATH="$DIR/installer" uv run --quiet --python 3.12 --with httpx --with pyyaml python -m bipinstall "$@"; }
if [ ! -t 0 ] && (exec </dev/tty) 2>/dev/null; then
  run "$@" </dev/tty
else
  run "$@"
fi
