#!/bin/sh
# The container's Chrome, started apart from the MCP server so that it outlives it (Hermes restarts its MCP servers:
# a gateway restart, a reconnect, a `hermes chat` that ends; a Chrome started as the server's child died with it, and
# the agent's page with it). Two callers:
#
# - the person's « Ouvrir le navigateur » in the app (`podman exec -d <container> /opt/stealth/open_browser.sh`, from
#   the bridge), with no arguments: Chrome with the agent's profile, on the DevTools port Hermes and the MCP server
#   know (STEALTH_CDP_PORT), and the flags the MCP server would give it;
# - the MCP server's spawn_browser (stealth.patch), with nodriver's own command line as arguments, detached: the
#   container's first process adopts this Chrome, and the server only connects to it.
#
# Nothing happens while a Chrome already answers.
set -eu

port="${STEALTH_CDP_PORT:-9222}"
profile="${STEALTH_USER_DATA_DIR:-/home/bip/profile}"
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
if ! pgrep -u "$(id -u)" -f -- "--user-data-dir=$profile" >/dev/null 2>&1; then
    rm -f "$profile/SingletonLock" "$profile/SingletonSocket" "$profile/SingletonCookie"
fi

if [ $# -eq 0 ]; then
    # nodriver's defaults (what spawn_browser launches with) but --remote-allow-origins=* (Chrome then refuses a
    # DevTools WebSocket from a web page: those carry an Origin; nodriver's and the relay's carry none), then the
    # container's flags (STEALTH_BROWSER_ARGS, « ; »-separated), its profile and its port.
    set -f
    old_ifs=$IFS
    IFS=';'
    set -- ${STEALTH_BROWSER_ARGS:-}
    IFS=$old_ifs
    set +f
    set -- --no-first-run --no-service-autorun --no-default-browser-check \
        --homepage=about:blank --no-pings --password-store=basic --disable-infobars --disable-breakpad \
        --disable-dev-shm-usage --disable-session-crashed-bubble --disable-search-engine-choice-screen \
        --disable-features=IsolateOrigins,site-per-process "$@" --no-sandbox \
        "--remote-debugging-host=127.0.0.1" "--remote-debugging-port=$port" "--user-data-dir=$profile"
fi

# Chrome reads only its last --disable-features: every one given (nodriver's own, the container's in
# STEALTH_BROWSER_ARGS, where the bridge puts its own) merged into one, last, with WebUIOmniboxPopup and
# WebUIOmniboxAimPopup. Chrome 157 lists its address bar's suggestion popup (chrome://omnibox-popup.top-chrome/) as a
# page with no window of its own, which the agent's server took for its tab once the first tab was closed: every
# call then failed (13/11). Fedora's Chromium 156 (the ARM image) has a second one (…/omnibox_popup_aim.html).
features=""
add_features() {   # a comma-separated list: each feature once, in order
    old_ifs=$IFS
    IFS=,
    set -f
    for feature in $1; do
        case ",$features," in
            *",$feature,"*) ;;
            *) [ -z "$feature" ] || features="${features:+$features,}$feature" ;;
        esac
    done
    IFS=$old_ifs
    set +f
}
for arg do
    shift
    case $arg in
        --disable-features=*) add_features "${arg#--disable-features=}" ;;
        *) set -- "$@" "$arg" ;;
    esac
done
add_features WebUIOmniboxPopup,WebUIOmniboxAimPopup
set -- "$@" "--disable-features=$features"
exec "$chrome" "$@" >/tmp/open-browser.log 2>&1
