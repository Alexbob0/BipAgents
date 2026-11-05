"""A fake phone for the phone exit (bridge-rs/src/phone_exit.rs): it opens the tunnel's WebSocket like the Android
demo, answers each OPEN (its own checks first, a test may refuse), connects to a local server standing in for the site,
checks the ClientHello's server name, and copies bytes both ways within the windows. Also: the agents' proxy's side
of the hub (its unix socket) and a minimal ClientHello."""
from __future__ import annotations

import asyncio
import json
import struct
from typing import Callable, Dict, List, Optional, Tuple

from websockets.asyncio.client import connect

HELLO, OPEN, OPEN_OK, OPEN_ERR, DATA, WINDOW, FIN, RST, STATS, BYE = 1, 2, 3, 4, 5, 6, 7, 8, 11, 12
START_WINDOW = 256 * 1024
CHUNK = 16 * 1024


def frame(kind: int, stream: int, payload: bytes = b"") -> bytes:
    return struct.pack(">BII", kind, stream, len(payload)) + payload


def client_hello(name: Optional[str]) -> bytes:
    """A TLS-shaped ClientHello record naming `name` (None: no server name extension)."""
    exts = bytes([0x00, 0x2B, 0x00, 0x03, 0x02, 0x03, 0x04])
    if name is not None:
        raw = name.encode()
        entry = b"\x00" + struct.pack(">H", len(raw)) + raw
        data = struct.pack(">H", len(entry)) + entry
        exts += b"\x00\x00" + struct.pack(">H", len(data)) + data
    body = b"\x03\x03" + b"\x07" * 32 + b"\x00" + b"\x00\x02\x13\x01" + b"\x01\x00" + struct.pack(">H", len(exts)) + exts
    hs = b"\x01" + len(body).to_bytes(3, "big") + body
    return b"\x16\x03\x01" + struct.pack(">H", len(hs)) + hs


def sni_of(data: bytes) -> Optional[str]:
    """The server name in a ClientHello record (None: not one, or none named)."""
    try:
        if data[0] != 0x16:
            return None
        h = data[9:]
        at = 2 + 32
        at += 1 + h[at]
        at += 2 + struct.unpack(">H", h[at:at + 2])[0]
        at += 1 + h[at]
        end = at + 2 + struct.unpack(">H", h[at:at + 2])[0]
        at += 2
        while at + 4 <= end:
            kind, size = struct.unpack(">HH", h[at:at + 4])
            if kind == 0:
                n = struct.unpack(">H", h[at + 7:at + 9])[0]
                return h[at + 9:at + 9 + n].decode()
            at += 4 + size
    except (IndexError, struct.error):
        return None
    return None


class Stream:
    def __init__(self):
        self.credit = START_WINDOW
        self.more = asyncio.Event()
        self.inbox: asyncio.Queue = asyncio.Queue()


class FakePhone:
    """`site(host, port)` -> (address, port) of the local server to reach; `judge(host, port)` -> None or the refusal
    reason (the phone's own checks)."""

    def __init__(self, url: str, token: str, site: Callable[[str, int], Tuple[str, int]],
                 judge: Callable[[str, int], Optional[str]] = lambda h, p: None):
        self.url, self.token, self.site, self.judge = url, token, site, judge
        self.opens: List[dict] = []
        self.refused: List[Tuple[str, str]] = []
        self.resets = 0
        self.bye: Optional[str] = None
        self.ready = asyncio.Event()
        self.closed = asyncio.Event()
        self.streams: Dict[int, Stream] = {}
        self.ws = None

    async def send(self, kind: int, stream: int, payload: bytes = b"") -> None:
        await self.ws.send(frame(kind, stream, payload))

    async def run(self) -> None:
        try:
            async with connect(self.url, additional_headers={"Authorization": f"Bearer {self.token}"},
                               max_size=None, compression=None, proxy=None, open_timeout=10) as ws:
                self.ws = ws
                await self.send(HELLO, 0, json.dumps({"device": "fake", "version": "0", "network": "wifi"}).encode())
                self.ready.set()
                async for message in ws:
                    kind, sid, size = struct.unpack(">BII", message[:9])
                    payload = message[9:9 + size]
                    if kind == OPEN:
                        info = json.loads(payload)
                        self.opens.append(info)
                        self.streams[sid] = Stream()
                        asyncio.ensure_future(self.carry(sid, info["host"], int(info["port"])))
                    elif kind in (DATA, FIN, RST) and sid in self.streams:
                        if kind == RST:
                            self.resets += 1
                        self.streams[sid].inbox.put_nowait((kind, payload))
                    elif kind == WINDOW and sid in self.streams:
                        self.streams[sid].credit += struct.unpack(">I", payload)[0]
                        self.streams[sid].more.set()
                    elif kind == BYE:
                        self.bye = payload.decode()
                        break
        finally:
            self.closed.set()

    async def carry(self, sid: int, host: str, port: int) -> None:
        stream = self.streams[sid]
        why = self.judge(host, port)
        if why:
            self.refused.append((host, why))
            await self.send(OPEN_ERR, sid, why.encode())
            return
        address, real_port = self.site(host, port)
        try:
            reader, writer = await asyncio.open_connection(address, real_port)
        except OSError:
            await self.send(OPEN_ERR, sid, b"connect")
            return
        await self.send(OPEN_OK, sid)

        async def down():
            while True:
                data = await reader.read(CHUNK)
                if not data:
                    await self.send(FIN, sid)
                    return
                while stream.credit < len(data):
                    stream.more.clear()
                    await stream.more.wait()
                stream.credit -= len(data)
                await self.send(DATA, sid, data)

        async def up():
            first, owed = True, 0
            while True:
                kind, payload = await stream.inbox.get()
                if kind != DATA:
                    if kind == FIN:
                        writer.write_eof()
                    else:
                        writer.close()
                    return
                if first:
                    first = False
                    if sni_of(payload) != host:
                        self.refused.append((host, "sni-mismatch"))
                        await self.send(RST, sid)
                        writer.close()
                        return
                writer.write(payload)
                await writer.drain()
                owed += len(payload)
                if owed >= START_WINDOW // 4:
                    await self.send(WINDOW, sid, struct.pack(">I", owed))
                    owed = 0

        tasks = [asyncio.ensure_future(down()), asyncio.ensure_future(up())]
        try:
            await asyncio.gather(*tasks)
        except Exception:
            for t in tasks:
                t.cancel()


async def exit_stream(socket_path: str, host: str, port: int = 443, agent: str = "vie"):
    """The agents' proxy's side: (answer line, reader, writer) for one stream asked of the hub."""
    reader, writer = await asyncio.open_unix_connection(socket_path)
    writer.write(f"EXIT {host} {port} {agent}\n".encode())
    await writer.drain()
    line = (await asyncio.wait_for(reader.readline(), 30)).decode().strip()
    return line, reader, writer


async def echo_server():
    """A local stand-in for a site: echoes every byte back. Returns (server, port, what it received)."""
    got = bytearray()

    async def handle(reader, writer):
        try:
            while True:
                data = await reader.read(65536)
                if not data:
                    break
                got.extend(data)
                writer.write(data)
                await writer.drain()
        except Exception:
            pass
        writer.close()

    server = await asyncio.start_server(handle, "127.0.0.1", 0)
    return server, server.sockets[0].getsockname()[1], got
