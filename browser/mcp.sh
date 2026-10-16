#!/bin/sh
# The MCP server, over stdio, as Hermes starts it: `podman exec -i bipagents-browser-<agent> /opt/stealth/mcp.sh`.
# Only what an agent needs to browse: no element cloning or extraction to files, no network interception, no raw CDP,
# no debugging hooks. Through hold_gate.py: while the person holds the browser in the app, its tools wait.
cd /opt/stealth/stealth-browser-mcp/src
exec python3.11 -I /opt/stealth/hold_gate.py /opt/stealth/venv/bin/python server.py \
    --disable-element-extraction --disable-file-extraction --disable-network-debugging --disable-cdp-functions \
    --disable-progressive-cloning --disable-dynamic-hooks --disable-debugging
