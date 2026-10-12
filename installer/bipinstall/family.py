"""``bipinstall famille ajouter <nom>``, run by the administrator in their own account: prepares a family member's
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

from . import account, ui
from .ui import tr
from .ports import FIRST_BLOCK, Ports, candidate_bases, port_free

INSTALL_LINE = "curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash -s --"
CONFIG_CANDIDATES = ("~/.config/bipagents/bridge.toml", "~/.config/hermes-ios/bridge.toml")


def find_config(given: Optional[str]) -> Path:
    for raw in ([given] if given else []) + [os.environ.get("BRIDGE_CONFIG", "")] + list(CONFIG_CANDIDATES):
        if raw and Path(raw).expanduser().is_file():
            return Path(raw).expanduser()
    raise SystemExit(tr("Je ne trouve pas la config de ton bridge : donne son chemin avec --config.",
                        "I can't find your bridge's config: give its path with --config."))


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
    raise SystemExit(tr("Plus de bloc de ports libre.", "No free block of ports left."))


def add(name: str, config: Optional[str] = None, is_free=port_free) -> Dict[str, object]:
    config_path = find_config(config)
    text = config_path.read_text(encoding="utf-8")
    public = re.search(r'^\s*public_url\s*=\s*"([^"]+)"', text, re.M)
    if not public:
        raise SystemExit(tr("Ton bridge n'a pas d'adresse sur le tailnet (public_url en haut de bridge.toml) : "
                            "les autres installations ne pourraient pas le joindre pour leurs notifications.",
                            "Your bridge has no address on the tailnet (public_url at the top of bridge.toml): "
                            "the other installs couldn't reach it for their notifications."))
    team = re.search(r'^\s*team_id\s*=\s*"([^"]+)"', text, re.M)
    if not team:
        print(ui.note(tr("Ton bridge n'a pas de clé Apple (APNs) : il ne pourra pas relayer les notifications.",
                         "Your bridge has no Apple key (APNs): it won't be able to relay notifications.")))
    member = slug(name)
    clients = ensure_clients_file(config_path, text)
    lines = clients.read_text(encoding="utf-8").splitlines() if clients.exists() else []
    if any(line.split()[:1] == [member] for line in lines if line.strip() and not line.startswith("#")):
        raise SystemExit(tr(f"{name} a déjà une clé de relais. Pour la refaire, retire sa ligne de {clients}.",
                            f"{name} already has a relay key. To make a new one, remove their line from {clients}."))
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
        return input(f"{ui.question(question)} {tr('[o/N] : ', '[y/N]: ')}").strip().lower() in ("o", "oui", "y", "yes")
    except EOFError:
        return False


def main(argv: List[str]) -> int:
    import argparse
    parser = argparse.ArgumentParser(prog="bipinstall famille", description="Prepare a family member's install")
    parser.add_argument("action", choices=["ajouter"])
    parser.add_argument("name", help="their first name")
    parser.add_argument("--compte", help="their Linux account (default: their first name, lower case, no accents)")
    parser.add_argument("--config", help="your bridge's config (default: found by itself)")
    parser.add_argument("--dry-run", action="store_true",
                        help="only show the commands that would prepare their account, change nothing")
    args = parser.parse_args(argv)
    linux = sys.platform.startswith("linux")
    login = args.compte or slug(args.name)
    total = 4 if linux else 3
    print(ui.bip("happy", tr(f"On prépare la place de {args.name}.", f"Let's get things ready for {args.name}.")))
    if linux:
        print(ui.step(1, total, tr(f"Son compte Linux : {login}", f"Their Linux account: {login}")))
        if not account.prepare(login, confirm, print, dry=args.dry_run):
            print(ui.fail(tr("Son compte n'est pas prêt : rien d'autre n'a été fait.",
                             "Their account isn't ready: nothing else was done.")))
            return 1
        if args.dry_run:
            return 0
        print(ui.done(tr(f"Le compte {login} est prêt (conteneurs sans droits admin, services qui restent lancés).",
                         f"The {login} account is ready (containers without admin rights, services that keep running).")))
    result = add(args.name, args.config)
    ports: Ports = result["ports"]   # type: ignore[assignment]
    print(ui.done(tr(f"Clé de relais créée pour {args.name} (dans {result['clients_file']}).",
                     f"Relay key created for {args.name} (in {result['clients_file']}).")))
    print(ui.done(tr(f"Ports réservés : {ports.base}–{ports.base + 9}.", f"Ports reserved: {ports.base}–{ports.base + 9}.")))
    first = 2 if linux else 1
    print(ui.step(first, total, tr("Redémarre ton bridge, pour qu'il connaisse cette clé",
                                   "Restart your bridge, so it knows this key")))
    line = f"{INSTALL_LINE} --relay-url {result['relay_url']} --relay-key {result['key']} --ports {ports.base}"
    print(ui.step(first + 1, total, tr(f"L'installation de {args.name}, dans son compte",
                                       f"{args.name}'s install, in their account")))
    if linux:
        # Run from here, its questions come in this terminal: in this language, whatever their account's.
        command = account.install_command(login, account.uid_of(login), f"{line} --lang {ui.LANG}")
        if confirm(tr(f"Je la lance maintenant dans le compte {login} (elle te posera les questions ici) ?",
                      f"Shall I run it now in the {login} account (it will ask you its questions here)?")):
            if subprocess.run(command).returncode != 0:
                print(ui.fail(tr("Son installation s'est arrêtée : relance la commande ci-dessous après avoir corrigé.",
                                 "Their install stopped: run the command below again once it's fixed.")))
                print(f"\n  {shlex.join(command)}\n")
                return 1
        else:
            print(tr("Pour la lancer plus tard depuis ton terminal :", "To run it later from your terminal:"))
            print(f"\n  {shlex.join(command)}\n")
    else:
        print(tr(f"{args.name} colle cette ligne dans un terminal de SA session (pas la tienne) :",
                 f"{args.name} pastes this line in a terminal of THEIR session (not yours):"))
        print(f"\n  {line}\n")
    print(ui.note(tr("La clé ne sert qu'à envoyer ses notifications par ton bridge. Ne l'envoie pas par e-mail.",
                     "The key is only used to send their notifications through your bridge. Don't send it by email.")))
    print(ui.step(total, total, tr("Quand son installation est finie, publie ses ports sur le tailnet",
                                   "Once their install is done, publish their ports on the tailnet")))
    for port in ports.tailnet:
        print(f"  sudo tailscale serve --bg --https={port} http://127.0.0.1:{port}")
    print(ui.note(tr("Le dernier ne sert que si son bureau des agents est installé.",
                     "The last one is only needed if their agents' desktop is installed.")))
    return 0
