"""The agents' only way out (bridge-rs/src/sandbox.rs, sandbox_sites.rs, send_guard.rs): an HTTP proxy that lets every
agent READ any public site and guards what it SENDS (owner decision 2026-10-28, « bloquer l'envoi d'informations sans
validation »). The sandboxes sit on a network without a route out; this container is on it and on the ordinary one.
HTTPS goes through CONNECT, to port 443; plain HTTP (Debian's apt, a browser's http:// page) as an absolute URI to
port 80. Each decision is one line of its log (`podman logs bipagents-egress`), each tunnel's byte counts too, so an
abuse complaint can be traced to an agent.

- **Who asks**: each isolated sandbox has its own proxy URL, `http://<agent>:<token>@…` (pip, npm, uv, curl, apt…
  send it as `Proxy-Authorization: Basic`). The bridge writes `BIP_POLICY` (read-only here): per agent, the SHA-256
  of its token and the hosts its person validated for sending (`data.gouv.fr`, or `*.example.org`), read again when it
  changes. An agent's visible browser is known by its address on the browsers' network (`BIP_BROWSERS`: this proxy's
  address there; the policy's `browsers` maps each browser's address to its agent). No credentials, or wrong ones:
  the registries only (a 407 asks a client that waits for it to send them).
- **Reading is open**: an agent reaches any public name on HTTPS (its browser on plain HTTP too) — never an address,
  nor a name of this machine's networks, nor one that resolves to a private, loopback, link-local, shared (Tailscale
  100.64/10) or reserved address; the connection goes to the address checked, never resolved again. The
  `Proxy-Authorization` header is never passed on.
- **Sending is guarded**: the tunnel is opaque (never any TLS interception), so the proxy counts what the client sends
  the site: a program's every byte (its TLS handshake too: a program frames data in any record type it likes), the
  browser's TLS records past their first `browser_record_free` bytes each, whatever their type (those free bytes
  within a pool, `browser_free_site_hour` per host and `browser_free_day` per agent: past it they count); and the
  name of each host not validated it connects to, once a day per name (data can be spelt in names). To a host its
  person did not validate (the registries count as validated: they have their own ceiling, `registry_hour` and
  `registry_day` per agent, never a card), past `send_site_hour` bytes per host per hour, or `send_day` bytes per
  agent per day across such hosts (a slow leak), the tunnel is cut: nothing more reaches the site, the proxy keeps
  reading what the client still sends for a few seconds to know its size, then
  appends one line `{"kind": "send", "agent", "host", "bytes"}` to `BIP_REPORTS` (the bridge reads it and asks the
  person on ONE card per agent). Until the person answers, that host (or, past the day's total, every host not
  validated) is held: it still opens for reading, within small allowances that come back every hour, shared by all
  its tunnels: `held_site` bytes per host and `held_hour` bytes per agent across held hosts; past one, the tunnel is
  cut and a program's new tunnels to it are refused (403) until the hour turns. A host the person refused (the
  policy's `refused`, host → until when: « Refuser », 24 hours; `refused_always`: « Refuser toujours ») is held the
  same way, never reported: readable, never refused outright. The policy's `reset` (the person's last « Autoriser »)
  clears an agent's counts.
- **Worst case with no card**, per agent per UTC day, for its programs and its browser's counted bytes together:
  the day's `send_day` (256 KB), then `held_hour` (64 KB) an hour for the rest of the day: at most 256 KB + 24 x 64 KB
  = 1.75 MB (the first card is raised at the 256 KB). On top of it the browser's free small records (its pool) and
  what goes to the package registries (their ceiling), each said where it is set.
- **Where the counts live**: in this process's memory, written to `counts.json` next to `BIP_USAGE` when they moved
  (every few seconds at most, at each cut and at a clean stop), and read back at start the same day: a restart of the
  proxy hands out no new allowance (11/11 audit R-1). At most `MAX_OPEN_PER_CLIENT` connections per client address and
  `MAX_OPEN_TOTAL` in all, a request head of at most `HEADER_LIMIT` bytes, and a per-tunnel write buffer of at most
  `WRITE_BUFFER` (the proxy stops reading one side when the other does not read), so a sandbox or a flooding site
  cannot run the proxy out of memory.
- **On the offer's servers** (limits the bridge sets, 0 = none): `download_day` bytes received per agent per day,
  `connections_minute` new connections per agent per minute, and `BIP_BLOCKLIST` (piracy and torrent trackers' names,
  one per line, refused with their subdomains). Each refusal says so plainly.
- **Connectors** (bridge-rs/src/connectors_run.rs): a policy entry with `"connector": true` (`conn-<id>`) reaches its
  approved hosts only, on HTTPS — never the registries, never another site — and is not counted (its sites were
  approved with it). A connection that arrives on one of `BIP_STRICT`'s addresses (this proxy's address on the
  connectors' network) is held to that: a connector's own credentials or nothing.
- **Refusals read plainly**: the reason phrase says what happens: clients show it in their error, so the agent tells
  its person what it waits for, not that « the proxy » cuts it.
- **What it counted** (`BIP_USAGE`, written each minute): per agent, today's bytes sent and received, and new
  connections; the bridge shows them.
- **The phone exit** (bridge-rs/src/phone_exit/, docs/phone-exit.md; only with `BIP_EXIT_SOCK`): an agent's HTTPS
  to a site of its policy entry's `phone` list (`[{"site": "ameli.fr", "minimal": true}]`: the site and its
  subdomains; with `minimal`, its static-file hosts such as `assets.` or `img.` still go direct) goes to the bridge's
  hub over that unix socket instead of out of this machine (« EXIT host 443 agent », then « OK » and the raw stream,
  or « ERR reason »). The hub and the phone check it again (the sessions, the name, the address the phone resolves,
  the ClientHello's server name). Never the datacenter instead: no phone, the tunnel is refused, its reason said.
  Counted and guarded like any other tunnel. When a site starts or stops going through the phone, the tunnels to it
  still open the other way are closed (`switch_tunnels`, every second): Chrome reuses an open connection, which
  would otherwise keep showing the datacenter's challenge after the person accepted (lab, 6/11).

Run by the bridge with `python3 -u -c <this file>`: standard library only.
"""
import asyncio
import base64
import hashlib
import hmac
import ipaddress
import json
import os
import signal
import socket
import time

