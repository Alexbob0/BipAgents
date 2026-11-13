"""Settings an install made before a change gets when it updates (`bipinstall update`): what a fresh install now
sets itself. Each one is applied once (`~/.local/share/bipagents/migrations.json` keeps the ids done) and can run
again without harm. A Hermes the person had before BipAgents (existing.py) keeps its settings: the update says what
we would change, the person decides.
"""
from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Callable, List, Optional, Tuple

from . import bridgeconf, hermes

STATE = Path.home() / ".local" / "share" / "bipagents" / "migrations.json"
REPO = Path(__file__).resolve().parents[2]


class NotYet(Exception):
    """A migration that couldn't be applied this time (safe to show): not marked done, tried at the next update."""


def is_place(home: Path) -> bool:
    """A place of the all-included offer (its account is the person's alone: its caches are ours to empty)."""
    return (home / ".config" / "bipagents" / "setup.json").is_file()


GATEWAY_UNITS = ("hermes-gateway.service", "bipagents-hermes.service")
SANDBOXES_DROPIN = "60-bipagents-sandboxes.conf"
SANDBOXES_TEXT = """# BipAgents (installer/bipinstall/migrations.py): the agents' sandboxes this gateway started die with it (their
# fuse-overlayfs mounts live in its cgroup) and Hermes would keep the dead containers: every terminal and file tool then
# fails (« transport endpoint is not connected », bench of 2026-10-31). Removed before it starts; Hermes makes them
# again at their next use (the agents' files are in their volumes, kept).
[Service]
ExecStartPre=-/bin/sh -c 'podman ps -a --format "{{.Names}}" | grep "^hermes-" | xargs -r podman rm -f >/dev/null 2>&1; exit 0'
"""


def sandboxes_at_gateway_start(runner: hermes.Runner, home: Path) -> str:
    """The gateway removes the agents' dead sandboxes before it starts (2026-10-31): a drop-in for each name its unit
    may have (written even before the unit exists: systemd reads it when it does). Linux only."""
    import sys
    if sys.platform == "darwin":
        return "macOS: the gateway's sandboxes left as they are"
    units = home / ".config" / "systemd" / "user"
    for unit in GATEWAY_UNITS:
        path = units / (unit + ".d") / SANDBOXES_DROPIN
        runner.log.append(f"write {path}")
        if not runner.dry:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(SANDBOXES_TEXT, encoding="utf-8")
    runner.run(["systemctl", "--user", "daemon-reload"], check=False)
    return "the gateway removes the agents' dead sandboxes before it starts (from its next start)"


def push_relay_service(runner: hermes.Runner, home: Path) -> str:
    """The relay between bridges retired (2026-11-04): an install that sent its notifications through another
    person's bridge (`[push] relay_url`, `relay_key`: Chi through Alex's) sends them through the publisher's relay
    (`[push] service_url`), and a bridge that relayed for others loses its `[relay]` table (its `relay-clients` file
    goes). Waits (NotYet) while this version doesn't know the relay's address."""
    from .family import bridge_restart
    from .stack import PUSH_SERVICE_URL
    config = home / ".config" / "bipagents" / "bridge.toml"
    if not config.is_file():
        return "no bridge here"
    text = config.read_text(encoding="utf-8")
    member = re.search(r"^\s*relay_(url|key)\s*=", text, re.M)
    serving = re.search(r"^\[relay(\.clients)?\]\s*$", text, re.M)
    if not member and not serving:
        return "no relay between bridges here"
    if member and not PUSH_SERVICE_URL:
        raise NotYet("the publisher's push relay has no address in this version yet: notifications still go through "
                     "the other bridge")

    def changed(text: str) -> str:
        kept, skipping = [], False
        for line in text.splitlines():
            header = line.strip()
            if header.startswith("[") and header.endswith("]"):
                skipping = header in ("[relay]", "[relay.clients]")
            if skipping or re.match(r"\s*relay_(url|key)(_file)?\s*=", line):
                continue
            kept.append(line)
        out = "\n".join(kept).rstrip("\n") + "\n"
        if member and not re.search(r"^\s*service_url\s*=", out, re.M):
            line = f'service_url = "{PUSH_SERVICE_URL}"'
            out = (re.sub(r"^\[push\]\s*$", f"[push]\n{line}", out, count=1, flags=re.M)
                   if re.search(r"^\[push\]\s*$", out, re.M) else out + f"\n[push]\n{line}\n")
        return out
    runner.log.append(f"write {config}")
    if not runner.dry:
        bridgeconf.update(config, changed)
        (config.parent / "relay-clients").unlink(missing_ok=True)
        from . import drain
        drain.until_quiet(config, "bridge")   # the turns going on end first (drain.py)
    runner.run(bridge_restart(home), check=False)
    return ("notifications now go through the publisher's push relay" if member
            else "this bridge no longer relays other installs' notifications") + " (bridge restarted)"


