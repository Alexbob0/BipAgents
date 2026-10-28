"""Hermes' own MCP code (tools/mcp_tool*: MCPServerTask, its message_handler and _refresh_tools) against the bridge:
prints LISTED once the bipagents tools are registered, then RELISTED with the calendar tools once Hermes' registry
has them (after the bridge's tools/list_changed). Same output contract as bbkit/mcp_sdk_client.py. Run by
test_tools_changed_sdk.py with MCP_SDK_CLIENT=<this file>, HERMES_SRC=<a Hermes checkout>, HERMES_HOME=<an empty
folder> and MCP_SDK_PYTHON=<a Python with that Hermes' dependencies>."""
import logging
import os
import sys
import time

sys.path.insert(0, os.environ["HERMES_SRC"])
if os.environ.get("SITE_FIRST"):
    sys.path.insert(0, os.environ["SITE_FIRST"])  # another MCP SDK (e.g. 2.0.0, Hermes 517b5e10f's pin) first
url, token = sys.argv[1], sys.argv[2]
logging.basicConfig(level=logging.INFO, stream=sys.stdout, format="LOG %(name)s %(message)s")

from importlib.metadata import version  # noqa: E402

print("SDK", version("mcp"), flush=True)
from tools.mcp_tool_discovery import register_mcp_servers  # noqa: E402
from tools.registry import registry  # noqa: E402
from tools import mcp_tool as core  # noqa: E402


def names():
    return sorted(n for n in registry.get_all_tool_names() if n.startswith("mcp__bipagents__"))


registered = register_mcp_servers({"bipagents": {"url": url, "headers": {"Authorization": f"Bearer {token}"},
                                                 "timeout": 60, "connect_timeout": 30}})
server = core._servers.get("bipagents") if hasattr(core, "_servers") else None
caps = getattr(getattr(server, "initialize_result", None), "capabilities", None)
print("LISTCHANGED", getattr(getattr(caps, "tools", None), "listChanged", None) or
      getattr(getattr(caps, "tools", None), "list_changed", None), flush=True)
print("LISTED", len(names()), flush=True)
end = time.monotonic() + 30
while time.monotonic() < end:
    calendar = [n.removeprefix("mcp__bipagents__") for n in names() if n.startswith("mcp__bipagents__calendar_")]
    if calendar:
        print("RELISTED", ",".join(calendar), flush=True)
        break
    time.sleep(0.2)
else:
    print("TIMEOUT", names(), flush=True)
os._exit(0)