REGISTRIES = {h.strip().lower().rstrip(".") for h in os.environ.get("BIP_ALLOW", "").split(",") if h.strip()}
PORT = int(os.environ.get("BIP_PORT", "3128"))
POLICY = os.environ.get("BIP_POLICY", "")
REPORTS = os.environ.get("BIP_REPORTS", "")
USAGE = os.environ.get("BIP_USAGE", "")
BLOCKLIST = os.environ.get("BIP_BLOCKLIST", "")
STRICT = {a.strip() for a in os.environ.get("BIP_STRICT", "").split(",") if a.strip()}
BROWSERS = {a.strip() for a in os.environ.get("BIP_BROWSERS", "").split(",") if a.strip()}
WAIT = 30
HOUR = 3600
REPORTS_MAX = 256 * 1024
# What the bridge does not set: the guard's starting points (send_guard.rs has the same).
LIMITS = {"send_site_hour": 64 * 1024, "send_day": 256 * 1024, "browser_record_free": 4096,
          "download_day": 0, "connections_minute": 0,
          # a held host (a card waits, the day's total was reached, or the person refused it): what may still go, in
          # all, per held host per hour and per agent per hour across held hosts (11/11 audit PI-2: it was 16 KB per
          # tunnel, again for every new tunnel; D-1: the agent's allowance comes back every hour, reading goes on)
          "held_site": 16 * 1024, "held_hour": 64 * 1024,
          # what may be SENT to the package registries, per sandbox (its agent and its address together), per hour
          # and per day: installing never raises a card, but npm's registry also takes publishes (11/11 audit PI-3).
          # A cold `npm install` of ~2 500 packages sends about 3 MB of requests (estimate, see the commit)
          "registry_hour": 6 * 1024 * 1024, "registry_day": 24 * 1024 * 1024,
          # the browser's free first bytes of each record (its requests), in all: per host per hour, per agent per day;
          # past them they count like a program's (11/11 audit PI-4: no total, any volume in small records)
          "browser_free_site_hour": 4 * 1024 * 1024, "browser_free_day": 32 * 1024 * 1024}
# A tunnel cut: what the client still sends is read (never passed on) for so long, to say how much it wanted to send.
SINK_IDLE = 3.0
SINK_SECONDS = 30.0
SINK_MAX = 1 << 30
USAGE_EVERY = 60
# the counts written for a restart (`save_counts`) at most so often, and at each cut
COUNTS_EVERY = 10
COUNTS = os.path.join(os.path.dirname(USAGE), "counts.json") if USAGE else ""
# one client address: so many connections open at once at most; a request head of at most so many bytes
MAX_OPEN_PER_CLIENT = 128
MAX_OPEN_TOTAL = 384
HEADER_LIMIT = 16 * 1024
# a tunnel's side that does not read: at most so much waits in the proxy for it (asyncio's write buffer)
WRITE_BUFFER = 16 * 1024
# names of hosts not validated an agent connected to today, each charged once (`charge_name`); past so many, each time
NAMES_SEEN = 4096
EXIT_SOCK = os.environ.get("BIP_EXIT_SOCK", "")
# the first label under a routed site that marks static files (bridge-rs phone_exit/checks.rs STATIC_LABELS)
STATIC_LABELS = frozenset(("assets", "static", "statics", "cdn", "img", "images", "image", "media", "medias", "fonts",
                           "font", "css", "js", "pics", "photos", "thumbs", "video", "videos"))

_policy = {"stamp": None, "agents": {}, "browsers": {}, "limits": dict(LIMITS)}
_blocklist = {"stamp": None, "names": frozenset()}
# (agent, host) -> [window start, bytes sent this hour] to hosts not validated
_sent = {}
# (agent, host) -> until when its tunnels are held, a card being asked
_held = {}
# (agent, host) -> [hour start, bytes]: what passed to a held host this hour (at most held_site)
_held_used = {}
# (agent, host) -> [window start, bytes]: the browser's free bytes to hosts not validated this hour
_free = {}
# agent -> today's counts
_days = {}
# agent -> [minute start, new connections]
_minutes = {}
# agent, and "@" + the client's address -> [hour start, bytes, day, bytes] sent to registries
_registries = {}
# client address -> connections open now
_open_by_client = {}


def normal(host):
    return host.strip().lower().rstrip(".")


def today():
    return time.strftime("%Y-%m-%d", time.gmtime())


# the tunnels open now: [agent, host, through the phone, client writer, site writer]
_open_tunnels = []


def load_policy():
    """The policy file, read again when it changed (a missing or broken file: nobody known)."""
    if not POLICY:
        return _policy
    try:
        info = os.stat(POLICY)
        stamp = (info.st_mtime_ns, info.st_size, info.st_ino)
    except OSError:
        _policy.update(stamp=None, agents={}, browsers={}, limits=dict(LIMITS))
        return _policy
    if stamp != _policy["stamp"]:
        try:
            with open(POLICY, encoding="utf-8") as f:
                data = json.load(f)
        except (OSError, ValueError):
            data = {}
        data = data if isinstance(data, dict) else {}
        agents, browsers, limits = data.get("agents"), data.get("browsers"), dict(LIMITS)
        for name, value in (data.get("limits") or {}).items() if isinstance(data.get("limits"), dict) else ():
            if name in LIMITS and isinstance(value, int) and not isinstance(value, bool) and value >= 0:
                limits[name] = value
        _policy.update(stamp=stamp, agents=agents if isinstance(agents, dict) else {},
                       browsers=browsers if isinstance(browsers, dict) else {}, limits=limits)
    return _policy


def policy():
    """The agents' keys and hosts."""
    return load_policy()["agents"]


def limit(name):
    return load_policy()["limits"].get(name, LIMITS[name])


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


def browser_of(address):
    """The agent whose visible browser has this address on the browsers' network, else None."""
    agent = load_policy()["browsers"].get(address)
    return agent if isinstance(agent, str) and isinstance(policy().get(agent), dict) else None


def host_matches(host, pattern):
    pattern = normal(pattern)
    if pattern.startswith("*."):
        base = pattern[2:]
        return host.endswith("." + base) and len(host) > len(base) + 1
    return host == pattern


def connector(agent):
    """A connector's credentials (`"connector": true`): its approved hosts only."""
    entry = policy().get(agent) if agent else None
    return isinstance(entry, dict) and entry.get("connector") is True


def validated(agent, host):
    """A host the agent may send to without counting: a registry, or one its person validated for it."""
    host = normal(host)
    if host in REGISTRIES:
        return True
    entry = policy().get(agent) or {}
    hosts = entry.get("hosts") or [] if isinstance(entry, dict) else []
    return any(isinstance(p, str) and host_matches(host, p) for p in hosts)


