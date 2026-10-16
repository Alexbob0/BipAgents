"""The agents' desktop, live in the app: watch what the agent does in its browser, take over with a tap.

Hermes runs a desktop per agent (« Bot Desktop »: Xvnc and Xfce, the agent's visible Chromium) and serves it as
RFB on a private Unix socket, `<agent's HERMES_HOME>/bot-desktop/rfb.sock` (or inside its sandbox, reached with
`docker exec`). The bridge relays it to the app over a WebSocket opened with a short-lived ticket, and shows it
with noVNC (static/novnc, MPL-2.0).

Watching sends no input: the bridge drops keyboard, pointer, clipboard and resize messages unless the person
took over. « Prendre la main » writes Hermes' own lease (`bot-desktop/lease.json`, holder « human »), which
stops the agent's browser and computer-use tools until the person hands back.

An agent whose browser runs on a VNC server set up by hand (like Vie's stealth Chrome) gives its address
(`desktop_vnc`: `ws://127.0.0.1:6080/websockify` for a websockify, or `tcp://127.0.0.1:5901`) and its VNC
password file (`desktop_vnc_password_file`): the bridge logs in itself and shows it the same way, so the
password never reaches the phone and the VNC port need not be published. Taking over there only lets input
through: such a browser has no Hermes lease to pause. (`desktop_url` still opens a viewer page as it is.)
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


# -- VNC set up by hand: the bridge logs in ------------------------------------------------------------

class Upstream:
    """Bytes to and from a desktop: a stream (Unix socket, TCP, docker exec) or a websockify WebSocket."""

    def __init__(self, reader: Any = None, writer: Any = None, close: Optional[Callable[[], Any]] = None,
                 websocket: Any = None):
        self.reader, self.writer, self._close, self.websocket = reader, writer, close, websocket
        self.buffer = b""

    async def read(self) -> bytes:
        if self.buffer:
            data, self.buffer = self.buffer, b""
            return data
        if self.websocket is not None:
            try:
                message = await self.websocket.recv()
            except Exception:
                return b""
            return message if isinstance(message, bytes) else message.encode()
        return await self.reader.read(65536)

    async def readexactly(self, n: int) -> bytes:
        while len(self.buffer) < n:
            chunk = await (self.websocket.recv() if self.websocket is not None else self.reader.read(65536))
            if not chunk:
                raise ConnectionError("desktop closed the connection")
            self.buffer += chunk if isinstance(chunk, bytes) else chunk.encode()
        data, self.buffer = self.buffer[:n], self.buffer[n:]
        return data

    async def write(self, data: bytes) -> None:
        if self.websocket is not None:
            await self.websocket.send(data)
        else:
            self.writer.write(data)
            await self.writer.drain()

    async def close(self) -> None:
        with contextlib.suppress(Exception):
            if self.websocket is not None:
                await self.websocket.close()
            elif self._close is not None:
                self._close()


VNCPASSWD_KEY = bytes([23, 82, 107, 6, 35, 78, 88, 7])   # the fixed key vncpasswd files are obscured with


def _des(key: bytes, data: bytes, decrypt: bool = False) -> bytes:
    from cryptography.hazmat.decrepit.ciphers.algorithms import TripleDES
    from cryptography.hazmat.primitives.ciphers import Cipher, modes

    cipher = Cipher(TripleDES(key * 3), modes.ECB())   # three equal keys: plain DES
    ctx = cipher.decryptor() if decrypt else cipher.encryptor()
    return ctx.update(data) + ctx.finalize()


def _reverse_bits(data: bytes) -> bytes:
    return bytes(int(f"{b:08b}"[::-1], 2) for b in data)


def read_vnc_password(path: str) -> str:
    """The password in clear, or a `vncpasswd` file (8 bytes obscured with a fixed, public key: not a protection,
    this file must stay out of the agents' reach like any other key)."""
    raw = Path(path).read_bytes()
    try:
        text = raw.decode("utf-8").strip()
        if text and text.isprintable():
            return text
    except UnicodeDecodeError:
        pass
    plain = _des(_reverse_bits(VNCPASSWD_KEY), raw[:8], decrypt=True)
    return plain.split(b"\0", 1)[0].decode("latin-1")


def vnc_response(password: str, challenge: bytes) -> bytes:
    """RFB « VNC Authentication »: the challenge encrypted with DES, keyed by the password's first 8 bytes with each
    byte's bits reversed (the protocol's quirk)."""
    key = password.encode("latin-1", "replace")[:8].ljust(8, b"\0")
    return _des(_reverse_bits(key), challenge)


async def vnc_login(up: Upstream, password: Optional[str]) -> None:
    """The client side of an RFB 3.8 handshake with the desktop's server, up to (not including) ClientInit."""
    version = await up.readexactly(12)
    if not version.startswith(b"RFB 003.") or int(version[8:11]) < 7:
        raise ConnectionError(f"unsupported VNC server {version!r}")
    await up.write(b"RFB 003.008\n")
    count = (await up.readexactly(1))[0]
    if count == 0:
        size = struct.unpack(">I", await up.readexactly(4))[0]
        raise ConnectionError((await up.readexactly(size)).decode(errors="replace"))
    offered = await up.readexactly(count)
    if 2 in offered and password:
        await up.write(b"\x02")
        await up.write(vnc_response(password, await up.readexactly(16)))
    elif 1 in offered:
        await up.write(b"\x01")
    else:
        raise ConnectionError("the VNC server wants a password the bridge does not have")
    if struct.unpack(">I", await up.readexactly(4))[0] != 0:
        raise ConnectionError("VNC password refused")


class Downstream:
    """The app's side (FastAPI WebSocket), read in exact sizes for the handshake the bridge answers itself."""

    def __init__(self, websocket: WebSocket):
        self.websocket = websocket
        self.buffer = b""

    async def readexactly(self, n: int) -> bytes:
        while len(self.buffer) < n:
            message = await self.websocket.receive()
            if message["type"] == "websocket.disconnect":
                raise WebSocketDisconnect()
            self.buffer += message.get("bytes") or (message.get("text") or "").encode()
        data, self.buffer = self.buffer[:n], self.buffer[n:]
        return data


async def serve_handshake(app_side: Downstream, websocket: WebSocket, up: Upstream) -> None:
    """To the app: no password (it passed Face ID and holds a ticket). Its ClientInit goes on to the server, shared."""
    await websocket.send_bytes(b"RFB 003.008\n")
    await app_side.readexactly(12)
    await websocket.send_bytes(b"\x01\x01")             # one security type: None
    await app_side.readexactly(1)
    await websocket.send_bytes(b"\x00\x00\x00\x00")   # SecurityResult: OK
    await app_side.readexactly(1)                     # ClientInit
    await up.write(b"\x01")                           # shared: never kick another viewer off


async def open_vnc(agent: AgentConfig) -> Upstream:
    url = agent.desktop_vnc or ""
    password = None
    if agent.desktop_vnc_password_file:
        password = read_vnc_password(agent.desktop_vnc_password_file)
    if url.startswith(("ws://", "wss://")):
        from websockets.asyncio.client import connect
        up = Upstream(websocket=await connect(url, subprotocols=["binary"], max_size=None, open_timeout=10))
    elif url.startswith("tcp://"):
        host, _, port = url[len("tcp://"):].rpartition(":")
        reader, writer = await asyncio.wait_for(asyncio.open_connection(host, int(port)), timeout=10)
        up = Upstream(reader, writer, writer.close)
    else:
        raise ValueError("desktop_vnc must be ws://… or tcp://host:port")
    try:
        await vnc_login(up, password)
    except Exception:
        await up.close()
        raise
    return up


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


PAGE = r"""<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,maximum-scale=1,user-scalable=no,viewport-fit=cover">
<style>html,body{margin:0;height:100%;background:#111;overflow:hidden;-webkit-user-select:none;user-select:none}
#screen{position:absolute;inset:0;touch-action:none}#screen.keys{bottom:calc(52px + env(safe-area-inset-bottom))}
#state{position:fixed;top:calc(8px + env(safe-area-inset-top));left:50%;transform:translateX(-50%);
font:600 13px -apple-system,sans-serif;color:#fff;background:rgba(0,0,0,.55);padding:4px 10px;border-radius:12px}
#bar{position:fixed;left:0;right:0;bottom:0;display:none;gap:6px;overflow-x:auto;background:#1c1c1e;
padding:8px calc(8px + env(safe-area-inset-right)) calc(8px + env(safe-area-inset-bottom)) calc(8px + env(safe-area-inset-left));
-webkit-overflow-scrolling:touch;scrollbar-width:none}#bar::-webkit-scrollbar{display:none}
#bar button{flex:0 0 auto;min-width:40px;height:36px;padding:0 12px;border:0;border-radius:9px;background:#3a3a3c;color:#fff;
font:600 14px -apple-system,sans-serif}#bar button.main{background:#fff;color:#000}
#typing{position:fixed;left:-1000px;top:0;opacity:0;width:1px;height:1px}</style></head>
<body><div id="screen"></div><div id="state">Connexion…</div>
<div id="bar">
<button id="kbd" class="main">⌨︎ Clavier</button><button id="paste">Coller</button>
<button data-key="Enter">⏎</button><button data-key="Tab">⇥</button><button data-key="Escape">Échap</button>
<button data-key="ArrowLeft">←</button><button data-key="ArrowUp">↑</button><button data-key="ArrowDown">↓</button>
<button data-key="ArrowRight">→</button><button data-key="PageUp">Page ↑</button><button data-key="PageDown">Page ↓</button>
</div>
<textarea id="typing" autocapitalize="off" autocomplete="off" autocorrect="off" spellcheck="false"></textarea>
<script type="module">
import RFB from "../static/novnc/core/rfb.js";
const $ = id => document.getElementById(id);
const state = $("state"), bar = $("bar"), typing = $("typing"), screenEl = $("screen");
const url = (location.protocol === "https:" ? "wss://" : "ws://") + location.host + location.pathname.replace(/\/$/, "") + "/ws";
const rfb = new RFB(screenEl, url, {shared: true});
rfb.scaleViewport = true; rfb.resizeSession = false; rfb.viewOnly = __VIEW_ONLY__;
rfb.addEventListener("connect", () => { state.style.display = "none"; post("connected"); });
rfb.addEventListener("disconnect", () => { state.style.display = "block"; state.textContent = "Bureau fermé"; post("disconnected"); });
function post(event) { try { window.webkit.messageHandlers.desktop.postMessage(event); } catch (_) {} }
function show() {
  bar.style.display = rfb.viewOnly ? "none" : "flex";
  screenEl.classList.toggle("keys", !rfb.viewOnly);
  window.dispatchEvent(new Event("resize"));
}
window.setViewOnly = on => { rfb.viewOnly = on; if (on) typing.blur(); show(); };
show();

// ---- Keys: iOS opens its keyboard only for a field the person taps; what is typed there becomes keys.
const KEYS = {Backspace: 0xff08, Enter: 0xff0d, Tab: 0xff09, Escape: 0xff1b, ArrowLeft: 0xff51, ArrowUp: 0xff52,
              ArrowRight: 0xff53, ArrowDown: 0xff54, PageUp: 0xff55, PageDown: 0xff56};
const keysym = ch => { const c = ch.codePointAt(0); return c < 0x100 ? c : 0x01000000 + c; };
const typeText = text => { for (const ch of text) ch === "\n" ? rfb.sendKey(KEYS.Enter, "Enter") : rfb.sendKey(keysym(ch)); };
window.typeText = typeText;
$("kbd").addEventListener("click", () => typing.focus());
$("paste").addEventListener("click", async () => {
  try { typeText(await navigator.clipboard.readText()); } catch (_) { post("paste-refused"); }
});
for (const b of bar.querySelectorAll("[data-key]")) b.addEventListener("click", () => rfb.sendKey(KEYS[b.dataset.key], b.dataset.key));
typing.addEventListener("input", e => {
  if (e.inputType === "deleteContentBackward") rfb.sendKey(KEYS.Backspace, "Backspace");
  else typeText(typing.value);
  typing.value = "";
});
typing.addEventListener("keydown", e => {
  if (KEYS[e.key] && !(e.key === "Backspace" && typing.value === "")) return;
  if (KEYS[e.key]) { e.preventDefault(); rfb.sendKey(KEYS[e.key], e.key); }
});

// ---- Zoom and pan on the phone (noVNC's clipped viewport: a click lands where it is drawn, zoomed or not).
const display = () => rfb._display, canvas = () => rfb._canvas;
let zoom = 0;   // 0: the whole screen fits
function fit() { const d = display(); return d.width ? Math.min(screenEl.clientWidth / d.width, screenEl.clientHeight / d.height) : 1; }
function local(x, y) { const r = canvas().getBoundingClientRect(); return {x: x - r.left, y: y - r.top}; }
function setZoom(z, cx, cy) {
  const d = display();
  if (!d.width) return;
  const at = local(cx, cy), fx = d.absX(at.x), fy = d.absY(at.y);   // the remote pixel under the fingers
  z = Math.min(Math.max(z, fit()), 4);
  if (z <= fit() * 1.03) { zoom = 0; rfb.clipViewport = false; rfb.scaleViewport = true; return; }
  rfb.scaleViewport = false; rfb.clipViewport = true;
  d.viewportChangeSize(screenEl.clientWidth / z, screenEl.clientHeight / z);
  d.scale = z;
  zoom = z;
  const now = local(cx, cy), vp = d._viewportLoc;
  d.viewportChangePos(fx - now.x / z - vp.x, fy - now.y / z - vp.y);   // that pixel stays under the fingers
}
function pan(dx, dy) { if (zoom) display().viewportChangePos(-dx / zoom, -dy / zoom); }
window.addEventListener("resize", () => {   // rotation: noVNC fits again, then the zoom comes back
  if (!zoom) return;
  const z = zoom, r = screenEl.getBoundingClientRect();
  requestAnimationFrame(() => requestAnimationFrame(() => setZoom(z, r.left + r.width / 2, r.top + r.height / 2)));
});

// ---- Touch: one finger pans (watching) or clicks and drags (with the hand); two fingers pinch to zoom, and move
// the zoomed view, or scroll the page when the whole screen shows; a long press is a right click.
const mouse = (x, y, mask) => { const p = local(x, y); rfb._sendMouse(p.x, p.y, mask); };
const dist = (a, b) => Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY);
const mid = (a, b) => ({x: (a.clientX + b.clientX) / 2, y: (a.clientY + b.clientY) / 2});
let one = null, two = null, lastTap = 0;
function start(e) {
  e.preventDefault(); e.stopPropagation();
  if (e.touches.length >= 2) {
    if (one && one.dragging && !rfb.viewOnly) mouse(one.x, one.y, 0);
    clearTimeout(one && one.timer); one = null;
    const [a, b] = e.touches;
    two = {d0: dist(a, b), z0: zoom || fit(), last: mid(a, b), pinching: false, scrolled: 0};
    return;
  }
  const t = e.touches[0];
  one = {x0: t.clientX, y0: t.clientY, x: t.clientX, y: t.clientY, moved: false, dragging: false, long: false};
  if (!rfb.viewOnly) one.timer = setTimeout(() => {   // long press: right click
    if (one && !one.moved) { one.long = true; mouse(one.x, one.y, 4); mouse(one.x, one.y, 0); }
  }, 600);
}
function move(e) {
  e.preventDefault(); e.stopPropagation();
  if (two && e.touches.length >= 2) {
    const [a, b] = e.touches, m = mid(a, b), d = dist(a, b);
    if (!two.pinching && Math.abs(d - two.d0) > 24) two.pinching = true;
    if (two.pinching) setZoom(two.z0 * d / two.d0, m.x, m.y);
    if (zoom) pan(m.x - two.last.x, m.y - two.last.y);
    else if (!rfb.viewOnly && !two.pinching) {   // whole screen shown: two fingers scroll the page
      two.scrolled += m.y - two.last.y;
      while (Math.abs(two.scrolled) >= 24) {
        const up = two.scrolled > 0, mask = up ? 8 : 16;
        mouse(m.x, m.y, mask); mouse(m.x, m.y, 0);
        two.scrolled += up ? -24 : 24;
      }
    }
    two.last = m;
    return;
  }
  if (!one) return;
  const t = e.touches[0], dx = t.clientX - one.x, dy = t.clientY - one.y;
  if (!one.moved && Math.hypot(t.clientX - one.x0, t.clientY - one.y0) > 8) { one.moved = true; clearTimeout(one.timer); }
  if (rfb.viewOnly) pan(dx, dy);
  else if (one.moved && !one.long) {
    if (!one.dragging) { one.dragging = true; mouse(one.x0, one.y0, 1); }
    mouse(t.clientX, t.clientY, 1);
  }
  one.x = t.clientX; one.y = t.clientY;
}
function end(e) {
  e.preventDefault(); e.stopPropagation();
  if (two) { if (e.touches.length === 0) two = null; return; }
  if (!one) return;
  clearTimeout(one.timer);
  const tap = !one.moved && !one.long;
  if (!rfb.viewOnly) {
    if (one.dragging) mouse(one.x, one.y, 0);
    else if (tap) { mouse(one.x, one.y, 1); mouse(one.x, one.y, 0); }
  } else if (tap) {   // watching: a double tap zooms in there, or back out
    const now = Date.now();
    if (now - lastTap < 300) { setZoom(zoom ? 0 : fit() * 2.5, one.x, one.y); lastTap = 0; } else lastTap = now;
  }
  one = null;
}
for (const [type, handler] of [["touchstart", start], ["touchmove", move], ["touchend", end], ["touchcancel", end]])
  screenEl.addEventListener(type, handler, {capture: true, passive: false});
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
        holds = (services.desktop_control.get(name, False) if agent and agent.desktop_vnc
                 else folder is not None and app_holds(folder))
        view_only = "false" if holds else "true"
        return HTMLResponse(PAGE.replace("__VIEW_ONLY__", view_only), headers={"Cache-Control": "no-store"})

    @router.websocket("/desktop/{ticket}/ws")
    async def stream(websocket: WebSocket, ticket: str) -> None:
        services = websocket.app.state.services
        name = services.desktop_tickets.agent(ticket)
        agent = services.config.agent(name) if name else None
        folder = desktop_dir(services.config, agent) if agent and not agent.desktop_vnc else None
        if agent is None or (folder is None and not agent.desktop_vnc):
            await websocket.close(code=4401)
            return
        try:
            if agent.desktop_vnc:
                up = await open_vnc(agent)
            else:
                assert folder is not None
                reader, writer, close = await open_rfb(folder)
                up = Upstream(reader, writer, close)
        except (OSError, ValueError, ConnectionError, asyncio.TimeoutError) as exc:
            log.info("desktop unavailable", extra=fields(agent=name, error=str(exc)[:200]))
            await websocket.close(code=4001)
            return
        await websocket.accept(subprotocol="binary" if "binary" in websocket.scope.get("subprotocols", []) else None)
        app_side = Downstream(websocket)
        if agent.desktop_vnc:
            try:
                await serve_handshake(app_side, websocket, up)
            except (WebSocketDisconnect, ConnectionError):
                await up.close()
                return
            gate = InputGate(lambda: services.desktop_control.get(agent.name, False))
            gate.handshake = 0
        else:
            gate = InputGate(lambda: app_holds(folder))
        last_touch = 0.0

        async def down() -> None:
            nonlocal last_touch
            while data := await up.read():
                await websocket.send_bytes(data)
                if folder is not None and time.time() - last_touch > 60:   # watched: Hermes keeps the desktop up
                    last_touch = time.time()
                    with contextlib.suppress(OSError):
                        (folder / "activity").touch()

        async def from_app() -> None:
            if app_side.buffer:
                pending, app_side.buffer = app_side.buffer, b""
                if out := gate.feed(pending):
                    await up.write(out)
            while True:
                message = await websocket.receive()
                if message["type"] == "websocket.disconnect":
                    return
                out = gate.feed(message.get("bytes") or (message.get("text") or "").encode())
                if out:
                    await up.write(out)

        log.info("desktop viewer connected", extra=fields(agent=name))
        tasks = [asyncio.create_task(down()), asyncio.create_task(from_app())]
        try:
            await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
        except (WebSocketDisconnect, ValueError, ConnectionError):
            pass
        finally:
            for task in tasks:
                task.cancel()
            await up.close()
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
        if agent.desktop_vnc:
            from .browsers import ports_of
            ticket = services.desktop_tickets.new(agent.name)
            # Its own browser container (browsers.py): taking over makes its tools wait. Set up by hand: it does not.
            return {"kind": "novnc", "path": f"desktop/{ticket}/", "you_hold": services.desktop_control.get(agent.name, False),
                    "holder": "human" if services.desktop_control.get(agent.name) else "agent", "reason": None,
                    "pauses_agent": ports_of(services.config, agent.name) is not None}
        if agent.desktop_url:
            return {"kind": "page", "url": agent.desktop_url}
        folder = desktop_dir(services.config, agent)
        if folder is None or not ((folder / "rfb.sock").exists() or (folder / "sandbox.json").exists()):
            raise HTTPException(status_code=409, detail={"code": "no_desktop"})
        ticket = services.desktop_tickets.new(agent.name)
        lease = _read_lease(folder)
        return {"kind": "novnc", "path": f"desktop/{ticket}/", "you_hold": app_holds(folder),
                "holder": lease.get("holder"), "reason": lease.get("reason") or None, "pauses_agent": True}

    @router.post("/agents/{agent_id}/desktop/control")
    async def control(agent_id: str, request: Request) -> Dict[str, Any]:
        services = request.app.state.services
        agent = services.config.agent(agent_id)
        body = await request.json()
        if agent is not None and agent.desktop_vnc:
            # No Hermes lease there: taking over lets the person's input through, and in the agent's own browser
            # container makes its tools wait (browsers.hold) until the person hands it back.
            from .browsers import hold
            take = bool(body.get("take"))
            services.desktop_control[agent.name] = take
            await hold(services.config, agent.name, take)
            log.info("desktop control", extra=fields(agent=agent.name, holder="human" if take else "agent"))
            return {"holder": "human" if take else "agent", "you_hold": take}
        folder = desktop_dir(services.config, agent) if agent else None
        if folder is None:
            raise HTTPException(status_code=404, detail="unknown agent")
        lease = set_lease(folder, bool(body.get("take")), reason="Depuis l'app BipAgents")
        log.info("desktop control", extra=fields(agent=agent.name, holder=lease["holder"]))
        return {"holder": lease["holder"], "you_hold": app_holds(folder)}

    return router


def static_files() -> StaticFiles:
    return StaticFiles(directory=str(Path(__file__).with_name("static") / "novnc"))
