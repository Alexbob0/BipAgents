"""The agents' desktop, live in the app: watch what the agent does in its browser, take over with a tap.

Hermes runs a desktop per agent (« Bot Desktop »: Xvnc and Xfce, the agent's visible Chromium) and serves it as
RFB on a private Unix socket, `<agent's HERMES_HOME>/bot-desktop/rfb.sock` (or inside its sandbox, reached with
`docker exec`). The bridge relays it to the app over a WebSocket opened with a short-lived ticket, and shows it
with noVNC (static/novnc, MPL-2.0).

Watching sends no input: the bridge drops keyboard, pointer, clipboard and resize messages unless the person
took over. « Prendre la main » writes Hermes' own lease (`bot-desktop/lease.json`, holder « human »), which
stops the agent's browser and computer-use tools until the person hands back.

An agent whose browser already has a viewer page (a noVNC set up by hand, like Vie's) gives its address in the
config (`desktop_url`), and the app opens that page instead.
"""
from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import os
import secrets
import struct
import time
from pathlib import Path
from typing import Any, Callable, Dict, Optional, Tuple

try:
    import fcntl
except ImportError:  # pragma: no cover
    fcntl = None  # type: ignore[assignment]

from fastapi import APIRouter, HTTPException, Request, WebSocket, WebSocketDisconnect
from fastapi.responses import HTMLResponse
from fastapi.staticfiles import StaticFiles

from .config import AgentConfig, Config
from .logs import fields

log = logging.getLogger("bipbridge.desktop")

VIEWER_ID = "bipagents-app"
TICKET_SECONDS = 600
_RELAY = (
    "import os,socket,sys,threading\n"
    "s=socket.socket(socket.AF_UNIX);s.connect(sys.argv[1])\n"
    "def up():\n"
    "  while True:\n"
    "    d=os.read(0,65536)\n"
    "    if not d: break\n"
    "    s.sendall(d)\n"
    "  s.shutdown(socket.SHUT_WR)\n"
    "threading.Thread(target=up,daemon=True).start()\n"
    "while True:\n"
    "  d=s.recv(65536)\n"
    "  if not d: break\n"
    "  os.write(1,d)\n"
)


# -- input gate --------------------------------------------------------------------------------------

class InputGate:
    """Client → server RFB (security « None »): passes the handshake, forces the shared flag, frames every
    message and drops input (key, pointer, clipboard, resize, QEMU key) unless `allow_input()` says yes."""

    INPUT = {4, 5, 6, 251, 255}

    def __init__(self, allow_input: Callable[[], bool]):
        self.allow_input = allow_input
        self.buffer = b""
        self.handshake = 14   # ProtocolVersion (12) + security type (1) + ClientInit (1)

    def _length(self) -> Optional[int]:
        b = self.buffer
        kind = b[0]
        fixed = {0: 20, 3: 10, 4: 8, 5: 6, 150: 10}
        if kind in fixed:
            return fixed[kind]
        if kind == 2:
            return None if len(b) < 4 else 4 + 4 * struct.unpack(">H", b[2:4])[0]
        if kind == 6:
            if len(b) < 8:
                return None
            size = abs(struct.unpack(">i", b[4:8])[0])
            if size > 256 * 1024:
                raise ValueError("clipboard too large")
            return 8 + size
        if kind == 248:
            return None if len(b) < 9 else 9 + b[8]
        if kind == 251:
            return None if len(b) < 7 else 8 + 16 * b[6]
        if kind == 255:
            return 12
        raise ValueError(f"unknown RFB message {kind}")

    def feed(self, chunk: bytes) -> bytes:
        out = b""
        self.buffer += chunk
        if self.handshake:
            take = self.buffer[:self.handshake]
            self.buffer = self.buffer[self.handshake:]
            self.handshake -= len(take)
            if self.handshake == 0 and take:
                take = take[:-1] + b"\x01"   # ClientInit: always shared, never kick the others off
            out += take
        while self.buffer:
            length = self._length()
            if length is None or len(self.buffer) < length:
                break
            message, self.buffer = self.buffer[:length], self.buffer[length:]
            if message[0] not in self.INPUT or self.allow_input():
                out += message
        return out


# -- Hermes' lease -----------------------------------------------------------------------------------

def desktop_dir(config: Config, agent: AgentConfig) -> Optional[Path]:
    root = agent.space or (config.hermes.home if config.hermes.enabled and
                           agent.hermes_url.startswith(f"{config.hermes.hub_url}/p/") else None)
    return Path(root) / "profiles" / agent.name / "bot-desktop" if root else None