def allowed(host, port, wanted, agent=None, strict=False, browser=False):
    """Registries for anyone (HTTPS, and plain HTTP for apt); any public name for a known agent (HTTPS; its browser on
    plain HTTP too). `strict` (the connectors' network) or a connector's credentials: that connector's hosts on HTTPS,
    nothing else."""
    host = normal(host)
    if port != wanted:
        return False
    held = strict or connector(agent)
    if held:
        if not connector(agent) or wanted != 443:
            return False
        hosts = policy().get(agent, {}).get("hosts") or []
        return any(isinstance(p, str) and host_matches(host, p) for p in hosts)
    if host in REGISTRIES:
        return True
    if agent is None or (wanted == 80 and not browser):
        return False
    return not local_name(host)


def blocked_name(host):
    """A name of the offer's blocklist (piracy, torrent trackers), or a subdomain of one."""
    if not BLOCKLIST:
        return False
    try:
        info = os.stat(BLOCKLIST)
        stamp = (info.st_mtime_ns, info.st_size, info.st_ino)
    except OSError:
        _blocklist.update(stamp=None, names=frozenset())
        return False
    if stamp != _blocklist["stamp"]:
        names = set()
        try:
            with open(BLOCKLIST, encoding="utf-8", errors="replace") as f:
                for line in f:
                    line = line.split("#", 1)[0].strip().lower().rstrip(".")
                    if line and " " not in line:
                        names.add(line)
        except OSError:
            names = set()
        _blocklist.update(stamp=stamp, names=frozenset(names))
    names = _blocklist["names"]
    if not names:
        return False
    host = normal(host)
    labels = host.split(".")
    return any(".".join(labels[i:]) in names for i in range(len(labels) - 1))


LOCAL_SUFFIXES = ("localhost", "local", "internal", "lan", "home", "arpa", "test", "invalid", "localdomain")


def nameable(host):
    """A name the bridge can put on a card (sandbox_sites.rs `clean_host`): labels of letters, digits and dashes, not
    starting or ending with a dash, at most 63 each, 253 in all, two at least (11/11 review: a send to any other name
    could never raise a card)."""
    labels = host.split(".")
    return len(host) <= 253 and len(labels) >= 2 and all(
        0 < len(label) <= 63 and all(c in NAME_CHARS for c in label) and label[0] != "-" and label[-1] != "-"
        for label in labels)


NAME_CHARS = frozenset("abcdefghijklmnopqrstuvwxyz0123456789-")


def local_name(host):
    """An address, or a name of this machine or its networks (never opened to reading)."""
    last = host.rsplit(".", 1)[-1]
    return literal(host) or "." not in host or last in LOCAL_SUFFIXES or not last.isalpha() and not last.startswith("xn--")


# IPv6 prefixes that carry an IPv4 address (NAT64, its local-use prefix, 6to4): no route of ours goes through them,
# and behind a NAT64 / 6to4 gateway they reach whatever IPv4 is inside (192.168.1.1 as 64:ff9b::c0a8:101). Python's
# own verdict on them changed between versions (3.9 calls 2002::/16 global): refused here whatever it says.
WRAPPED = tuple(ipaddress.ip_network(n) for n in ("64:ff9b::/96", "64:ff9b:1::/48", "2002::/16"))


def public(address):
    """An address of the internet, not of this machine or a network it is on."""
    try:
        ip = ipaddress.ip_address(address.split("%", 1)[0])
    except ValueError:
        return False
    if ip.version == 6 and any(ip in net for net in WRAPPED):
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
    except Exception as exc:
        log(f"name-not-found ({type(exc).__name__}: {str(exc)[:120]})", "RESOLVE", host)
        return None
    addresses = [info[4][0] for info in found]
    if not addresses or not all(public(a) for a in addresses):
        return []
    return addresses


def log(verdict, method, target, agent=None):
    who = f" [{agent}]" if agent else ""
    print(f"{time.strftime('%Y-%m-%dT%H:%M:%S')} {verdict}{who} {method} {target[:200]}", flush=True)


def append_report(row):
    """One line for the bridge (it reads and empties the file)."""
    if not REPORTS:
        return
    line = json.dumps(row) + "\n"
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


# --- the counts ----------------------------------------------------------------------------------------------------

def new_day(day, since):
    return {"day": day, "since": since, "sent": 0, "received": 0, "unvalidated": 0, "connections": 0, "held": False,
            "held_start": 0, "held_sent": 0, "download_logged": False, "names": set(), "free": 0,
            "free_logged": False}


def day_of(agent):
    """The agent's counts for today (new ones at midnight UTC, or after its person's « Autoriser »)."""
    now, day = time.time(), today()
    entry = _days.get(agent)
    reset = (policy().get(agent) or {}).get("reset") if isinstance(policy().get(agent), dict) else None
    if entry is not None and isinstance(reset, (int, float)) and reset > entry["since"]:
        # the person allowed a site lately: what was counted before is theirs to have seen
        for key in [k for k in _sent if k[0] == agent]:
            del _sent[key]
        for key in [k for k in _held if k[0] == agent]:
            del _held[key]
        for key in [k for k in _held_used if k[0] == agent]:
            del _held_used[key]
        for key in [k for k in _free if k[0] == agent]:
            del _free[key]
        entry.update(since=now, unvalidated=0, held=False, held_start=0, held_sent=0, free=0)
    if entry is None or entry["day"] != day:
        kept = entry or {}
        entry = new_day(day, max(now, kept.get("since", 0)))
        _days[agent] = entry
        if len(_days) > 4096:
            _days.clear()
            _days[agent] = entry
    return entry


def too_many_connections(agent):
    """A new connection for `agent`: False, or True past `connections_minute`."""
    most = limit("connections_minute")
    now = time.time()
    window = _minutes.get(agent)
    if window is None or now - window[0] >= 60:
        window = [now, 0]
        _minutes[agent] = window
        if len(_minutes) > 4096:
            _minutes.clear()
            _minutes[agent] = window
    window[1] += 1
    day_of(agent)["connections"] += 1
    return bool(most) and window[1] > most


def download_over(agent):
    most = limit("download_day")
    return bool(most) and day_of(agent)["received"] >= most


def registry_window(sender):
    """[hour start, bytes this hour, day, bytes today] sent to the registries by `sender`."""
    now, day = time.time(), today()
    window = _registries.get(sender)
    if window is None:
        window = [now, 0, day, 0]
        if len(_registries) > 4096:
            _registries.clear()
        _registries[sender] = window
    if now - window[0] >= HOUR:
        window[0], window[1] = now, 0
    if window[2] != day:
        window[2], window[3] = day, 0
    return window


