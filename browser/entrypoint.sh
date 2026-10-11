#!/bin/sh
# The agent's screen: Xvfb, a window manager with its panel (a desktop that looks like one), the VNC server the bridge
# logs in to (password in /run/bipagents/vnc.pass, mounted read-only), and the relay through which the host reaches
# Chrome's DevTools (devtools_relay.py: Chrome only listens on this container's loopback). Then it waits: Hermes
# starts the MCP server (`podman exec … /opt/stealth/mcp.sh`), which starts Chrome on this screen when needed.
set -eu

Xvfb :99 -screen 0 1920x1080x24 -nolisten tcp >/tmp/xvfb.log 2>&1 &
screen=$!
i=0
while [ ! -e /tmp/.X11-unix/X99 ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done

openbox >/tmp/openbox.log 2>&1 &
lxpanel >/tmp/lxpanel.log 2>&1 &

if [ -s /run/bipagents/vnc.pass ]; then
    x11vnc -display :99 -passwdfile /run/bipagents/vnc.pass -rfbport 5900 -forever -shared -noxdamage -quiet \
        -bg -o /tmp/x11vnc.log
else
    echo "no VNC password: the desktop is not shared" >&2
fi

python3.11 -I /opt/stealth/devtools_relay.py >/tmp/devtools-relay.log 2>&1 &

# A Chrome stopped with its container leaves its locks in the persistent profile: none runs yet.
rm -f /home/bip/profile/SingletonLock /home/bip/profile/SingletonSocket /home/bip/profile/SingletonCookie

trap 'kill -TERM "$screen" 2>/dev/null' TERM INT
wait "$screen"
