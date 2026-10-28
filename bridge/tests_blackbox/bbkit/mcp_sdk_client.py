"""The real MCP Python SDK client (mcp==2.0.0, Hermes 517b5e10f's pin) against the bridge: prints LISTED <n>, waits
for tools/list_changed (the test changes the tools after LISTED), re-lists, prints the new names."""
import sys
from importlib.metadata import version

import anyio
import httpx2
from mcp import ClientSession, types
from mcp.client.streamable_http import streamable_http_client

url, token = sys.argv[1], sys.argv[2]


async def main():
    changed = anyio.Event()

    async def handler(message):
        if isinstance(message, Exception):
            print("EXC", repr(message), flush=True)
            return
        root = getattr(message, "root", message)
        print("MESSAGE", type(root).__name__, flush=True)
        if isinstance(root, types.ToolListChangedNotification):
            changed.set()

    async with httpx2.AsyncClient(headers={"Authorization": f"Bearer {token}"},
                                  timeout=httpx2.Timeout(30, read=300)) as http:
        async with streamable_http_client(url, http_client=http) as streams:
            async with ClientSession(streams[0], streams[1], message_handler=handler) as session:
                init = await session.initialize()
                print("PROTOCOL", init.protocol_version, "LISTCHANGED", init.capabilities.tools.list_changed, flush=True)
                tools = await session.list_tools()
                print("LISTED", len(tools.tools), flush=True)
                with anyio.fail_after(30):
                    await changed.wait()
                tools = await session.list_tools()
                print("RELISTED", ",".join(t.name for t in tools.tools if t.name.startswith("calendar_")), flush=True)


print("SDK", version("mcp"), flush=True)
anyio.run(main)