def registry_over(senders):
    """Whether `senders` (a sandbox's agent and its address: ONE budget, whether it shows its credentials or not)
    sent the registries all they may: "day", "hour", or None."""
    windows = [registry_window(s) for s in senders]
    if any(w[3] >= limit("registry_day") for w in windows):
        return "day"
    if any(w[1] >= limit("registry_hour") for w in windows):
        return "hour"
    return None


def registry_take(senders, size):
    """`size` more bytes to a registry: True within the ceilings (counted for each of `senders`), else False (and the
    ceiling marked reached, so new connections are refused until the window passes)."""
    windows = [registry_window(s) for s in senders]
    hour, day = limit("registry_hour"), limit("registry_day")
    if any(w[1] + size > hour or w[3] + size > day for w in windows):
        for w in windows:
            if w[1] + size > hour:
                w[1] = max(w[1], hour)
            if w[3] + size > day:
                w[3] = max(w[3], day)
        return False
    for w in windows:
        w[1] += size
        w[3] += size
    return True


def refused(agent, host):
    """A host the agent's person refused on its card: « Refuser » (the policy's `refused`, host → until when, 24 hours,
    sandbox_sites.rs QUIET_AFTER_NO) or « Refuser toujours » (`refused_always`). It stays readable: it is held
    (`hold_of`), with the hour's small allowance, and never reported."""
    entry = policy().get(agent) if agent else None
    if not isinstance(entry, dict):
        return False
    host = normal(host)
    always = entry.get("refused_always")
    if isinstance(always, list) and host in always:
        return True
    marks = entry.get("refused")
    until = marks.get(host) if isinstance(marks, dict) else None
    return isinstance(until, (int, float)) and not isinstance(until, bool) and until > time.time()


def hold_of(agent, host, day):
    """Whether this agent's sends to this host are held: a card waits for it, the day's total was reached (every
    host not validated, until midnight UTC), or the person refused it."""
    return _held.get((agent, host), 0) > time.time() or day["held"] or refused(agent, host)


def held_site(agent, host):
    """[hour start, bytes]: what went to this held host this hour, all its tunnels together."""
    key, now = (agent, host), time.time()
    used = _held_used.get(key)
    if used is None or now - used[0] >= HOUR:
        used = [now, 0]
        if len(_held_used) > 16384:
            _held_used.clear()
        _held_used[key] = used
    return used


def held_hour(day):
    """The agent's held allowance this hour (`day["held_sent"]` since `day["held_start"]`), back every hour."""
    now = time.time()
    if now - day["held_start"] >= HOUR:
        day["held_start"], day["held_sent"] = now, 0
    return day


def held_spent(agent, host):
    """A program's new connection to a held host whose allowance for the hour went (the host's, or the agent's
    across held hosts): refused until the hour turns. The browser is never refused for it: its small records are
    free (its pool), and what it would send past the allowance is cut in the tunnel."""
    day = day_of(agent)
    if not hold_of(agent, host, day):
        return False
    used = held_site(agent, host)
    return used[1] >= limit("held_site") or held_hour(day)["held_sent"] >= limit("held_hour")


def free_over(agent, host, spared, day):
    """The browser's free bytes (`spared`) taken from its pools (per host per hour, per agent per day); what does not
    fit, returned, counts like a program's bytes. The first time a pool is full in a day, one log line."""
    key, now = (agent, host), time.time()
    window = _free.get(key)
    if window is None or now - window[0] >= HOUR:
        window = [now, 0]
        if len(_free) > 16384:
            _free.clear()
        _free[key] = window
    room = max(0, min(limit("browser_free_site_hour") - window[1], limit("browser_free_day") - day["free"]))
    taken = min(spared, room)
    window[1] += taken
    day["free"] += taken
    if taken < spared and not day["free_logged"]:
        day["free_logged"] = True
        log(f"browser's free bytes pool reached ({window[1]} bytes this hour to this host, {day['free']} today): "
            "counted from now", "SEND", host, agent)
    return spared - taken


def charge_name(agent, host, browser=False):
    """A new connection (a CONNECT, a plain-HTTP request) to a host not validated: its name's length counts as sent,
    before it is looked up — data can be spelt in names (11/11 audit PI-5), many distinct ones reach the day's total
    like any slow leak (one report, then the held hours). A name costs its length once a day, then 2 bytes at each
    new connection (which of the names already charged is chosen carries up to 12 bits: 11/11 review C-3), up to
    NAMES_SEEN names, then its length every time. False: refused (the held hour's allowance went)."""
    day = day_of(agent)
    seen = day["names"]
    size = 2 if host in seen else len(host)
    if not hold_of(agent, host, day) and day["unvalidated"] + size > limit("send_day"):
        day["held"] = True
        append_report({"kind": "send", "agent": agent, "host": host[:253], "bytes": int(day["unvalidated"] + size),
                       "day": True, "at": int(time.time())})
        log(f"cut (send: the day's total reached, {day['unvalidated'] + size} bytes, names included)", "NAME", host,
            agent)
    if hold_of(agent, host, day):
        if held_hour(day)["held_sent"] + size > limit("held_hour"):
            # the browser keeps reading: its name then comes from its pool of free bytes (the bound it has anyway);
            # a program, or a browser past its pool too, is refused until the hour turns
            if not browser or free_over(agent, host, size, day):
                return False
        else:
            day["held_sent"] += size
    else:
        day["unvalidated"] += size
    if len(seen) < NAMES_SEEN:
        seen.add(host)
    return True


class Tls:
    """What a browser sends in a TLS stream, record by record: every record's content counts (whatever its type: a
    client chooses the type it frames its bytes in), but the first `free` bytes of each (of each read, out of TLS) —
    a browser's requests are small records, an upload's full ones (16 KB). A stream that does not start as TLS counts
    whole. The sandboxes' programs never go through this: they choose their own record sizes and types, every byte
    they send counts (`Guard`)."""

    def __init__(self, free=0):
        self.mode = None   # None (first byte not seen), "tls" or "raw"
        self.header = b""
        self.left = 0
        self.kind = 0
        self.free = free
        self.free_left = 0
        self.spared = 0    # the free bytes of the last `count`

    def count(self, data):
        if self.mode is None and data:
            self.mode = "tls" if data[0] == 0x16 else "raw"
        if self.mode == "raw":
            self.spared = min(len(data), self.free)
            return len(data) - self.spared
        counted, at = 0, 0
        self.spared = 0
        while at < len(data):
            if self.left:
                take = min(self.left, len(data) - at)
                spared = min(self.free_left, take)
                self.free_left -= spared
                self.spared += spared
                counted += take - spared
                self.left -= take
                at += take
                continue
            need = 5 - len(self.header)
            self.header += data[at:at + need]
            at += need
            if len(self.header) == 5:
                self.kind = self.header[0]
                self.left = int.from_bytes(self.header[3:5], "big")
                self.free_left = self.free
                self.header = b""
                if self.kind not in (20, 21, 22, 23, 24):
                    # not TLS after all: whatever follows counts whole
                    self.mode = "raw"
                    return counted + (len(data) - at)
        return counted


