"""``bipinstall famille ajouter <nom>``, run by the administrator in their own account: prepares a family member's
install on the same server. It gives them a relay key on the administrator's bridge (their notifications go out
through it, the APNs key never leaves it), reserves their block of ports, and prints the one line they paste in their
own session, then the `tailscale serve` commands to run once their install is done. Nothing in their account is
touched: they install for themselves, without admin rights."""
from __future__ import annotations

import json
import os
import re
import secrets
from pathlib import Path
from typing import Dict, List, Optional

from . import ui
from .ports import FIRST_BLOCK, Ports, candidate_bases, port_free

INSTALL_LINE = "curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash -s --"
CONFIG_CANDIDATES = ("~/.config/bipagents/bridge.toml", "~/.config/hermes-ios/bridge.toml")


def find_config(given: Optional[str]) -> Path:
    for raw in ([given] if given else []) + [os.environ.get("BRIDGE_CONFIG", "")] + list(CONFIG_CANDIDATES):
        if raw and Path(raw).expanduser().is_file():
            return Path(raw).expanduser()
    raise SystemExit("Je ne trouve pas la config de ton bridge : donne son chemin avec --config.")


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
    raise SystemExit("Plus de bloc de ports libre.")


def add(name: str, config: Optional[str] = None, is_free=port_free) -> Dict[str, object]:
    config_path = find_config(config)
    text = config_path.read_text(encoding="utf-8")
    public = re.search(r'^\s*public_url\s*=\s*"([^"]+)"', text, re.M)
    if not public:
        raise SystemExit("Ton bridge n'a pas d'adresse sur le tailnet (public_url en haut de bridge.toml) : "
                         "les autres installations ne pourraient pas le joindre pour leurs notifications.")
    team = re.search(r'^\s*team_id\s*=\s*"([^"]+)"', text, re.M)
    if not team:
        print(ui.note("Ton bridge n'a pas de clé Apple (APNs) : il ne pourra pas relayer les notifications."))
    member = slug(name)
    clients = ensure_clients_file(config_path, text)
    lines = clients.read_text(encoding="utf-8").splitlines() if clients.exists() else []
    if any(line.split()[:1] == [member] for line in lines if line.strip() and not line.startswith("#")):
        raise SystemExit(f"{name} a déjà une clé de relais. Pour la refaire, retire sa ligne de {clients}.")
    key = secrets.token_urlsafe(32)
    clients.write_text("\n".join(lines + [f"{member} {key}"]) + "\n", encoding="utf-8")
    clients.chmod(0o600)
    blocks = reserved_blocks(config_path.parent)
    ports = next_block(list(blocks.values()), is_free)
    blocks[member] = ports.base
    (config_path.parent / "family-ports.json").write_text(json.dumps(blocks, indent=2) + "\n", encoding="utf-8")
    return {"member": member, "key": key, "relay_url": public.group(1).rstrip("/"), "ports": ports,
            "clients_file": str(clients)}


def main(argv: List[str]) -> int:
    import argparse
    parser = argparse.ArgumentParser(prog="bipinstall famille", description="Prepare a family member's install")
    parser.add_argument("action", choices=["ajouter"])
    parser.add_argument("name", help="their first name")
    parser.add_argument("--config", help="your bridge's config (default: found by itself)")
    args = parser.parse_args(argv)
    result = add(args.name, args.config)
    ports: Ports = result["ports"]   # type: ignore[assignment]
    print(ui.bip("happy", f"On prépare la place de {args.name}."))
    print(ui.done(f"Clé de relais créée pour {args.name} (dans {result['clients_file']})."))
    print(ui.done(f"Ports réservés : {ports.base}–{ports.base + 9}."))
    print(ui.step(1, 3, "Redémarre ton bridge, pour qu'il connaisse cette clé"))
    print(ui.step(2, 3, f"{args.name} colle cette ligne dans un terminal de SA session (pas la tienne)"))
    print(f"\n  {INSTALL_LINE} --relay-url {result['relay_url']} --relay-key {result['key']} --ports {ports.base}\n")
    print(ui.note("La clé ne sert qu'à envoyer ses notifications par ton bridge. Transmets-la-lui en direct, pas par e-mail."))
    print(ui.step(3, 3, "Quand son installation est finie, publie ses ports sur le tailnet"))
    for port in ports.tailnet:
        print(f"  sudo tailscale serve --bg --https={port} http://127.0.0.1:{port}")
    print(ui.note("Le dernier ne sert que si son bureau des agents est installé."))
    return 0
