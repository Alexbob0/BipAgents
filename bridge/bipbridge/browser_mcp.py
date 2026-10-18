"""An agent's browser tools, started when used (browsers.py): the MCP server Hermes keeps connected in place of the
one inside the browser container.

It answers Hermes' handshake and tool list with what the real server said last time, starts the agent's browser
container and the real server (stealth-browser-mcp, in it) at the first call that needs them, relays everything
while they run, and stops both after ``idle`` seconds of nobody using that browser. A container stopped this way
costs no memory; the next call starts it again (a few seconds) with the same profile, logins included.

« Using » is shared through the ``busy`` file, touched at each call (and every 30 s of a long one) by every copy of
this server (the agent's gateway, a `hermes chat` beside it), every minute by the bridge while the person watches or
drives that desktop, and here while something holds a connection to Chrome's DevTools (``cdp_port``: the vault,
Hermes' own browser tools). None stops the browser while another uses it.

    python3 browser_mcp.py <config.json>

``config.json`` (written by browsers.setup): ``start``, ``ready``, ``active``, ``server`` and ``stop`` (commands),
``cache`` (the file keeping the real server's answers), ``busy`` (a file), ``cdp_port``, ``idle`` (seconds). Standard
library only: Hermes runs it with the bridge's Python, from anywhere.
"""
from __future__ import annotations

import asyncio
import json
import os
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Optional

# What Hermes asks while connecting, answered without starting anything once the real server has said it (the
# others, never asked while it ran, as empty).
LISTS = ("tools/list", "resources/list", "resources/templates/list", "prompts/list")
EMPTY = {"resources/list": {"resources": []}, "resources/templates/list": {"resourceTemplates": []},
         "prompts/list": {"prompts": []}}
LINE_LIMIT = 64 * 1024 * 1024   # a screenshot comes back as one JSON line
READY_SECONDS = 60
TCP_TABLES = ("/proc/net/tcp", "/proc/net/tcp6")


def log(text: str) -> None:
    print(f"[browser-mcp] {text}", file=sys.stderr, flush=True)