class Guard:
    """One tunnel of a known agent: its bytes counted for the day; its sends counted against the budgets when the host
    is not validated for it (`counted`), the tunnel cut past them. To a registry (`registry`: who sends, an agent or a
    client address without credentials, then `agent` is None): what it sends counted against the registries' ceiling,
    with no card."""

    def __init__(self, agent, host, counted, browser=False, registry=None):
        self.agent, self.host, self.counted = agent, host, counted
        self.registry = registry
        # a program: every byte (TLS record headers and handshakes included: a program frames what it likes in them,
        # 2026-11-11 audit BV-4); the browser: its records past their free first bytes
        self.tls = Tls(limit("browser_record_free")) if browser else None
        self.up = 0          # every byte the client sent through (the log's figure)
        self.down = 0
        self.app = 0         # what counted against the budgets in this tunnel
        self.pending = 0     # the chunk that went over (never passed on)
        self.sunk = 0        # what the client still sent once cut (never passed on)
        self.cut = None      # why the tunnel was cut: "send", "held", "download", "registry"

    def sending(self, data):
        """`data` the client sends: True to pass it on, False to cut the tunnel (`self.cut` says why)."""
        self.up += len(data)
        if self.registry is not None and not registry_take(self.registry, len(data)):
            self.cut = "registry"
            log(f"cut (registry: sending limit reached, {self.up} bytes in this tunnel)", "SEND", self.host,
                self.agent or self.registry[-1])
            save_counts()
            return False
        if self.agent is None:
            return True   # no credentials: a registry, nothing else to count
        day = day_of(self.agent)
        if not self.counted:
            day["sent"] += len(data)
            return True
        size = self.tls.count(data) if self.tls is not None else len(data)
        if self.tls is not None and self.tls.spared:
            size += free_over(self.agent, self.host, self.tls.spared, day)
        if not size:
            day["sent"] += len(data)
            return True
        key = (self.agent, self.host)
        now = time.time()
        if hold_of(self.agent, self.host, day):
            # held (a card waits, the day's total went, or the person refused it): reading still opens, within ONE
            # allowance an hour for the host and one for the agent, whatever the number of tunnels; past it, cut. The
            # browser's free small records do not come from it (its pool): no better off than a fresh host.
            used, hour = held_site(self.agent, self.host), held_hour(day)
            over_site = used[1] + size > limit("held_site")
            over_hour = hour["held_sent"] + size > limit("held_hour")
            if over_site or over_hour:
                # a chunk that does not fit spends the host's allowance; the agent's only when what is left of it
                # would not open a connection (11/11 review C-2: one retried chunk burnt every held host's hour)
                used[1] = max(used[1], limit("held_site"))
                if limit("held_hour") - hour["held_sent"] < 2048:
                    hour["held_sent"] = limit("held_hour")
                self.cut = "held"
                return False
            used[1] += size
            day["held_sent"] += size
            self.app += size
            day["sent"] += len(data)
            return True
        window = _sent.get(key)
        if window is None or now - window[0] >= HOUR:
            window = [now, 0]
            _sent[key] = window
            if len(_sent) > 16384:
                _sent.clear()
                _sent[key] = window
        over_day = day["unvalidated"] + size > limit("send_day")
        if window[1] + size > limit("send_site_hour") or over_day:
            self.cut = "send"
            day["held"] = day["held"] or over_day
            if len(_held) > 16384:
                _held.clear()
            _held[key] = window[0] + HOUR
            return False
        window[1] += size
        day["unvalidated"] += size
        day["sent"] += len(data)
        self.app += size
        return True

    def receiving(self, data):
        self.down += len(data)
        if self.agent is None:
            return True
        day = day_of(self.agent)
        day["received"] += len(data)
        if download_over(self.agent):
            self.cut = "download"
            if not day["download_logged"]:
                day["download_logged"] = True
                log("download-limit", "DAY", f"{day['received']} bytes", self.agent)
            return False
        return True

    def tried(self):
        """What the client sent to this host this hour, or tried to (the card's figure)."""
        window = _sent.get((self.agent, self.host)) or [0, 0]
        return int(window[1] + self.pending + self.sunk)

    def report(self):
        """The person asked (by the bridge): one line, with what the client sent or tried to."""
        if self.cut != "send":
            return
        append_report({"kind": "send", "agent": self.agent, "host": self.host[:253], "bytes": self.tried(),
                       "day": bool(day_of(self.agent)["held"]), "at": int(time.time())})


def close(writer):
    try:
        writer.close()
    except Exception:
        pass


async def pipe_up(reader, up_writer, writer, guard):
    """The client's bytes to the site, counted; past the budgets nothing more goes, and what the client still sends
    is measured (`sink`)."""
    try:
        while True:
            data = await reader.read(65536)
            if not data:
                break
            if guard is not None and not guard.sending(data):
                guard.pending = len(data)
                break
            up_writer.write(data)
            await up_writer.drain()
    except Exception:
        pass
    finally:
        close(up_writer)
        if guard is not None and guard.cut in ("send", "held"):
            await sink(reader, guard)
            close(writer)
        elif guard is not None and guard.cut == "registry":
            close(writer)


async def pipe_down(up_reader, writer, up_writer, guard):
    """The site's bytes to the client, counted (the offer's daily download limit)."""
    try:
        while True:
            data = await up_reader.read(65536)
            if not data:
                break
            if guard is not None and not guard.receiving(data):
                break
            writer.write(data)
            await writer.drain()
    except Exception:
        pass
    finally:
        if guard is not None and guard.cut == "download":
            close(up_writer)
        if guard is None or guard.cut not in ("send", "held"):
            close(writer)   # once cut while sending, pipe_up measures, then closes


async def sink(reader, guard):
    """A tunnel cut while sending: what the client still sends is read and dropped, to know how much it wanted to
    send (the card's « environ 2 Mo »), until it stops a moment, then reported."""
    started = time.time()
    try:
        while guard.sunk < SINK_MAX and time.time() - started < SINK_SECONDS:
            data = await asyncio.wait_for(reader.read(65536), SINK_IDLE)
            if not data:
                break
            guard.sunk += len(data)
    except Exception:
        pass
    guard.report()
    save_counts()
    log(f"cut ({guard.cut}: about {guard.tried()} bytes sent or tried this hour)", "SEND", guard.host, guard.agent)


