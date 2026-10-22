"""Chrome's DevTools, reachable from the host at an address that outlives Chrome.

Chrome only listens on this container's loopback (9222), and its browser endpoint (`/devtools/browser/<id>`) changes
each time it starts: the MCP server starts it on demand and closes it when idle. Hermes keeps the endpoint it resolved
(its vault fills go through it), so this relay listens on 9223, the port published on the host's 127.0.0.1, and sends
any browser endpoint to the current one. Everything else passes through unchanged.

While the person holds the browser in the app (hold_gate.py's flag), nothing gets through, and the connections open
when they took it are closed: Hermes (the vault's fills) waits too. While a card payment holds it (the flag reads
« card <token> »), only the bridge's own connections get through, those whose request shows that token
(`X-Bip-Card: <token>`): it types the card and clicks « Pay »; nothing else reads the page meanwhile."""
from __future__ import annotations

import asyncio
import hmac
import json
import os
import re
import sys
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hold_gate import HOLD, held  # noqa: E402

CHROME = ("127.0.0.1", 9222)
BROWSER_ENDPOINT = re.compile(rb"^GET /devtools/browser/[^ ?]*")
CARD_HEADER = re.compile(rb"\r\nx-bip-card:[ \t]*([A-Za-z0-9]+)[ \t]*\r\n", re.IGNORECASE)


def current_endpoint() -> bytes:
    with urllib.request.urlopen(f"http://{CHROME[0]}:{CHROME[1]}/json/version", timeout=3) as response:
        url = json.load(response)["webSocketDebuggerUrl"]
    return b"GET /devtools/browser/" + url.rsplit("/devtools/browser/", 1)[1].encode()


def payment_token() -> bytes | None:
    """The token of the card payment holding the browser, if one does."""
    try:
        with open(HOLD, "rb") as flag:
            text = flag.read(200)
    except OSError:
        return None
    return text[5:].strip() or None if text.startswith(b"card ") else None


def let_through(head: bytes) -> bool:
    """Not held, or held by a card payment and this is the bridge's own connection."""
    if not held():
        return True
    token, shown = payment_token(), CARD_HEADER.search(head)
    return bool(token and shown and hmac.compare_digest(token, shown.group(1)))


async def pipe(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    try:
        while data := await reader.read(65536):
            writer.write(data)
            await writer.drain()
    except (ConnectionError, OSError):
        pass
    finally:
        writer.close()


OPEN: set = set()     # the writers of the connections passing through, closed when the person takes over
BRIDGE: set = set()   # those of the bridge's payment connections, left open by a payment's hold


async def relay(client_reader: asyncio.StreamReader, client_writer: asyncio.StreamWriter) -> None:
    if held() and payment_token() is None:
        client_writer.close()
        return
    try:
        head = await client_reader.readuntil(b"\r\n\r\n")
        if not let_through(head):
            client_writer.close()
            return
        first, _, rest = head.partition(b"\r\n")
        if BROWSER_ENDPOINT.match(first):
            first = BROWSER_ENDPOINT.sub(await asyncio.to_thread(current_endpoint), first, count=1)
        chrome_reader, chrome_writer = await asyncio.open_connection(*CHROME)
    except (OSError, ValueError, KeyError, asyncio.IncompleteReadError, asyncio.LimitOverrunError):
        client_writer.close()   # no Chrome running yet: the caller retries
        return
    chrome_writer.write(first + b"\r\n" + rest)
    pair = {client_writer, chrome_writer}
    bridge = held()   # let through while held: the payment's own connection
    (BRIDGE if bridge else OPEN).update(pair)
    try:
        await asyncio.gather(pipe(client_reader, chrome_writer), pipe(chrome_reader, client_writer))
    finally:
        OPEN.difference_update(pair)
        BRIDGE.difference_update(pair)


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