class Proxy:
    def __init__(self, config: Dict[str, Any], output: Any = None) -> None:
        self.config = config
        self.idle = float(config.get("idle", 600))
        self.cache_path = Path(config["cache"])
        try:
            self.cache: Dict[str, Any] = json.loads(self.cache_path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            self.cache = {}
        self.output = output or sys.stdout.buffer
        self.child: Optional[asyncio.subprocess.Process] = None
        self.client_init: Dict[str, Any] = {}
        self.pending: Dict[int, asyncio.Future] = {}
        self.next_id = 1
        self.calls = 0                       # tool calls under way
        self.starting: Optional[asyncio.Task] = None
        self.switching = asyncio.Lock()      # starting and stopping the container, one at a time

    # -- the two sides -------------------------------------------------------------------------------------------

    def to_client(self, message: Dict[str, Any]) -> None:
        self.output.write(json.dumps(message, separators=(",", ":")).encode() + b"\n")
        self.output.flush()

    async def to_server(self, message: Dict[str, Any]) -> None:
        assert self.child is not None and self.child.stdin is not None
        self.child.stdin.write(json.dumps(message, separators=(",", ":")).encode() + b"\n")
        await self.child.stdin.drain()

    async def request(self, method: str, params: Any) -> Dict[str, Any]:
        """A request to the real server; its whole response (``result`` or ``error``)."""
        number, self.next_id = self.next_id, self.next_id + 1
        future = asyncio.get_running_loop().create_future()
        self.pending[number] = future
        message: Dict[str, Any] = {"jsonrpc": "2.0", "id": number, "method": method}
        if params is not None:
            message["params"] = params
        await self.to_server(message)
        return await future

    # -- Hermes' messages ----------------------------------------------------------------------------------------

    async def handle(self, message: Dict[str, Any]) -> None:
        method, ident = message.get("method"), message.get("id")
        if method is None:
            # Hermes answering a request of the real server (ids of the server's own numbering).
            if self.running:
                await self.to_server(message)
            return
        if method == "initialize":
            self.client_init = message.get("params") or {}
            try:
                if "initialize" not in self.cache:
                    await self.ensure_server()   # the first time ever: what it serves is learned once
                self.to_client({"jsonrpc": "2.0", "id": ident, "result": self.cache["initialize"]})
            except Exception as exc:  # noqa: BLE001 — told to Hermes, which retries later
                self.fail(ident, exc)
            return
        if method == "notifications/initialized":
            return                               # the real server gets its own after its handshake
        if method == "ping":
            self.to_client({"jsonrpc": "2.0", "id": ident, "result": {}})
            return
        if method.startswith("notifications/"):
            if self.running:
                await self.to_server(message)
            return
        if method in LISTS and not self.running and (method in self.cache or method in EMPTY):
            self.to_client({"jsonrpc": "2.0", "id": ident, "result": self.cache.get(method, EMPTY.get(method))})
            return
        calling = method == "tools/call"
        if calling:
            self.calls += 1
            self.touch()
        try:
            await self.ensure_server()
            response = await self.request(method, message.get("params"))
            if method in LISTS and "result" in response:
                self.remember(method, response["result"])
            response["id"] = ident
            self.to_client(response)
        except Exception as exc:  # noqa: BLE001
            self.fail(ident, exc)
        finally:
            if calling:
                self.calls -= 1
                self.touch()

    def fail(self, ident: Any, exc: BaseException) -> None:
        if ident is not None:
            self.to_client({"jsonrpc": "2.0", "id": ident,
                            "error": {"code": -32603, "message": f"the agent's browser could not start: {exc}"}})

    def remember(self, key: str, value: Any) -> None:
        if self.cache.get(key) == value:
            return
        self.cache[key] = value
        try:
            self.cache_path.parent.mkdir(parents=True, exist_ok=True)
            partial = self.cache_path.with_suffix(".part")
            partial.write_text(json.dumps(self.cache), encoding="utf-8")
            os.replace(partial, self.cache_path)
        except OSError as exc:
            log(f"cache not saved: {exc}")

    # -- the real server -----------------------------------------------------------------------------------------

    @property
    def running(self) -> bool:
        return self.child is not None and self.child.returncode is None

    async def ensure_server(self) -> None:
        if self.running:
            return
        if self.starting is None or self.starting.done():
            self.starting = asyncio.create_task(self.start())
        await asyncio.shield(self.starting)

    async def start(self) -> None:
        async with self.switching:
            if self.running:
                return
            try:
                await self.start_server()
            except BaseException:
                await self.stop_child()
                raise

    async def start_server(self) -> None:
        started = time.monotonic()
        await run(self.config["start"])
        while await run(self.config["ready"], check=False) != 0:
            if time.monotonic() - started > READY_SECONDS:
                raise RuntimeError("its container did not start")
            await asyncio.sleep(0.5)
        self.child = await asyncio.create_subprocess_exec(
            *self.config["server"], stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE, limit=LINE_LIMIT)
        asyncio.create_task(self.read_server(self.child))
        init = await self.request("initialize", self.client_init or {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "bipagents", "version": "1"}})
        if "result" not in init:
            raise RuntimeError(f"its server refused to start: {init.get('error')}")
        self.remember("initialize", init["result"])
        await self.to_server({"jsonrpc": "2.0", "method": "notifications/initialized"})
        tools = await self.request("tools/list", {})
        if "result" in tools:
            self.remember("tools/list", tools["result"])
        self.touch()
        log(f"started in {time.monotonic() - started:.1f} s")

    async def read_server(self, child: asyncio.subprocess.Process) -> None:
        assert child.stdout is not None
        while line := await child.stdout.readline():
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if "method" in message:
                self.to_client(message)          # its notifications and requests go to Hermes as they are
            elif (future := self.pending.pop(message.get("id"), None)) is not None and not future.done():
                future.set_result(message)
        await child.wait()
        for future in self.pending.values():
            if not future.done():
                future.set_exception(RuntimeError("its server stopped"))
        self.pending.clear()

    async def stop(self) -> None:
        async with self.switching:
            await self.stop_child()
            await run(self.config["stop"], check=False)

    async def stop_child(self) -> None:
        child, self.child = self.child, None
        if child is not None and child.returncode is None:
            if child.stdin is not None:
                child.stdin.close()
            try:
                await asyncio.wait_for(child.wait(), 5)
            except asyncio.TimeoutError:
                child.kill()

    def touch(self) -> None:
        busy = Path(self.config["busy"])
        try:
            busy.touch()
        except FileNotFoundError:
            busy.parent.mkdir(parents=True, exist_ok=True)
            busy.touch()
        except OSError:
            pass

    def unused_for(self) -> float:
        try:
            return time.time() - Path(self.config["busy"]).stat().st_mtime
        except OSError:
            return float("inf")

    async def watch_idle(self, every: float = 15) -> None:
        while True:
            await asyncio.sleep(every)
            if self.calls or devtools_connected(self.config.get("cdp_port")):
                if self.unused_for() > 30:
                    self.touch()
            elif self.unused_for() >= self.idle and (self.running or await run(self.config["active"], check=False) == 0):
                # Also when another started it (the desktop in the app, a `hermes chat` since closed).
                log(f"unused for {self.idle:.0f} s: stopped")
                await self.stop()

    async def serve(self, reader: asyncio.StreamReader) -> None:
        idle = asyncio.create_task(self.watch_idle()) if self.idle > 0 else None
        tasks = set()
        try:
            while line := await reader.readline():
                try:
                    message = json.loads(line)
                except ValueError:
                    continue
                task = asyncio.create_task(self.handle(message))
                tasks.add(task)
                task.add_done_callback(tasks.discard)
            if tasks:   # what it asked last, answered before leaving
                await asyncio.wait(tasks, timeout=30)
        finally:
            if idle:
                idle.cancel()
            # Hermes gone: its server too. The browser stays for whoever else uses it, until it is unused.
            await self.stop_child()


