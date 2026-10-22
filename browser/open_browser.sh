#!/bin/sh
# The person's « Ouvrir le navigateur » in the app (`podman exec -d <container> /opt/stealth/open_browser.sh`, from the
# bridge): Chrome started on this screen with the agent's profile, on the DevTools port Hermes and the MCP server know
# (STEALTH_CDP_PORT), when none runs yet. The MCP server's spawn_browser then connects to it rather than launching a
# second one (stealth.patch). Nothing happens while a Chrome already answers.
set -eu

port="${STEALTH_CDP_PORT:-9222}"
answers() {
    python3.11 -I -c "import sys, urllib.request; urllib.request.urlopen('http://127.0.0.1:$port/json/version', timeout=2)" \
        >/dev/null 2>&1
}
answers && exit 0

chrome=""
for candidate in google-chrome-stable google-chrome chromium-browser chromium; do
    if command -v "$candidate" >/dev/null 2>&1; then chrome="$candidate"; break; fi
done
[ -n "$chrome" ] || { echo "no Chrome in this image" >&2; exit 1; }

# A Chrome stopped without closing (killed, crashed) leaves its locks: none runs now.
if ! pgrep -u "$(id -u)" -f -- "--user-data-dir=${STEALTH_USER_DATA_DIR:-/home/bip/profile}" >/dev/null 2>&1; then
    rm -f "${STEALTH_USER_DATA_DIR:-/home/bip/profile}/SingletonLock" \
        "${STEALTH_USER_DATA_DIR:-/home/bip/profile}/SingletonSocket" \
        "${STEALTH_USER_DATA_DIR:-/home/bip/profile}/SingletonCookie"
fi

# The flags the MCP server launches it with (STEALTH_BROWSER_ARGS, « ; »-separated), its profile and its port.
set -f
old_ifs=$IFS
IFS=';'
set -- ${STEALTH_BROWSER_ARGS:-}
IFS=$old_ifs
set +f
exec "$chrome" "$@" --no-sandbox --remote-allow-origins=* "--remote-debugging-port=$port" \
    "--user-data-dir=${STEALTH_USER_DATA_DIR:-/home/bip/profile}" >/tmp/open-browser.log 2>&1
