"""Chrome's DevTools, reachable from the host at an address that outlives Chrome.

Chrome only listens on this container's loopback (9222), and its browser endpoint (`/devtools/browser/<id>`) changes
each time it starts: the MCP server starts it on demand and closes it when idle. Hermes keeps the endpoint it resolved
(its vault fills go through it), so this relay listens on 9223, the port published on the host's 127.0.0.1, and sends
any browser endpoint to the current one. Everything else passes through unchanged."""
from __future__ import annotations

import asyncio
import json
import re
import urllib.request

CHROME = ("127.0.0.1", 9222)
BROWSER_ENDPOINT = re.compile(rb"^GET /devtools/browser/[^ ?]*")


def current_endpoint() -> bytes:
    with urllib.request.urlopen(f"http://{CHROME[0]}:{CHROME[1]}/json/version", timeout=3) as response:
        url = json.load(response)["webSocketDebuggerUrl"]
    return b"GET /devtools/browser/" + url.rsplit("/devtools/browser/", 1)[1].encode()


async def pipe(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    try:
        while data := await reader.read(65536):
            writer.write(data)
            await writer.drain()
    except (ConnectionError, OSError):
        pass
    finally:
        writer.close()


async def relay(client_reader: asyncio.StreamReader, client_writer: asyncio.StreamWriter) -> None:
    try:
        first = await client_reader.readuntil(b"\r\n")
        if BROWSER_ENDPOINT.match(first):
            first = BROWSER_ENDPOINT.sub(await asyncio.to_thread(current_endpoint), first, count=1)
        chrome_reader, chrome_writer = await asyncio.open_connection(*CHROME)
    except (OSError, ValueError, KeyError, asyncio.IncompleteReadError, asyncio.LimitOverrunError):
        client_writer.close()   # no Chrome running yet: the caller retries
        return
    chrome_writer.write(first)
    await asyncio.gather(pipe(client_reader, chrome_writer), pipe(chrome_reader, client_writer))


async def main() -> None:
    server = await asyncio.start_server(relay, "0.0.0.0", 9223)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main())