def devtools_connected(port: Optional[int]) -> bool:
    """Something on this machine holds a connection to the browser's DevTools port (Linux: /proc/net/tcp)."""
    if not port:
        return False
    wanted = f":{int(port):04X}"
    for table in TCP_TABLES:
        try:
            lines = Path(table).read_text().splitlines()[1:]
        except OSError:
            continue
        for line in lines:
            parts = line.split()
            # local address, remote address, state (01: established)
            if len(parts) > 3 and parts[3] == "01" and (parts[1].endswith(wanted) or parts[2].endswith(wanted)):
                return True
    return False


async def run(command: List[str], check: bool = True) -> int:
    process = await asyncio.create_subprocess_exec(*command, stdout=asyncio.subprocess.DEVNULL,
                                                   stderr=asyncio.subprocess.DEVNULL)
    code = await process.wait()
    if check and code != 0:
        raise RuntimeError(f"{Path(command[0]).name} {' '.join(command[1:3])} failed ({code})")
    return code


async def main(path: str) -> None:
    config = json.loads(Path(path).read_text(encoding="utf-8"))
    # Hermes starts MCP servers with few variables: `systemctl --user` and rootless podman find the session here.
    runtime_dir = Path(f"/run/user/{os.getuid()}")
    if "XDG_RUNTIME_DIR" not in os.environ and runtime_dir.is_dir():
        os.environ["XDG_RUNTIME_DIR"] = str(runtime_dir)
    loop = asyncio.get_running_loop()
    reader = asyncio.StreamReader(limit=LINE_LIMIT)
    await loop.connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), sys.stdin)
    await Proxy(config).serve(reader)


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1]))