# --- the refusals --------------------------------------------------------------------------------------------------

NOT_CONNECTOR = "Site not approved for this connector - your person approves its sites with it"
NOT_OPEN = "Site not allowed - only package registries without your own access"
NOT_HTTPS = "Only HTTPS sites are reachable (https://, port 443)"
PRIVATE = "Private address not allowed"
BLOCKED = "Site refused on this service (piracy or torrent tracker)"
DOWNLOADS = "Download limit for today reached - retry tomorrow, or ask your person"
PHONE_PAUSED = "Phone exit paused - this site goes through your person's phone, which is not relaying now"
PHONE_REFUSED = "Site refused by the phone exit"
PHONE_FAILED = "Bad Gateway (the phone could not reach the site)"
CONNECTIONS = "Too many new connections - wait a minute"
UNKNOWN_BROWSER = "Unknown browser"
REFUSED = "Sending to this site refused by your person - it stays readable, retry within the hour"
REGISTRY_HOUR = "Package registries paused - too much was sent to them, retry within the hour"
REGISTRY_DAY = "Package registries paused - too much was sent to them today, retry tomorrow"
PAUSED = "Sending paused - your person has a card to answer about this site, retry within the hour"
PAUSED_TODAY = "Sending limited for the rest of the hour - today's small sends are used up, retry within the hour"
UNNAMED = "Site name not allowed - only names with letters, digits and dashes are reachable"
TOO_MANY_OPEN = "Too many connections open at once - close some, then retry"
BODIES = {
    REGISTRY_HOUR: "What your programs send to the package registries is limited per hour and per day, and the "
                   "hour's limit was reached: installs wait until it clears, within the hour. Tell your person if it "
                   "blocks their work.",
    REGISTRY_DAY: "What your programs send to the package registries is limited per hour and per day, and today's "
                  "limit was reached: installs wait until tomorrow (midnight UTC). Tell your person if it blocks "
                  "their work.",
    REFUSED: "Your person refused sending to this site. It stays readable with a small allowance an hour, which is "
             "used up: retry within the hour, and do without sending to it.",
    PAUSED_TODAY: "Your programs sent today what sites not validated may take without your person: a small amount "
                  "passes each hour, this hour's is used up. Retry within the hour; if your person's work needs "
                  "this site, ask them to allow it (sandbox_sites_request).",
    UNNAMED: "Sites are reached by their name (letters, digits and dashes, like data.gouv.fr).",
    PAUSED: "You sent this site more than a small amount: your person has a card to answer. Reading goes on with a "
            "small allowance an hour, which is used up: retry within the hour, or wait for their answer.",
    TOO_MANY_OPEN: "Your programs hold too many connections to the proxy open at once: close some, then retry.",
    NOT_CONNECTOR: "This connector may only reach the sites your person approved with it.",
    NOT_OPEN: "Only the package registries can be reached without your own access.",
    NOT_HTTPS: "Your programs reach sites over HTTPS only.",
    PRIVATE: "Addresses of this machine and its networks are never reachable.",
    BLOCKED: "Known piracy and torrent tracker sites are refused on BipAgents' servers.",
    DOWNLOADS: "Your downloads for today reached this server's daily limit. Tell your person.",
    CONNECTIONS: "Too many new connections in a minute: wait a moment, then go on more slowly.",
    PHONE_PAUSED: "Your person set this site to go through their phone. Ask them to start the relay, then retry.",
    PHONE_REFUSED: "The phone exit refused this site or address.",
}


def refusal(agent, strict, wanted, port=443):
    """The reason phrase of a refusal: what happens, in words the agent repeats to its person."""
    if strict or connector(agent):
        return NOT_CONNECTOR
    if agent and (wanted != 443 or port != 443):
        return NOT_HTTPS
    return PRIVATE if agent else NOT_OPEN


async def answer(writer, status, reason, extra=""):
    body = (BODIES.get(reason) or f"{status} {reason}.") + "\n"
    body = body.encode()
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


def phone_exit(host, agent):
    """This agent's tunnel to this host goes through its person's phone (the hub checks again): a site of its
    `phone` list, but a static-file host of a site with the minimal route goes direct."""
    if not EXIT_SOCK or agent is None or connector(agent):
        return False
    entry = policy().get(agent)
    routes = entry.get("phone") if isinstance(entry, dict) else None
    host = normal(host)
    for route in routes if isinstance(routes, list) else []:
        site = normal(route.get("site", "")) if isinstance(route, dict) else ""
        if not site or not (host == site or host.endswith("." + site)):
            continue
        if route.get("minimal") and host != site and host[:-len(site) - 1].rsplit(".", 1)[-1] in STATIC_LABELS:
            return False
        return True
    return False


class PhoneRefused(Exception):
    pass


async def connect_phone(host, port, agent):
    """The hub's stream to the site through the phone, or PhoneRefused with the hub's reason."""
    try:
        reader, writer = await asyncio.wait_for(asyncio.open_unix_connection(EXIT_SOCK), WAIT)
    except Exception:
        raise PhoneRefused("paused") from None
    try:
        name = "".join(c for c in (agent or "-") if c.isalnum() or c in "-_.")[:64] or "-"
        writer.write(f"EXIT {host} {port} {name}\n".encode())
        await writer.drain()
        line = await asyncio.wait_for(reader.readline(), WAIT)
    except Exception:
        close(writer)
        raise PhoneRefused("paused") from None
    if line.startswith(b"OK"):
        return reader, writer
    close(writer)
    raise PhoneRefused(line[4:].decode("latin-1").strip()[:40] or "paused")


def phone_answer(reason):
    """The refusal the agent reads: paused (it waits, its person is asked), refused, or the site unreachable (the
    browser tool tells the agent the hub's exact reason, docs/phone-exit.md §7)."""
    reason = reason.replace("-", "_")
    if reason in ("paused", "off", "timeout", "rate", "expired", "wifi_only", "phone"):
        return 503, PHONE_PAUSED
    if reason in ("dns", "connect"):
        return 502, PHONE_FAILED
    return 403, PHONE_REFUSED


async def connect(addresses, port):
    last = None
    for address in addresses:
        try:
            return await asyncio.wait_for(asyncio.open_connection(address, port, limit=HEADER_LIMIT), WAIT)
        except Exception as exc:
            last = exc
    raise last or OSError("no address")


