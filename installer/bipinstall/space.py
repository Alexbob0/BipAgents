"""`bipagents encrypt`: this account's space encrypted at rest (internal/space-encryption.md, private, bridge-rs/src/space.rs).

Everything personal (Hermes' home, the bridge's config and data, the backup key, the browsers' profiles) moves into
one gocryptfs folder whose key is two shares: one on this server, the other at Kerfy's key service (keys-rs/, at
another provider). After a reboot the bridge fetches it and opens the folder by itself; the agents' services wait for
it (a drop-in each: `ConditionPathExists` on the open folder's marker) and the bridge starts them once it is open.

    bipagents encrypt [--yes] [--keep-rollback] [--allow-other]     encrypt now (the agents stop for a minute or so)
    bipagents encrypt status | unlock | lock | rollback | finish    the bridge's `space` command

The key service: --key-service URL (or $BIPAGENTS_KEYS_URL) and this place's token ($BIPAGENTS_KEYS_TOKEN, else the
guichet's place token from ~/.config/bipagents/backup.json); a server of the offer has both from its setup
(keys.json: the address and the guichet's grant). Off unless run: the offer turns it on per place by putting « keys »
in the place's setup (cloud.py), a self-hosted install only by running this."""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Dict, List, Optional, Sequence

from . import ui

KEYS = Path(".config") / "bipagents" / "keys.json"   # {url, grant, signature}: from the offer's setup
SPACE = Path(".local") / "share" / "bipagents" / "space"
MARKER = ".bipagents-space"
DROP_IN = "50-bipagents-space.conf"
# The services that read or write someone's data: they wait for the open folder. Not the bridge (its locked door
# opens the folder) nor the voices (no one's data).
GATED = ("hermes-gateway*.service", "bipagents-hermes.service", "bipagents-backup.service",
         "bipagents-refresh-browser.service", "bipagents-update.service", "bipagents-browser-*.service")
# Among them, those running all the time: started by the bridge once the folder is open (the others are timers' or
# started when needed).
STARTED = ("hermes-gateway", "bipagents-hermes")

EXPLAIN = """Your space will be encrypted on this server's disk.

  What changes: everything personal (your agents' memories, conversations, files, the bridge's data and keys)
  goes into an encrypted folder. Its key is in two halves: one on this server, the other at Kerfy Studio's key
  service, at another provider. A copy of this disk (a snapshot, a disk taken out) can't be read.
  After a reboot, the bridge fetches the other half and opens your space by itself: nothing for you to do.

  What it does not protect against: Kerfy Studio (which runs the key service), someone reading the running
  server's memory, or a server's host that boots it with its own code (each opening is logged at the key service).

  Your agents stop for a minute or so while it is done. Free disk needed: about as much as your data."""


def home() -> Path:
    return Path(os.environ.get("HOME") or Path.home())


def units_dir(base: Optional[Path] = None) -> Path:
    return (base or home()) / ".config" / "systemd" / "user"


def gated_units(base: Optional[Path] = None) -> List[str]:
    """This account's units that hold someone's data: those written in its systemd folder, and its browsers'
    containers (quadlets, generated as services of the same name)."""
    found = set()
    folder = units_dir(base)
    for pattern in GATED:
        found.update(p.name for p in folder.glob(pattern) if p.is_file())
    quadlets = (base or home()) / ".config" / "containers" / "systemd"
    found.update(p.stem + ".service" for p in quadlets.glob("bipagents-browser-*.container"))
    return sorted(found)


def started_units(gated: Sequence[str]) -> List[str]:
    return [u for u in gated if u.removesuffix(".service").startswith(STARTED)]


def drop_in(space: Path) -> str:
    return ("# BipAgents: this service waits for the encrypted space to be open (bipagents encrypt).\n"
            "[Unit]\n"
            f"ConditionPathExists={space / 'open' / MARKER}\n")


def write_drop_ins(units: Sequence[str], space: Path, base: Optional[Path] = None) -> List[Path]:
    written = []
    for unit in units:
        folder = units_dir(base) / f"{unit}.d"
        folder.mkdir(parents=True, exist_ok=True)
        path = folder / DROP_IN
        path.write_text(drop_in(space), encoding="utf-8")
        written.append(path)
    return written


def remove_drop_ins(base: Optional[Path] = None) -> None:
    for path in units_dir(base).glob(f"*.d/{DROP_IN}"):
        path.unlink(missing_ok=True)
        try:
            path.parent.rmdir()
        except OSError:
            pass