def _read_lease(folder: Path) -> Dict[str, Any]:
    try:
        return json.loads((folder / "lease.json").read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {"holder": "agent", "viewer_id": None, "since": time.time(), "reason": "", "epoch": 0}
    except (OSError, ValueError):
        return {"holder": "human", "viewer_id": None, "since": time.time(), "reason": "", "epoch": 0}   # fail safe


def set_lease(folder: Path, human: bool, reason: str = "") -> Dict[str, Any]:
    """The same file, lock and fields as Hermes' own lease (tools/bot_desktop/lease.py)."""
    folder.mkdir(mode=0o700, parents=True, exist_ok=True)
    with open(folder / "lease.lock", "a+", encoding="utf-8") as lock:
        if fcntl is not None:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        lease = _read_lease(folder)
        holder = "human" if human else "agent"
        if lease.get("holder") != holder or (human and lease.get("viewer_id") != VIEWER_ID):
            lease.update(holder=holder, viewer_id=VIEWER_ID if human else None, since=time.time(),
                         reason=reason if human else "", epoch=int(lease.get("epoch", 0)) + 1)
            tmp = folder / "lease.json.tmp"
            fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, "w", encoding="utf-8") as fh:
                json.dump(lease, fh)
            os.replace(tmp, folder / "lease.json")
    return lease


def app_holds(folder: Path) -> bool:
    lease = _read_lease(folder)
    return lease.get("holder") == "human" and lease.get("viewer_id") == VIEWER_ID


# -- the RFB stream ------------------------------------------------------------------------------------

async def open_rfb(folder: Path) -> Tuple[asyncio.StreamReader, Any, Callable[[], Any]]:
    """(reader, writer, close) on the desktop: its socket on this host, or inside its sandbox."""
    marker = folder / "sandbox.json"
    if marker.exists():
        info = json.loads(marker.read_text(encoding="utf-8"))
        if not info.get("container"):
            raise FileNotFoundError("this agent's desktop runs in a sandbox the bridge cannot reach")
        proc = await asyncio.create_subprocess_exec(
            info.get("docker") or "docker", "exec", "-i", "-u", "pn", info["container"], "python3", "-c", _RELAY,
            f"{info['dir']}/rfb.sock", stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL)
        assert proc.stdout is not None and proc.stdin is not None

        def close() -> None:
            with contextlib.suppress(ProcessLookupError):
                proc.kill()
        return proc.stdout, proc.stdin, close
    sock = folder / "rfb.sock"
    if not sock.exists():
        raise FileNotFoundError("the desktop is not running")
    reader, writer = await asyncio.open_unix_connection(str(sock))
    return reader, writer, writer.close


# -- routes ------------------------------------------------------------------------------------------

class Tickets:
    def __init__(self) -> None:
        self.items: Dict[str, Tuple[str, float]] = {}

    def new(self, agent: str) -> str:
        now = time.time()
        self.items = {t: v for t, v in self.items.items() if v[1] > now}
        ticket = secrets.token_urlsafe(24)
        self.items[ticket] = (agent, now + TICKET_SECONDS)
        return ticket

    def agent(self, ticket: str) -> Optional[str]:
        found = self.items.get(ticket)
        return found[0] if found and found[1] > time.time() else None


PAGE = """<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,maximum-scale=1,user-scalable=no">
<style>html,body{margin:0;height:100%;background:#111;overflow:hidden}#screen{width:100%;height:100%}
#state{position:fixed;top:8px;left:50%;transform:translateX(-50%);font:600 13px -apple-system,sans-serif;color:#fff;
background:rgba(0,0,0,.55);padding:4px 10px;border-radius:12px}
#kbd{position:fixed;right:16px;bottom:24px;width:52px;height:52px;border-radius:26px;border:0;font-size:24px;
background:rgba(255,255,255,.9);display:none}
#typing{position:fixed;left:-1000px;top:0;opacity:0;width:1px;height:1px}</style></head>
<body><div id="screen"></div><div id="state">Connexion…</div>
<button id="kbd" aria-label="Clavier">⌨︎</button>
<textarea id="typing" autocapitalize="off" autocomplete="off" autocorrect="off" spellcheck="false"></textarea>
<script type="module">
import RFB from "../static/novnc/core/rfb.js";
const state = document.getElementById("state"), kbd = document.getElementById("kbd"), typing = document.getElementById("typing");
const url = (location.protocol === "https:" ? "wss://" : "ws://") + location.host + location.pathname.replace(/\\/$/, "") + "/ws";
const rfb = new RFB(document.getElementById("screen"), url, {shared: true});
rfb.scaleViewport = true; rfb.resizeSession = false; rfb.viewOnly = __VIEW_ONLY__;
rfb.addEventListener("connect", () => { state.style.display = "none"; post("connected"); });
rfb.addEventListener("disconnect", () => { state.style.display = "block"; state.textContent = "Bureau fermé"; post("disconnected"); });
function post(event) { try { window.webkit.messageHandlers.desktop.postMessage(event); } catch (_) {} }
function show() { kbd.style.display = rfb.viewOnly ? "none" : "block"; }
window.setViewOnly = on => { rfb.viewOnly = on; if (on) typing.blur(); show(); };
show();
// iOS opens its keyboard only for a text field tapped by the person: this one turns what is typed into keys.
kbd.addEventListener("click", () => typing.focus());
const keysym = ch => { const c = ch.codePointAt(0); return c < 0x100 ? c : 0x01000000 + c; };
typing.addEventListener("input", e => {
  if (e.inputType === "deleteContentBackward") rfb.sendKey(0xff08, "Backspace");
  else for (const ch of typing.value) ch === "\\n" ? rfb.sendKey(0xff0d, "Enter") : rfb.sendKey(keysym(ch));
  typing.value = "";
});
typing.addEventListener("keydown", e => {
  const special = {Backspace: 0xff08, Enter: 0xff0d, Tab: 0xff09, Escape: 0xff1b, ArrowLeft: 0xff51, ArrowUp: 0xff52,
                   ArrowRight: 0xff53, ArrowDown: 0xff54};
  if (special[e.key] && !(e.key === "Backspace" && typing.value === "")) return;
  if (special[e.key]) { e.preventDefault(); rfb.sendKey(special[e.key], e.key); }
});
</script></body></html>"""


