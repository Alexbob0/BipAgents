"""Chrome's DevTools, reachable from the host at an address that outlives Chrome.

Chrome only listens on this container's loopback (9222), and its browser endpoint (`/devtools/browser/<id>`) changes
each time it starts: the MCP server starts it on demand and closes it when idle. Hermes keeps the endpoint it resolved
(its vault fills go through it), so this relay listens on 9223, the port published on the host's 127.0.0.1, and sends
any browser endpoint to the current one. Everything else passes through unchanged.

While the person holds the browser in the app (hold_gate.py's flag), nothing gets through, and the connections open
when they took it are closed: Hermes (the vault's fills) waits too."""
from __future__ import annotations

import asyncio
import json
import os
import re
import sys
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hold_gate import held  # noqa: E402

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


OPEN: set = set()   # the writers of the connections passing through, closed when the person takes over


async def relay(client_reader: asyncio.StreamReader, client_writer: asyncio.StreamWriter) -> None:
    if held():
        client_writer.close()
        return
    try:
        first = await client_reader.readuntil(b"\r\n")
        if BROWSER_ENDPOINT.match(first):
            first = BROWSER_ENDPOINT.sub(await asyncio.to_thread(current_endpoint), first, count=1)
        chrome_reader, chrome_writer = await asyncio.open_connection(*CHROME)
    except (OSError, ValueError, KeyError, asyncio.IncompleteReadError, asyncio.LimitOverrunError):
        client_writer.close()   # no Chrome running yet: the caller retries
        return
    chrome_writer.write(first)
    pair = {client_writer, chrome_writer}
    OPEN.update(pair)
    try:
        await asyncio.gather(pipe(client_reader, chrome_writer), pipe(chrome_reader, client_writer))
    finally:
        OPEN.difference_update(pair)


async def watch_hold() -> None:
    while True:
        await asyncio.sleep(0.5)
        if held() and OPEN:
            for writer in list(OPEN):
                writer.close()


async def main() -> None:
    server = await asyncio.start_server(relay, "0.0.0.0", 9223)
    asyncio.create_task(watch_hold())
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main())
