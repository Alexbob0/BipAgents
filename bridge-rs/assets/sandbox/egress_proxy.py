"""The agents' sandboxes' only way out (bridge-rs/src/sandbox.rs, sandbox_sites.rs): an HTTP proxy that lets through
the official package registries, and for an agent that gives its own credentials, the sites its person allowed it.
The sandboxes sit on a network without a route out; this container is on it and on the ordinary one. HTTPS goes
through CONNECT, to port 443 of an allowed host only; plain HTTP (Debian's apt) to port 80 of a registry only. Each
decision is one line of its log (`podman logs bipagents-egress`).

- **Who asks**: each isolated sandbox has its own proxy URL, `http://<agent>:<token>@…` (pip, npm, uv, curl, apt…
  send it as `Proxy-Authorization: Basic`). The bridge writes `BIP_POLICY` (read-only here): per agent, the SHA-256
  of its token and its allowed hosts (`data.gouv.fr`, or `*.example.org` for its subdomains), read again when it
  changes. No credentials, or wrong ones: the registries only (a 407 asks a client that waits for it to send them).
- **A site refused** to an agent known by its credentials: one JSON line appended to `BIP_REPORTS` (the bridge reads
  and empties it, and asks the person), once an hour per agent and host at most.
- **Never this machine nor its networks**: a name that resolves to a private, loopback, link-local, shared (Tailscale
  100.64/10) or reserved address is refused, the registries included; the connection goes to the address checked,
  never resolved again. The `Proxy-Authorization` header is never passed on.

Run by the bridge with `python3 -u -c <this file>`: standard library only.
"""
import asyncio
import base64
import hashlib
import hmac
import ipaddress
import json
import os
import socket
import time

REGISTRIES = {h.strip().lower().rstrip(".") for h in os.environ.get("BIP_ALLOW", "").split(",") if h.strip()}
PORT = int(os.environ.get("BIP_PORT", "3128"))
POLICY = os.environ.get("BIP_POLICY", "")
REPORTS = os.environ.get("BIP_REPORTS", "")
WAIT = 30
REPORT_EVERY = 3600
REPORTS_MAX = 256 * 1024

_policy = {"stamp": None, "agents": {}}
_reported = {}


def normal(host):
    return host.strip().lower().rstrip(".")


def policy():
    """The agents' keys and hosts, read again when the file changed (a missing or broken file: none)."""
    if not POLICY:
        return {}
    try:
        info = os.stat(POLICY)
        stamp = (info.st_mtime_ns, info.st_size, info.st_ino)
    except OSError:
        _policy.update(stamp=None, agents={})
        return {}
    if stamp != _policy["stamp"]:
        try:
            with open(POLICY, encoding="utf-8") as f:
                data = json.load(f)
            agents = data.get("agents") if isinstance(data, dict) else None
            _policy.update(stamp=stamp, agents=agents if isinstance(agents, dict) else {})
        except (OSError, ValueError):
            _policy.update(stamp=stamp, agents={})
    return _policy["agents"]


def who(header):
    """The agent whose credentials these are (`Basic base64(agent:token)`), else None."""
    if not header:
        return None
    scheme, _, value = header.strip().partition(" ")
    if scheme.lower() != "basic":
        return None
    try:
        name, _, token = base64.b64decode(value.strip(), validate=True).decode("utf-8").partition(":")
    except (ValueError, UnicodeDecodeError):
        return None
    entry = policy().get(name)
    if not isinstance(entry, dict) or not token or not isinstance(entry.get("key"), str):
        return None
    key = hashlib.sha256(token.encode("utf-8")).hexdigest()
    return name if hmac.compare_digest(key, entry["key"]) else None


def host_matches(host, pattern):
    pattern = normal(pattern)
    if pattern.startswith("*."):
        base = pattern[2:]
        return host.endswith("." + base) and len(host) > len(base) + 1
    return host == pattern


def allowed(host, port, wanted, agent=None):
    """Registries (HTTPS, and plain HTTP for apt); an agent's own hosts on HTTPS only."""
    host = normal(host)
    if port != wanted:
        return False
    if host in REGISTRIES:
        return True
    if agent is None or wanted != 443:
        return False
    hosts = (policy().get(agent) or {}).get("hosts") or []
    return any(isinstance(p, str) and host_matches(host, p) for p in hosts)


def public(address):
    """An address of the internet, not of this machine or a network it is on."""
    try:
        ip = ipaddress.ip_address(address.split("%", 1)[0])
    except ValueError:
        return False
    if ip.version == 6 and ip.ipv4_mapped:
        ip = ip.ipv4_mapped
    if ip.version == 4 and ip in ipaddress.ip_network("100.64.0.0/10"):
        return False   # shared address space: Tailscale's addresses, a carrier's NAT
    return ip.is_global and not (ip.is_private or ip.is_loopback or ip.is_link_local or ip.is_multicast
                                 or ip.is_reserved or ip.is_unspecified)