def desktop_router() -> APIRouter:
    """`POST /v1/agents/{id}/desktop` and `…/desktop/control` take the bridge key (mounted under /v1); the page and its
    WebSocket (`/desktop/<ticket>/…`) take the ticket."""
    router = APIRouter()

    @router.get("/desktop/{ticket}/")
    async def page(ticket: str, request: Request) -> HTMLResponse:
        services = request.app.state.services
        name = services.desktop_tickets.agent(ticket)
        if name is None:
            raise HTTPException(status_code=404, detail="expired")
        agent = services.config.agent(name)
        folder = desktop_dir(services.config, agent) if agent else None
        view_only = "false" if folder is not None and app_holds(folder) else "true"
        return HTMLResponse(PAGE.replace("__VIEW_ONLY__", view_only), headers={"Cache-Control": "no-store"})

    @router.websocket("/desktop/{ticket}/ws")
    async def stream(websocket: WebSocket, ticket: str) -> None:
        services = websocket.app.state.services
        name = services.desktop_tickets.agent(ticket)
        agent = services.config.agent(name) if name else None
        folder = desktop_dir(services.config, agent) if agent else None
        if folder is None:
            await websocket.close(code=4401)
            return
        try:
            reader, writer, close = await open_rfb(folder)
        except (OSError, ValueError) as exc:
            log.info("desktop unavailable", extra=fields(agent=name, error=str(exc)))
            await websocket.close(code=4001)
            return
        await websocket.accept(subprotocol="binary" if "binary" in websocket.scope.get("subprotocols", []) else None)
        gate = InputGate(lambda: app_holds(folder))
        last_touch = 0.0

        async def down() -> None:
            nonlocal last_touch
            while data := await reader.read(65536):
                await websocket.send_bytes(data)
                if time.time() - last_touch > 60:   # someone is watching: Hermes keeps the desktop up
                    last_touch = time.time()
                    with contextlib.suppress(OSError):
                        (folder / "activity").touch()

        async def up() -> None:
            while True:
                message = await websocket.receive()
                if message["type"] == "websocket.disconnect":
                    return
                data = message.get("bytes") or (message.get("text") or "").encode()
                out = gate.feed(data)
                if out:
                    writer.write(out)
                    await writer.drain()

        log.info("desktop viewer connected", extra=fields(agent=name))
        tasks = [asyncio.create_task(down()), asyncio.create_task(up())]
        try:
            await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
        except (WebSocketDisconnect, ValueError, ConnectionError):
            pass
        finally:
            for task in tasks:
                task.cancel()
            close()
            with contextlib.suppress(Exception):
                await websocket.close()

    return router


def desktop_admin_router() -> APIRouter:
    router = APIRouter()

    @router.post("/agents/{agent_id}/desktop")
    async def open_desktop(agent_id: str, request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        agent = services.config.agent(agent_id)
        if agent is None:
            raise HTTPException(status_code=404, detail="unknown agent")
        if agent.desktop_url:
            return {"kind": "page", "url": agent.desktop_url}
        folder = desktop_dir(services.config, agent)
        if folder is None or not ((folder / "rfb.sock").exists() or (folder / "sandbox.json").exists()):
            raise HTTPException(status_code=409, detail={"code": "no_desktop"})
        ticket = services.desktop_tickets.new(agent.name)
        lease = _read_lease(folder)
        return {"kind": "novnc", "path": f"desktop/{ticket}/", "you_hold": app_holds(folder),
                "holder": lease.get("holder"), "reason": lease.get("reason") or None}

    @router.post("/agents/{agent_id}/desktop/control")
    async def control(agent_id: str, request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        agent = services.config.agent(agent_id)
        folder = desktop_dir(services.config, agent) if agent else None
        if folder is None:
            raise HTTPException(status_code=404, detail="unknown agent")
        body = await request.json()
        lease = set_lease(folder, bool(body.get("take")), reason="Depuis l'app BipAgents")
        log.info("desktop control", extra=fields(agent=agent.name, holder=lease["holder"]))
        return {"holder": lease["holder"], "you_hold": app_holds(folder)}

    return router


def static_files() -> StaticFiles:
    return StaticFiles(directory=str(Path(__file__).with_name("static") / "novnc"))