def push_service_everywhere(runner: hermes.Runner, home: Path) -> str:
    """Every install sends its notifications through the publisher's relay (`[push] service_url`), not only the ones
    that relayed through another bridge (2026-11-12: the offer's places and the installs made before 2026-11-04 18:31,
    when PUSH_SERVICE_URL was still empty, had none; their bridge answered `push_relay: false`, the phones registered
    a token it could never send to, and no notification ever left). An install with an Apple key of its own gets it
    too (Alex 2026-11-12: every install through the relay; the bridge prefers it as soon as `service_url` is set, its
    `[apns]` and .p8 are left for the person to remove). A `service_url` already given is kept; an empty one is filled. Then the
    bridge restarts (its turns first, drain.py), and the phones take a relay token at their next launch."""
    from .family import bridge_restart
    from .stack import PUSH_SERVICE_URL
    config = home / ".config" / "bipagents" / "bridge.toml"
    if not config.is_file():
        return "no bridge here"
    if not PUSH_SERVICE_URL:
        raise NotYet("the publisher's push relay has no address in this version yet")
    line = f'service_url = "{PUSH_SERVICE_URL}"'

    def has_one(text: str) -> bool:
        return bool(re.search(r'^\s*service_url\s*=\s*"[^"\s]+"', _push_table(text), re.M))

    if has_one(config.read_text(encoding="utf-8")):
        return "notifications already go through the publisher's push relay"

    def changed(text: str) -> str:
        if has_one(text):
            return text
        out, inside, put = [], False, False
        for row in text.splitlines():
            header = row.strip()
            if header.startswith("[") and header.endswith("]"):
                inside = header == "[push]"
                out.append(row)
                if inside and not put:
                    out.append(line)
                    put = True
                continue
            if inside and re.match(r"\s*service_url\s*=", row):
                continue   # an empty one, replaced by the line above
            out.append(row)
        body = "\n".join(out).rstrip("\n") + "\n"
        return body if put else body + f"\n[push]\n{line}\n"
    runner.log.append(f"write {config}")
    if not runner.dry:
        bridgeconf.update(config, changed)
        from . import drain
        drain.until_quiet(config, "bridge")   # the turns going on end first (drain.py)
    runner.run(bridge_restart(home), check=False)
    said = "notifications now go through the publisher's push relay (bridge restarted; phones follow at their next launch)"
    key = re.search(r'^\s*p8_path\s*=\s*"([^"]+)"', config.read_text(encoding="utf-8"), re.M)
    if key:
        # Alex 2026-11-12: his own install too. Its [apns] stays for now: a phone still known by its APNs token is sent
        # to directly until its next launch gives a relay token (bridge-rs app.rs push_mode), so none is missed.
        said += (f"; the Apple key {key.group(1)} is no longer needed once every phone moved (the bridge's log says "
                 "« same phone's other token dropped »): remove the [apns] table and that file yourself")
    return said


def _push_table(text: str) -> str:
    """The lines of `[push]` (empty when there is none)."""
    out, inside = [], False
    for line in text.splitlines():
        header = line.strip()
        if header.startswith("[") and header.endswith("]"):
            inside = header == "[push]"
            continue
        if inside:
            out.append(line)
    return "\n".join(out)


