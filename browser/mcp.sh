#!/bin/sh
# The MCP server, over stdio, as Hermes starts it: `podman exec -i bipagents-browser-<agent> /opt/stealth/mcp.sh`.
# Only what an agent needs to browse: no element cloning or extraction to files, no network interception, no raw CDP,
# no debugging hooks.
cd /opt/stealth/stealth-browser-mcp/src
exec /opt/stealth/venv/bin/python server.py \
    --disable-element-extraction --disable-file-extraction --disable-network-debugging --disable-cdp-functions \
    --disable-progressive-cloning --disable-dynamic-hooks --disable-debugging
