"""``bipinstall family add <name>``, run by the administrator in their own account: prepares a family member's
install on the same server. On Linux it first gets their account ready (created if needed, ID ranges for rootless
podman, linger: `account.py`, each sudo command shown and confirmed). It gives them a relay key on the
administrator's bridge (their notifications go out through it, the APNs key never leaves it), reserves their block of
ports, offers to run their install in their account right away (or prints the line to paste in their session), then
the `tailscale serve` commands to run once it is done. Their install itself runs as them, without admin rights."""
from __future__ import annotations

import json
import os
import re
import secrets
import shlex
import subprocess
import sys
from pathlib import Path
from typing import Dict, List, Optional
from urllib.parse import urlparse

from . import account, ui
from .ports import FIRST_BLOCK, Ports, candidate_bases, port_free

INSTALL_LINE = "curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash -s --"
CONFIG_CANDIDATES = ("~/.config/bipagents/bridge.toml", "~/.config/hermes-ios/bridge.toml")


def find_config(given: Optional[str]) -> Path:
    for raw in ([given] if given else []) + [os.environ.get("BRIDGE_CONFIG", "")] + list(CONFIG_CANDIDATES):
        if raw and Path(raw).expanduser().is_file():
            return Path(raw).expanduser()
    raise SystemExit("I can't find your bridge's config: give its path with --config.")


def bridge_restart(home: Optional[Path] = None) -> List[str]:
    """The command restarting the administrator's bridge: the service whose command runs `bipbridge` (an install
    made before the installer may have named it otherwise), else the installer's name."""
    home = home or Path.home()
    if sys.platform == "darwin":
        label = "io.github.bipagents.bridge"
        for plist in sorted((home / "Library" / "LaunchAgents").glob("*.plist")):
            if "bipbridge" in plist.read_text(errors="ignore"):
                label = plist.stem
                break
        return ["launchctl", "kickstart", "-k", f"gui/{os.getuid()}/{label}"]
    unit = "bipagents-bridge"
    for path in sorted((home / ".config" / "systemd" / "user").glob("*.service")):
        if "bipbridge" in path.read_text(errors="ignore"):
            unit = path.stem
            break
    return ["systemctl", "--user", "restart", unit]


def slug(name: str) -> str:
    import unicodedata
    plain = unicodedata.normalize("NFKD", name).encode("ascii", "ignore").decode().lower()
    return re.sub(r"[^a-z0-9]+", "-", plain).strip("-") or "membre"


def ensure_clients_file(config_path: Path, text: str) -> Path:
    """The relay keys live in a file next to bridge.toml (« name key » per line), named in its [relay] table."""
    found = re.search(r'^\s*clients_file\s*=\s*"([^"]+)"', text, re.M)
    if found:
        return Path(found.group(1)).expanduser()
    path = config_path.parent / "relay-clients"
    line = f'clients_file = "{path}"'
    if re.search(r"^\[relay\]\s*$", text, re.M):
        text = re.sub(r"^\[relay\]\s*$", f"[relay]\n{line}", text, count=1, flags=re.M)
    else:
        text = text.rstrip("\n") + f"\n\n# Other installs on this server send their notifications through this bridge.\n[relay]\n{line}\n"
    config_path.write_text(text, encoding="utf-8")
    return path