def ends(writer):
    """This proxy's address the client reached, and the client's address."""
    try:
        local = (writer.get_extra_info("sockname") or ("",))[0]
        peer = (writer.get_extra_info("peername") or ("",))[0]
    except Exception:
        local, peer = "", ""
    return local, peer


async def serve(reader, writer):
    """One connection, within its client address's share: at most MAX_OPEN_PER_CLIENT open at once (a sandbox holding
    hundreds of unfinished request heads could otherwise run the proxy out of memory, and a restart used to reset
    every count, 11/11 review R-1)."""
    _, peer = ends(writer)
    if _open_by_client.get(peer, 0) >= MAX_OPEN_PER_CLIENT or sum(_open_by_client.values()) >= MAX_OPEN_TOTAL:
        log("blocked (too many connections open)", "-", peer)
        return await answer(writer, 429, TOO_MANY_OPEN)
    _open_by_client[peer] = _open_by_client.get(peer, 0) + 1
    try:
        await handle(reader, writer)
    finally:
        _open_by_client[peer] -= 1
        if _open_by_client[peer] <= 0:
            del _open_by_client[peer]


async def start(host="0.0.0.0", port=None):
    """The proxy's server: each request head at most HEADER_LIMIT bytes (asyncio's stream limit)."""
    return await asyncio.start_server(serve, host, PORT if port is None else port, limit=HEADER_LIMIT)


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
    local, peer = ends(writer)
    strict = local in STRICT
    browser = local in BROWSERS
    if browser:
        agent = browser_of(peer)
        auth = ""
        if agent is None:
            log("unknown-browser", method, peer)
            return await answer(writer, 403, UNKNOWN_BROWSER)
    else:
        auth = next((line.split(b":", 1)[1].decode("latin-1") for name, line in headers
                     if name == "proxy-authorization"), "")
        agent = who(auth)
    if method.upper() == "CONNECT":
        host, port = split_authority(target, 443)
        wanted = 443
        request = b""
    elif target.lower().startswith("http://"):
        authority, _, path = target[7:].partition("/")
        host, port = split_authority(authority, 80)
        wanted = 80
        # the Host line is the requested host's, never the client's own (RFC 9112 §3.2.2; 27/10 audit: a registry
        # name in the request line, another site in Host, on a shared front)
        kept = b"".join(line + b"\r\n" for name, line in headers if name not in ("proxy-authorization",
                                                                                   "proxy-connection", "host"))
        named = normal(host) + (f":{port}" if port != 80 else "")
        request = f"{method} /{path} {version}\r\nHost: {named}\r\n".encode("latin-1") + kept + b"\r\n"
    else:
        return await answer(writer, 400, "Bad Request")
    host = normal(host)
    if not allowed(host, port, wanted, agent, strict, browser):
        if agent is None and not auth and wanted == 443 and port == 443 and not browser:
            # a client that sends its credentials only when asked (the registries need none)
            log("ask-credentials", method, target)
            return await answer(writer, 407, "Proxy Authentication Required",
                                'Proxy-Authenticate: Basic realm="bipagents"\r\n')
        log("blocked", method, target, agent)
        return await answer(writer, 403, refusal(agent, strict, wanted, port))
    counted = agent is not None and not connector(agent)
    # the registries' budget is the sandbox's, credentials or not: its agent's and its address's together
    senders = (agent, "@" + peer) if agent is not None else ("@" + peer,)
    over = registry_over(senders) if host in REGISTRIES else None
    if over:
        log(f"blocked (registry sending limit, {over})", method, target, agent or senders[-1])
        return await answer(writer, 403, REGISTRY_DAY if over == "day" else REGISTRY_HOUR)
    if counted and host not in REGISTRIES and blocked_name(host):
        log("blocked (blocklist)", method, target, agent)
        return await answer(writer, 403, BLOCKED)
    if counted and download_over(agent):
        log("blocked (download limit)", method, target, agent)
        return await answer(writer, 403, DOWNLOADS)
    if counted and not validated(agent, host):
        if not nameable(host):
            # a send to it could never be asked about (the bridge's cards take these names only): not reachable
            log("blocked (not a site's name)", method, target, agent)
            return await answer(writer, 403, UNNAMED)
        if (not browser and held_spent(agent, host)) or not charge_name(agent, host, browser):
            reason = REFUSED if refused(agent, host) else PAUSED if _held.get((agent, host), 0) > time.time() \
                else PAUSED_TODAY
            log("blocked (held: the hour's small allowance went)", method, target, agent)
            return await answer(writer, 403, reason)
    if agent is not None and too_many_connections(agent):
        log("blocked (connections per minute)", method, target, agent)
        return await answer(writer, 429, CONNECTIONS)
    if phone_exit(host, agent):
        if wanted != 443:
            log("blocked (phone exit: HTTPS only)", method, target, agent)
            return await answer(writer, 403, NOT_HTTPS)
        try:
            up_reader, up_writer = await connect_phone(host, port, agent)
        except PhoneRefused as exc:
            status, reason = phone_answer(str(exc))
            log(f"blocked (phone exit: {exc})", method, target, agent)
            return await answer(writer, status, reason)
        log("allowed (phone exit)", method, target, agent)
        return await tunnel(reader, writer, up_reader, up_writer, method, target, agent, host, b"", counted, browser,
                            senders)
    addresses = await resolve(host, port)
    if addresses == []:
        log("blocked-address", method, target, agent)
        return await answer(writer, 403, PRIVATE)
    if addresses is None:
        # the reason is in the client's error (« 502 Bad Gateway (site name not found) »), so the agent says it
        log("failed (name not found)", method, target, agent)
        return await answer(writer, 502, "Bad Gateway (site name not found)")
    try:
        up_reader, up_writer = await connect(addresses, port)
    except Exception as exc:
        log(f"failed ({type(exc).__name__}: {str(exc)[:120]})", method, target, agent)
        return await answer(writer, 502, "Bad Gateway (site unreachable)")
    log("allowed", method, target, agent)
    return await tunnel(reader, writer, up_reader, up_writer, method, target, agent, host, request, counted, browser,
                        senders)


async def tunnel(reader, writer, up_reader, up_writer, method, target, agent, host, request, counted, browser,
                 senders=None):
    """The client's stream to the site (or the phone exit's), counted and guarded."""
    registry = senders if senders and normal(host) in REGISTRIES and not connector(agent) else None
    guard = Guard(agent if counted else None, host, counted and not validated(agent, host), browser, registry) \
        if counted or registry else None
    for side in (writer, up_writer):
        try:
            side.transport.set_write_buffer_limits(high=WRITE_BUFFER)
        except Exception:
            pass
    entry = [agent, normal(host), phone_exit(host, agent), writer, up_writer]
    _open_tunnels.append(entry)
    try:
        await carry(reader, writer, up_reader, up_writer, method, target, agent, request, guard)
    finally:
        if entry in _open_tunnels:
            _open_tunnels.remove(entry)