def clarify_hour(runner: hermes.Runner, home: Path) -> str:
    """A question waits an hour for its answer (2026-11-10, before Hermes 0.23: it waits 90 s, and its config migration
    v53 lowers an hour to that): the hub of each Hermes of this account (~/.hermes, each space's), the profile every
    agent is cloned from, gets both keys (hermes.CLARIFY_WAIT). The agents' own profiles are the bridge's, at each of
    its starts (tool_diet.rs `with_clarify`). A second run writes the same values."""
    roots = [home / ".hermes"] + sorted((home / ".local" / "share" / "bipagents" / "spaces").glob("*"))
    count, env = 0, runner.env
    for root in roots:
        if not (root / "config.yaml").is_file():
            continue
        runner.env = {**env, "HERMES_HOME": str(root)}
        for key, value in hermes.CLARIFY_WAIT:
            runner.run([hermes.hermes_bin(), "config", "set", key, value], check=False)
        count += 1
    runner.env = env
    return f"a question waits an hour for your answer ({count} Hermes)"


def hub_key(runner: hermes.Runner, home: Path) -> str:
    """The hub's own key again (2026-11-13): an install made between 10/11 and 13/11 got « clarify.timeout » as its
    hub's API_SERVER_KEY (setup_hub's clarify loop reused the name `key`), which Hermes 0.23 refuses (shorter than
    16): its API server never started, so none of its agents was ever served (a claim answered 502 « the setup could
    not finish »). Such a key, or none, is replaced by a new random one in ~/.hermes/.env, then the gateway restarted
    to listen. Nothing else reads the hub's key (the agents have their own); a good key is left as it is."""
    env = home / ".hermes" / ".env"
    if not (home / ".hermes" / "config.yaml").is_file():
        return "no Hermes of its own here: nothing to do"
    current = ""
    if env.is_file():
        for line in env.read_text(encoding="utf-8", errors="replace").splitlines():
            name, _, value = line.partition("=")
            if name.strip().removeprefix("export ").strip() == "API_SERVER_KEY":
                current = value.strip().strip("'\"")
    if len(current) >= 16 and current not in {name for name, _ in hermes.CLARIFY_WAIT}:
        return "the hub's key is its own already"
    hermes.write_env_file(runner, env, "API_SERVER_KEY", hermes.new_key())
    for unit in GATEWAY_UNITS:
        runner.run(["systemctl", "--user", "try-restart", unit], check=False)
    return "the hub's key made again: its agents are served (gateway restarted)"


# In order; an id never changes once released. The ones from 2026-10-15 to 2026-11-02 left on 2026-11-04 (every
# install had them from that day's release): their code is in git history.
MIGRATIONS: List[Tuple[str, Callable[[hermes.Runner, Path], str]]] = [
    ("2026-11-04-push-relay-service", push_relay_service),
    ("2026-11-10-clarify-hour", clarify_hour),
    ("2026-11-12-push-service-everywhere", push_service_everywhere),
    ("2026-11-13-hub-key", hub_key),
]

# BipAgents' own settings (bridge.toml), never the person's Hermes: applied on an install over their own Hermes too.
BRIDGE_ONLY = {"2026-11-04-push-relay-service", "2026-11-12-push-service-everywhere"}


def done() -> List[str]:
    try:
        return list(json.loads(STATE.read_text(encoding="utf-8")).get("done", []))
    except (OSError, ValueError):
        return []


def mark(ids: List[str], runner: hermes.Runner) -> None:
    runner.log.append(f"write {STATE}")
    if runner.dry:
        return
    STATE.parent.mkdir(parents=True, exist_ok=True)
    STATE.write_text(json.dumps({"done": sorted(set(done()) | set(ids))}, indent=2) + "\n", encoding="utf-8")


def pending() -> List[str]:
    finished = set(done())
    return [mid for mid, _ in MIGRATIONS if mid not in finished]


def run(runner: hermes.Runner, home: Optional[Path] = None, own_hermes: bool = True) -> List[str]:
    """The pending ones, applied (or, on the person's own Hermes, only described). Returns what each did."""
    home = home or Path.home()
    said: List[str] = []
    for mid, apply in MIGRATIONS:
        if mid not in pending():
            continue
        if not own_hermes and mid not in BRIDGE_ONLY:
            said.append(f"{mid}: not applied to your own Hermes (run `install.sh update --apply-to-my-hermes` "
                        "to apply it)")
            continue
        try:
            said.append(apply(runner, home))
        except NotYet as exc:
            said.append(f"{mid}: {exc}")
            continue
        mark([mid], runner)
    return said
