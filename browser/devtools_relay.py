"""Chrome's DevTools, reachable from the host at an address that outlives Chrome.

Chrome only listens on this container's loopback (9222), and its browser endpoint (`/devtools/browser/<id>`) changes
each time it starts: the MCP server starts it on demand and closes it when idle. Hermes keeps the endpoint it resolved
(its vault fills go through it), so this relay listens on 9223, the port published on the host's 127.0.0.1, and sends
any browser endpoint to the current one. Everything else passes through unchanged.

While the person holds the browser in the app (hold_gate.py's flag), nothing gets through, and the connections open
when they took it are closed: Hermes (the vault's fills) waits too. Only the bridge's own connections get through,
those whose request shows the hold's token: while a card payment holds it (the flag reads « card <token> »,
`X-Bip-Card: <token>`), it types the card and clicks « Pay », and nothing else reads the page meanwhile; while the
person holds it from the app (« human <token> », `X-Bip-Hold: <token>`), it shows them the page and passes on their
taps and keys (a flag with no token, from an older bridge, lets nothing through).

`GET /bipagents/relay` is answered here, never by Chrome: this relay's version, so the bridge can tell an image built
before it (Chrome's 404) and ask the person to update the agent's browser.

Nothing else gets through without the browser's secret (security audit 2026-10-27, 11 and 21): another account of a
shared server reaches 127.0.0.1:<port> too, and a page in this Chrome could open a WebSocket to this container's
loopback. The bridge writes the secret next to vnc.pass and mounts it read-only at /run/bipagents/cdp.token; a request
shows it as a header (`X-Bip-Cdp: <secret>`, the bridge's own) or as the start of its path (`/t/<secret>/…`, Hermes'
`browser.cdp_url`, which takes a URL only): the prefix is taken off before Chrome sees the request, and put back in
the WebSocket addresses Chrome's /json answers name, so a client resolving them comes back through it. A request
carrying an `Origin:` (what a web page's WebSocket or fetch always sends; Hermes, nodriver and the bridge send none) is
refused whatever it shows. A container started by an older unit, without the mount, has no secret: there, everything
but Origin-carrying requests passes as before (and a /t/<anything>/ prefix is taken off), until its unit is written
again (`bipagents update`, the nightly browser refresh)."""
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
VERSION_PATH = re.compile(rb"^GET /bipagents/relay[ ?]")
RELAY_VERSION = 3   # 2: the version route itself, the person's hold token (« human <token> »); 3: the secret
TOKEN_HEADER = re.compile(rb"\r\nx-bip-(?:card|hold):[ \t]*([A-Za-z0-9]+)[ \t]*\r\n", re.IGNORECASE)
SECRET_FILE = os.environ.get("BIP_CDP_TOKEN_FILE", "/run/bipagents/cdp.token")
SECRET_HEADER = re.compile(rb"\r\nx-bip-cdp:[ \t]*([A-Za-z0-9]+)[ \t]*\r\n", re.IGNORECASE)
# The request line with the secret in front of its path: method, secret, the rest of the path (« / » when none).
PREFIXED = re.compile(rb"^([A-Z]+) /t/([A-Za-z0-9]+)(/[^ ]*)? ")
ORIGIN = re.compile(rb"\r\norigin:", re.IGNORECASE)
UPGRADE = re.compile(rb"\r\nupgrade:[ \t]*websocket", re.IGNORECASE)
# The WebSocket addresses in Chrome's /json answers (webSocketDebuggerUrl, devtoolsFrontendUrl's ws=…).
WS_ADDRESS = re.compile(rb"((?:wss?://|[?&]wss?=)[^/\"&\s]+)/devtools/")


def current_endpoint() -> bytes:
    with urllib.request.urlopen(f"http://{CHROME[0]}:{CHROME[1]}/json/version", timeout=3) as response:
        url = json.load(response)["webSocketDebuggerUrl"]
    return b"GET /devtools/browser/" + url.rsplit("/devtools/browser/", 1)[1].encode()


def _flag() -> bytes:
    try:
        with open(HOLD, "rb") as flag:
            return flag.read(200)
    except OSError:
        return b""


def payment_token() -> bytes | None:
    """The token of the card payment holding the browser, if one does."""
    text = _flag()
    return text[5:].strip() or None if text.startswith(b"card ") else None


def hold_token() -> bytes | None:
    """The token of whatever holds the browser (a card payment, the person in the app), if it has one."""
    text = _flag()
    for kind in (b"card ", b"human "):
        if text.startswith(kind):
            return text[len(kind):].strip() or None
    return None