def switch_tunnels():
    """The tunnels whose way changed (a site now through the phone, or no longer) closed: the browser opens new ones,
    which go the new way. Never a site of the phone left on the datacenter."""
    for entry in list(_open_tunnels):
        agent, host, phone, writer, up_writer = entry
        if agent is not None and phone_exit(host, agent) != phone:
            log("closed (phone exit switched)", "CONNECT", f"{host}:443", agent)
            close(up_writer)
            close(writer)
            _open_tunnels.remove(entry)


async def keep_switching():
    while True:
        await asyncio.sleep(1)
        try:
            switch_tunnels()
        except Exception as exc:   # never the proxy down for this
            print(f"switch_tunnels: {exc!r}", flush=True)


async def carry(reader, writer, up_reader, up_writer, method, target, agent, request, guard):
    if request:
        if guard is not None and not guard.sending(request) and guard.cut == "registry":
            close(up_writer)
            over = registry_over(guard.registry)
            return await answer(writer, 403, REGISTRY_DAY if over == "day" else REGISTRY_HOUR)
        if guard is not None and guard.cut:
            guard.pending = len(request)
            close(up_writer)
            await sink(reader, guard)
            close(writer)
            return
        up_writer.write(request)
    else:
        writer.write(b"HTTP/1.1 200 Connection established\r\n\r\n")
    started = time.time()
    await asyncio.gather(pipe_up(reader, up_writer, writer, guard), pipe_down(up_reader, writer, up_writer, guard))
    if guard is not None:
        log(f"closed up={guard.up} down={guard.down} sent={guard.app} secs={int(time.time() - started)}", method,
            target, agent)


# --- what it counted -----------------------------------------------------------------------------------------------

def usage():
    """Today's counts per agent, for the bridge."""
    day = today()
    agents = {name: {"sent": e["sent"], "received": e["received"], "unvalidated": e["unvalidated"],
                     "connections": e["connections"], "held": e["held"], "browser_free": e.get("free", 0)}
              for name, e in _days.items() if e["day"] == day}
    return {"day": day, "at": int(time.time()), "agents": agents}


def write_json(path, value):
    if not path:
        return
    tmp = path + ".part"
    try:
        try:
            os.unlink(tmp)
        except FileNotFoundError:
            pass
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        try:
            os.write(fd, (json.dumps(value) + "\n").encode("utf-8"))
        finally:
            os.close(fd)
        os.replace(tmp, path)
    except OSError:
        pass


def write_usage():
    write_json(USAGE, usage())


def counts_state():
    """What a restart must not forget: today's totals per agent, the hour windows still running, the holds."""
    now, day = time.time(), today()
    rows = lambda table: [[a, h, *w] for (a, h), w in table.items() if now - w[0] < HOUR]  # noqa: E731
    return {"v": 1, "day": day, "at": int(now),
            "days": {a: dict(e, names=sorted(e["names"])) for a, e in _days.items() if e["day"] == day},
            "sent": rows(_sent), "held_used": rows(_held_used), "free": rows(_free),
            "held": [[a, h, until] for (a, h), until in _held.items() if until > now],
            "registries": {s: w for s, w in _registries.items() if w[2] == day}}


_saved = {"body": None}


def save_counts():
    """Written only when something moved since the last write (an idle place does not touch the file: the backups
    look at it, 11/11 review B-2)."""
    state = counts_state()
    body = json.dumps({k: v for k, v in state.items() if k != "at"}, sort_keys=True)
    if body == _saved["body"]:
        return
    _saved["body"] = body
    write_json(COUNTS, state)


def same_kind(value, default):
    """A saved value of the field's kind (a number for a number, a flag for a flag, a day for a day)."""
    if isinstance(default, bool) or isinstance(value, bool):
        return isinstance(default, bool) and isinstance(value, bool)
    if isinstance(default, (int, float)):
        return isinstance(value, (int, float))
    return type(value) is type(default)


def load_counts():
    """The counts a previous run of the proxy wrote today, if any (a broken file: none)."""
    try:
        with open(COUNTS, encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, ValueError):
        return
    if not isinstance(data, dict) or data.get("v") != 1 or data.get("day") != today():
        return
    for agent, saved in (data.get("days") or {}).items():
        if isinstance(saved, dict):
            entry = new_day(today(), time.time())
            entry.update({k: v for k, v in saved.items() if k in entry and same_kind(v, entry[k])})
            entry["names"] = {n for n in saved.get("names") or [] if isinstance(n, str)}
            _days[agent] = entry
    for table, name in ((_sent, "sent"), (_held_used, "held_used"), (_free, "free")):
        for row in data.get(name) or []:
            if isinstance(row, list) and len(row) == 4:
                table[(row[0], row[1])] = [row[2], row[3]]
    for row in data.get("held") or []:
        if isinstance(row, list) and len(row) == 3:
            _held[(row[0], row[1])] = row[2]
    for sender, window in (data.get("registries") or {}).items():
        if isinstance(window, list) and len(window) == 4:
            _registries[sender] = window
    log(f"counts restored ({len(_days)} agents, written at {data.get('at')})", "START", COUNTS)


async def keep_counts():
    while True:
        await asyncio.sleep(COUNTS_EVERY)
        save_counts()


async def keep_usage():
    while True:
        await asyncio.sleep(USAGE_EVERY)
        write_usage()


async def main():
    load_counts()
    server = await start()
    print(f"agents' proxy on :{PORT}, registries: {', '.join(sorted(REGISTRIES))}; agents from "
          f"{POLICY or 'nowhere'}; reading open, sending guarded", flush=True)
    loop = asyncio.get_running_loop()
    loop.create_task(keep_usage())
    loop.create_task(keep_counts())
    if EXIT_SOCK:
        loop.create_task(keep_switching())
    stopped = loop.create_future()
    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, lambda: stopped.done() or stopped.set_result(None))
    async with server:
        await stopped
        # a clean stop (podman stop): the counts kept for the next start, at once, then out — leaving the block would
        # wait for every open connection since Python 3.12 (11/11 review S-1), until podman's SIGKILL
        save_counts()
        write_usage()
        os._exit(0)


if __name__ == "__main__":
    asyncio.run(main())