def key_service(args_url: Optional[str], base: Optional[Path] = None) -> Dict[str, str]:
    """The key service's address, this place's token and, the first time, the guichet's grant (never shown)."""
    base = base or home()
    saved = {}
    try:
        saved = json.loads((base / KEYS).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        pass
    url = args_url or os.environ.get("BIPAGENTS_KEYS_URL") or saved.get("url") or ""
    token = os.environ.get("BIPAGENTS_KEYS_TOKEN") or ""
    if not token:
        from . import backup
        token = backup.settings(base).get("token") or ""
    found = {"BIPAGENTS_KEYS_URL": url, "BIPAGENTS_KEYS_TOKEN": token}
    if saved.get("grant") and saved.get("signature"):
        found.update({"BIPAGENTS_KEYS_GRANT": saved["grant"], "BIPAGENTS_KEYS_SIGNATURE": saved["signature"]})
    return found


def bridge_command(base: Optional[Path] = None) -> List[str]:
    from .rustbridge import cli
    return cli(base or home())


def missing_tools() -> Optional[str]:
    if sys.platform != "linux":
        return "Encrypting the space needs Linux (gocryptfs, FUSE)."
    if not shutil.which("gocryptfs"):
        return "gocryptfs is missing: sudo apt install gocryptfs fuse3 (Ubuntu, Debian) or sudo dnf install gocryptfs fuse3 (Fedora)."
    if not (shutil.which("fusermount3") or shutil.which("fusermount")):
        return "FUSE is missing: sudo apt install fuse3 (Ubuntu, Debian) or sudo dnf install fuse3 (Fedora)."
    return None


def run(args: Sequence[str], env: Optional[Dict[str, str]] = None) -> subprocess.CompletedProcess:
    return subprocess.run(list(args), text=True, capture_output=True, env={**os.environ, **(env or {})}, check=False)


def encrypt(url: Optional[str], keep_rollback: bool = False, allow_other: bool = False,
            say=print, runner=run) -> int:
    """Stops the bridge and the agents' services, has the bridge encrypt the space, starts everything again. On a
    failure, the bridge has already put everything back: the drop-ins go and everything starts as before."""
    problem = missing_tools()
    if problem:
        say(ui.sad(problem))
        return 1
    keys = key_service(url)
    if not keys["BIPAGENTS_KEYS_URL"] or not keys["BIPAGENTS_KEYS_TOKEN"]:
        say(ui.sad("No key service: give --key-service URL (and this place's token in $BIPAGENTS_KEYS_TOKEN)."))
        return 1
    space = home() / SPACE
    gated = gated_units()
    started = started_units(gated)
    config = str(home() / ".config" / "bipagents" / "bridge.toml")
    command = bridge_command() + ["space", "enable", "--config", config]
    for unit in started:
        command += ["--unit", unit]
    if keep_rollback:
        command.append("--keep-rollback")
    if allow_other:
        command.append("--allow-other")
    write_drop_ins(gated, space)
    runner(["systemctl", "--user", "daemon-reload"])
    runner(["systemctl", "--user", "stop", "bipagents-bridge.service", *gated])
    result = runner(command, env={**keys, "BRIDGE_CONFIG": config})
    if result.returncode != 0:
        remove_drop_ins()
        runner(["systemctl", "--user", "daemon-reload"])
        runner(["systemctl", "--user", "start", "bipagents-bridge.service", *started])
        said = (result.stderr or result.stdout).strip()[-800:]
        say(ui.sad(f"Not encrypted, everything is as before: {said}"))
        return 1
    runner(["systemctl", "--user", "start", "bipagents-bridge.service", *started])
    say(ui.done("Your space is encrypted. After a reboot it opens by itself."))
    if keep_rollback:
        say(ui.note("The unencrypted copy is kept: `bipagents encrypt finish` deletes it, "
                    "`bipagents encrypt rollback` puts it back."))
    return 0


def main(argv: Optional[List[str]] = None) -> int:
    import argparse
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] in ("status", "unlock", "lock", "rollback", "finish"):
        action = argv[0]
        result = run(bridge_command() + ["space", action, "--config", str(home() / ".config" / "bipagents" / "bridge.toml")],
                     env={"BRIDGE_CONFIG": str(home() / ".config" / "bipagents" / "bridge.toml")})
        print(result.stdout, end="")
        if result.returncode != 0:
            print(result.stderr, end="", file=sys.stderr)
        if action == "rollback" and result.returncode == 0:
            remove_drop_ins()
            run(["systemctl", "--user", "daemon-reload"])
            run(["systemctl", "--user", "start", "bipagents-bridge.service", *started_units(gated_units())])
        return result.returncode
    parser = argparse.ArgumentParser(prog="bipagents encrypt", description="Encrypt this account's space at rest.")
    parser.add_argument("--key-service", help="the key service's address (default: from the offer's setup)")
    parser.add_argument("--yes", action="store_true", help="no question")
    parser.add_argument("--keep-rollback", action="store_true",
                        help="keep the unencrypted copy until `bipagents encrypt finish`")
    parser.add_argument("--allow-other", action="store_true",
                        help="let the agents' browser containers (other user ids) read the open folder "
                             "(needs user_allow_other in /etc/fuse.conf)")
    args = parser.parse_args(argv)
    print(EXPLAIN)
    if not args.yes:
        answer = input("\nEncrypt now? [y/N] ").strip().lower()
        if answer not in ("y", "yes", "o", "oui"):
            print("Nothing changed.")
            return 0
    return encrypt(args.key_service, keep_rollback=args.keep_rollback, allow_other=args.allow_other)