def literal(host):
    try:
        ipaddress.ip_address(host.strip("[]"))
        return True
    except ValueError:
        return False


async def resolve(host, port):
    """The host's public addresses (none: refused). An address literal is never let through."""
    if literal(host):
        return []
    loop = asyncio.get_running_loop()
    try:
        found = await asyncio.wait_for(loop.getaddrinfo(host, port, type=socket.SOCK_STREAM), WAIT)
    except Exception:
        return None
    addresses = [info[4][0] for info in found]
    if not addresses or not all(public(a) for a in addresses):
        return []
    return addresses


def log(verdict, method, target, agent=None):
    who = f" [{agent}]" if agent else ""
    print(f"{time.strftime('%Y-%m-%dT%H:%M:%S')} {verdict}{who} {method} {target[:200]}", flush=True)


def report(agent, host, port):
    """A site refused to a known agent, for the bridge to ask its person (once an hour per agent and host)."""
    if not REPORTS or agent is None or port != 443:
        return
    key = (agent, host)
    now = time.time()
    if now - _reported.get(key, 0) < REPORT_EVERY:
        return
    _reported[key] = now
    if len(_reported) > 4096:
        _reported.clear()
    line = json.dumps({"agent": agent, "host": host[:253], "at": int(now)}) + "\n"
    try:
        if os.path.exists(REPORTS) and os.path.getsize(REPORTS) > REPORTS_MAX:
            return   # the bridge isn't reading them: nothing more
        fd = os.open(REPORTS, os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW, 0o600)
        try:
            os.write(fd, line.encode("utf-8"))
        finally:
            os.close(fd)
    except OSError:
        pass


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


async def answer(writer, status, reason, extra=""):
    body = (f"{status} {reason}: from this sandbox, only the package registries and the sites the person allowed "
            "can be reached.\n").encode()
    writer.write(f"HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\n{extra}"
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


def headers_of(rest):
    """The request's header lines (name, raw line), without the blank end."""
    lines = rest.split(b"\r\n")
    out = []
    for line in lines:
        if not line:
            continue
        name = line.split(b":", 1)[0].strip().lower().decode("latin-1")
        out.append((name, line))
    return out


async def connect(addresses, port):
    last = None
    for address in addresses:
        try:
            return await asyncio.wait_for(asyncio.open_connection(address, port), WAIT)
        except Exception as exc:
            last = exc
    raise last or OSError("no address")


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
    headers = headers_of(rest)
    auth = next((line.split(b":", 1)[1].decode("latin-1") for name, line in headers if name == "proxy-authorization"),
                "")
    agent = who(auth)
    if method.upper() == "CONNECT":
        host, port = split_authority(target, 443)
        wanted = 443
        request = b""
    elif target.lower().startswith("http://"):
        authority, _, path = target[7:].partition("/")
        host, port = split_authority(authority, 80)
        wanted = 80
        kept = b"".join(line + b"\r\n" for name, line in headers if name not in ("proxy-authorization",
                                                                                   "proxy-connection"))
        request = f"{method} /{path} {version}\r\n".encode("latin-1") + kept + b"\r\n"
    else:
        return await answer(writer, 400, "Bad Request")
    host = normal(host)
    if not allowed(host, port, wanted, agent):
        if agent is None and not auth and wanted == 443 and port == 443:
            # a client that sends its credentials only when asked (the registries need none)
            log("ask-credentials", method, target)
            return await answer(writer, 407, "Proxy Authentication Required",
                                'Proxy-Authenticate: Basic realm="bipagents"\r\n')
        log("blocked", method, target, agent)
        report(agent, host, port)
        return await answer(writer, 403, "Forbidden")
    addresses = await resolve(host, port)
    if not addresses:
        log("blocked-address" if addresses == [] else "failed", method, target, agent)
        return await answer(writer, 403 if addresses == [] else 502, "Forbidden" if addresses == [] else "Bad Gateway")
    try:
        up_reader, up_writer = await connect(addresses, port)
    except Exception:
        log("failed", method, target, agent)
        return await answer(writer, 502, "Bad Gateway")
    log("allowed", method, target, agent)
    if request:
        up_writer.write(request)
    else:
        writer.write(b"HTTP/1.1 200 Connection established\r\n\r\n")
    await asyncio.gather(pipe(reader, up_writer), pipe(up_reader, writer))


async def main():
    server = await asyncio.start_server(handle, "0.0.0.0", PORT)
    print(f"sandboxes' proxy on :{PORT}, registries: {', '.join(sorted(REGISTRIES))}; agents' sites from "
          f"{POLICY or 'nowhere'}", flush=True)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main())