def reserved_blocks(folder: Path) -> Dict[str, int]:
    try:
        return json.loads((folder / "family-ports.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}


def next_block(taken: List[int], is_free=port_free) -> Ports:
    for base in candidate_bases():
        ports = Ports(base)
        if base == FIRST_BLOCK:
            continue   # the administrator's own block, even when nothing listens there right now
        if base not in taken and all(is_free(p) for p in range(base, base + 10)):
            return ports
    raise SystemExit("No free block of ports left.")


def add(name: str, config: Optional[str] = None, is_free=port_free) -> Dict[str, object]:
    config_path = find_config(config)
    text = config_path.read_text(encoding="utf-8")
    public = re.search(r'^\s*public_url\s*=\s*"([^"]+)"', text, re.M)
    if not public:
        raise SystemExit("Your bridge has no address on the tailnet (public_url at the top of bridge.toml): "
                            "the other installs couldn't reach it for their notifications.")
    team = re.search(r'^\s*team_id\s*=\s*"([^"]+)"', text, re.M)
    if not team:
        print(ui.note("Your bridge has no Apple key (APNs): it won't be able to relay notifications."))
    member = slug(name)
    clients = ensure_clients_file(config_path, text)
    lines = clients.read_text(encoding="utf-8").splitlines() if clients.exists() else []
    if any(line.split()[:1] == [member] for line in lines if line.strip() and not line.startswith("#")):
        raise SystemExit(f"{name} already has a relay key. To make a new one, remove their line from {clients}.")
    key = secrets.token_urlsafe(32)
    clients.write_text("\n".join(lines + [f"{member} {key}"]) + "\n", encoding="utf-8")
    clients.chmod(0o600)
    blocks = reserved_blocks(config_path.parent)
    ports = next_block(list(blocks.values()), is_free)
    blocks[member] = ports.base
    (config_path.parent / "family-ports.json").write_text(json.dumps(blocks, indent=2) + "\n", encoding="utf-8")
    return {"member": member, "key": key, "relay_url": public.group(1).rstrip("/"), "ports": ports,
            "clients_file": str(clients)}


def confirm(question: str) -> bool:
    try:
        return input(f"{ui.question(question)} [y/N]: ").strip().lower() in ("y", "yes")
    except EOFError:
        return False


def main(argv: List[str]) -> int:
    import argparse
    parser = argparse.ArgumentParser(prog="bipinstall family", description="Prepare a family member's install")
    parser.add_argument("action", choices=["add", "ajouter"])   # « ajouter »: its first name, kept working
    parser.add_argument("name", help="their first name")
    parser.add_argument("--account", "--compte", dest="account",
                        help="their Linux account (default: their first name, lower case, no accents)")
    parser.add_argument("--config", help="your bridge's config (default: found by itself)")
    parser.add_argument("--dry-run", action="store_true",
                        help="only show the commands that would prepare their account, change nothing")
    args = parser.parse_args(argv)
    linux = sys.platform.startswith("linux")
    login = args.account or slug(args.name)
    total = 5 if linux else 4
    print(ui.bip("happy", f"Let's get things ready for {args.name}."))
    if linux:
        print(ui.step(1, total, f"Their Linux account: {login}"))
        if not account.prepare(login, confirm, print, dry=args.dry_run):
            print(ui.fail("Their account isn't ready: nothing else was done."))
            return 1
        if args.dry_run:
            return 0
        print(ui.done(f"The {login} account is ready (containers without admin rights, services that keep running)."))
    result = add(args.name, args.config)
    ports: Ports = result["ports"]   # type: ignore[assignment]
    print(ui.done(f"Relay key created for {args.name} (in {result['clients_file']})."))
    print(ui.done(f"Ports reserved: {ports.base}–{ports.base + 9}."))
    first = 2 if linux else 1
    print(ui.step(first, total, "Restart your bridge, so it knows this key"))
    restart = bridge_restart()
    if confirm(f"Shall I restart it now ({shlex.join(restart)})?") and subprocess.run(restart).returncode == 0:
        print(ui.done("Your bridge restarted."))
    else:
        print("Restart it from another terminal of YOUR account before going on:")
        print(f"\n  {shlex.join(restart)}\n")
    line = f"{INSTALL_LINE} --relay-url {result['relay_url']} --relay-key {result['key']} --ports {ports.base}"
    print(ui.step(first + 1, total, f"{args.name}'s install, in their account"))
    if linux:
        command = account.install_command(login, account.uid_of(login), line)
        print(ui.note(f"No need to log into {login}: their install runs from here through sudo, and its questions "
                      "show up in this terminal. You answer them for them."))
        installed = confirm(f"Shall I run it now in the {login} account (it will ask you its questions here)?")
        if installed:
            if subprocess.run(command).returncode != 0:
                print(ui.fail("Their install stopped: run the command below again once it's fixed."))
                print(f"\n  {shlex.join(command)}\n")
                return 1
        else:
            print("To run it later from your terminal:")
            print(f"\n  {shlex.join(command)}\n")
    else:
        installed = False
        print(f"{args.name} pastes this line in a terminal of THEIR session (not yours):")
        print(f"\n  {line}\n")
    print(ui.note("The key is only used to send their notifications through your bridge. Don't send it by email."))
    print(ui.step(total - 1, total, "Publish their ports on the tailnet"))
    serve = [["sudo", "tailscale", "serve", "--bg", f"--https={port}", f"http://127.0.0.1:{port}"] for port in ports.tailnet]
    if installed and confirm("Shall I publish them now (with sudo)?") and \
            all(subprocess.run(command).returncode == 0 for command in serve):
        print(ui.done(f"Ports {', '.join(str(p) for p in ports.tailnet)} published on your tailnet."))
    else:
        print("Once their install is done, run from YOUR account:")
        for command in serve:
            print(f"  {shlex.join(command)}")
        print(ui.note("The last one is only needed if their agents' desktop is installed."))
    print(ui.step(total, total, "Their iPhone, on YOUR tailnet"))
    for line in iphone_steps(args.name, result["relay_url"]):
        print(line)
    if linux:
        print(f"To show their QR code again:\n  bipagents qr {login}")
    return 0


def iphone_steps(name: str, relay_url: str) -> List[str]:
    """Their iPhone has to join the administrator's tailnet: a Tailscale account of their own makes a tailnet of its
    own, where this server isn't."""
    host = urlparse(relay_url).hostname or ""
    machine, _, tailnet = host.partition(".")
    tailnet = tailnet or "yours"
    return [
        f"{name}'s iPhone must be in YOUR tailnet ({tailnet}), where {machine or 'this server'} is. With a Tailscale "
        "account of their own, it is in another one and the app can't reach their agents.",
        "  1. Invite them: https://login.tailscale.com/admin/users › Invite users (their email).",
        "  2. On their iPhone, they accept the invite, then in the Tailscale app: profile icon › Add account › sign in",
        f"     with the invited email › pick the {tailnet} tailnet.",
        f"  3. Once the Tailscale app lists {machine or 'this server'}: Tailscale on, then the BipAgents app (TestFlight or",
        "     App Store) › \"+\" › Scan QR code (the one their install showed).",
        "  Or, simpler but their iPhone is then one of your devices: sign Tailscale in with YOUR account.",
    ]