def secret() -> bytes | None:
    """The browser's secret, b"" when its file is mounted but unusable (nothing then gets through), None when there is
    no file (a container from an older unit)."""
    try:
        with open(SECRET_FILE, "rb") as file:
            text = file.read(200).strip()
    except FileNotFoundError:
        return None
    except OSError:
        return b""
    return text if len(text) >= 16 and text.isalnum() else b""


def authenticate(head: bytes) -> tuple[bytes | None, bytes | None]:
    """(the head to send Chrome, the path prefix to put back in its answer), or (None, None): refused. A web page's
    request (it carries an Origin) never gets through; with a secret, only a request showing it does."""
    if ORIGIN.search(head):
        return None, None
    expected = secret()
    prefixed = PREFIXED.match(head)
    if prefixed:
        shown, prefix = prefixed.group(2), b"/t/" + prefixed.group(2)
        head = prefixed.group(1) + b" " + (prefixed.group(3) or b"/") + b" " + head[prefixed.end():]
    else:
        found = SECRET_HEADER.search(head)
        shown, prefix = (found.group(1) if found else None), None
    if expected is None:
        return head, prefix
    if not expected or not shown or not hmac.compare_digest(expected, shown):
        return None, None
    return head, prefix


def with_prefix(body: bytes, prefix: bytes) -> bytes:
    """Chrome's /json answer with the WebSocket addresses it names coming back through the prefix."""
    return WS_ADDRESS.sub(lambda m: m.group(1) + prefix + b"/devtools/", body)


async def answer_with_prefix(head: bytes, prefix: bytes, client_writer: asyncio.StreamWriter) -> None:
    """One plain HTTP request (Hermes' /json/version): Chrome's answer read whole, its addresses given the prefix, the
    connection closed after it."""
    chrome_reader, chrome_writer = await asyncio.open_connection(*CHROME)
    try:
        chrome_writer.write(head)
        await chrome_writer.drain()
        answer = await chrome_reader.readuntil(b"\r\n\r\n")
        length = re.search(rb"\r\ncontent-length:[ \t]*(\d+)", answer, re.IGNORECASE)
        body = await chrome_reader.readexactly(int(length.group(1))) if length else b""
        body = with_prefix(body, prefix)
        lines = [line for line in answer[:-4].split(b"\r\n")
                 if not line.lower().startswith((b"content-length:", b"connection:"))]
        client_writer.write(b"\r\n".join(lines) + b"\r\nContent-Length: " + str(len(body)).encode()
                            + b"\r\nConnection: close\r\n\r\n" + body)
        await client_writer.drain()
    finally:
        chrome_writer.close()
        client_writer.close()


def let_through(head: bytes) -> bool:
    """Not held, or held and this is the bridge's own connection (it shows the hold's token)."""
    if not held():
        return True
    token, shown = hold_token(), TOKEN_HEADER.search(head)
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
BRIDGE: set = set()   # those of the bridge's own connections, left open by the hold they came with


def version_answer() -> bytes:
    body = json.dumps({"relay": RELAY_VERSION, "hold_token": True, "secret": secret() is not None}).encode()
    return (b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: "
            + str(len(body)).encode() + b"\r\n\r\n" + body)


async def relay(client_reader: asyncio.StreamReader, client_writer: asyncio.StreamWriter) -> None:
    try:
        head = await client_reader.readuntil(b"\r\n\r\n")
        if VERSION_PATH.match(head):
            client_writer.write(version_answer())
            await client_writer.drain()
            client_writer.close()
            return
        head, prefix = authenticate(head)
        if head is None or (held() and hold_token() is None) or not let_through(head):
            client_writer.close()
            return
        first, _, rest = head.partition(b"\r\n")
        if BROWSER_ENDPOINT.match(first):
            # only past the secret: whatever browser id it names (Hermes keeps the one it resolved) goes to the current
            first = BROWSER_ENDPOINT.sub(await asyncio.to_thread(current_endpoint), first, count=1)
        if prefix and not UPGRADE.search(rest):
            await answer_with_prefix(first + b"\r\n" + rest, prefix, client_writer)
            return
        chrome_reader, chrome_writer = await asyncio.open_connection(*CHROME)
    except (OSError, ValueError, KeyError, asyncio.IncompleteReadError, asyncio.LimitOverrunError):
        client_writer.close()   # no Chrome running yet: the caller retries
        return
    chrome_writer.write(first + b"\r\n" + rest)
    pair = {client_writer, chrome_writer}
    bridge = held()   # let through while held: the bridge's own connection (a payment, the person's view)
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
