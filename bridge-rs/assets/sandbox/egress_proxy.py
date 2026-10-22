"""The agents' sandboxes' only way out (bridge-rs/src/sandbox.rs): an HTTP proxy that lets through the official
package registries and nothing else. The sandboxes sit on a network without a route out; this container is on it and
on the ordinary one. HTTPS goes through CONNECT, to port 443 of an allowed host only; plain HTTP (Debian's apt) to
port 80 of an allowed host only. Each decision is one line of its log (`podman logs bipagents-egress`).

Run by the bridge with `python3 -u -c <this file>`: standard library only, no file of its own on the host.
"""
import asyncio
import os
import time

ALLOW = {h.strip().lower().rstrip(".") for h in os.environ.get("BIP_ALLOW", "").split(",") if h.strip()}
PORT = int(os.environ.get("BIP_PORT", "3128"))
WAIT = 30


def allowed(host, port, wanted):
    return port == wanted and host.strip().lower().rstrip(".") in ALLOW


def log(verdict, method, target):
    print(f"{time.strftime('%Y-%m-%dT%H:%M:%S')} {verdict} {method} {target[:200]}", flush=True)


async def pipe(reader, writer):
    try:
        while True:
            data = await reader.read(65536)
            if not data:
                break
            writer.write(data)
            await writer.drain()
    except Exception:
        pass
    finally:
        try:
            writer.close()
        except Exception:
            pass


async def answer(writer, status, reason):
    body = (f"{status} {reason}: from this sandbox, only the official package registries can be reached "
            "(PyPI, npm…).\n").encode()
    writer.write(f"HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\n"
                 f"Content-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode() + body)
    try:
        await writer.drain()
    except Exception:
        pass
    writer.close()


def split_authority(authority, default_port):
    if authority.startswith("["):
        host, _, rest = authority[1:].partition("]")
        port = rest[1:] if rest.startswith(":") else str(default_port)
    elif ":" in authority:
        host, port = authority.rsplit(":", 1)
    else:
        host, port = authority, str(default_port)
    return host, int(port) if port.isdigit() else -1


async def handle(reader, writer):
    try:
        head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), WAIT)
    except Exception:
        writer.close()
        return
    first, _, rest = head.partition(b"\r\n")
    parts = first.decode("latin-1").split()
    if len(parts) != 3:
        return await answer(writer, 400, "Bad Request")
    method, target, version = parts
    if method.upper() == "CONNECT":
        host, port = split_authority(target, 443)
        wanted = 443
        request = b""
    elif target.lower().startswith("http://"):
        authority, _, path = target[7:].partition("/")
        host, port = split_authority(authority, 80)
        wanted = 80
        request = f"{method} /{path} {version}\r\n".encode("latin-1") + rest
    else:
        return await answer(writer, 400, "Bad Request")
    if not allowed(host, port, wanted):
        log("blocked", method, target)
        return await answer(writer, 403, "Forbidden")
    try:
        up_reader, up_writer = await asyncio.wait_for(asyncio.open_connection(host, port), WAIT)
    except Exception:
        log("failed", method, target)
        return await answer(writer, 502, "Bad Gateway")
    log("allowed", method, target)
    if request:
        up_writer.write(request)
    else:
        writer.write(b"HTTP/1.1 200 Connection established\r\n\r\n")
    await asyncio.gather(pipe(reader, up_writer), pipe(up_reader, writer))


async def main():
    server = await asyncio.start_server(handle, "0.0.0.0", PORT)
    print(f"registries' proxy on :{PORT}, allowed: {', '.join(sorted(ALLOW))}", flush=True)
    async with server:
        await server.serve_forever()


asyncio.run(main())
