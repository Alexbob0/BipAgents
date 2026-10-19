"""The installer's `--cloud FILE` mode: a server of the all-included offer (« no computer », the guichet's reserve),
installed at first boot with nobody at the keyboard (installer/cloud/user-data.yaml).

FILE, written at first boot from that setup: {"v": 1, "pair": "<base64url>", "language": "fr", "profile": "solo"}, and "port" when the door isn't on 443 (a server shared by two people).
`pair` is the one-time pairing secret the guichet hands to the app that bought the server; the profile is « mini »
(no visible browser, for a 2 GB server) or « solo » (the default, 4 GB). No model key: the app sends it to the bridge
once paired (bipbridge/cloud_setup.py), since the provider keeps that setup.

The bridge's door is then on the Internet, on port 443 with its own certificate, the app pinning it; nothing else
listens outside 127.0.0.1. Each step is written to a progress file the bridge shows the app until it is done."""
from __future__ import annotations

import base64
import ipaddress
import json
import os
import socket
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Dict, Optional

PROFILES = ("mini", "solo")
LANGUAGES = ("fr", "en", "de", "es")
DOOR_PORT = 443


class CloudError(ValueError):
    """Safe to show: the file cloud-init wrote is not what the app makes."""


@dataclass
class CloudSetup:
    pair: str
    language: str = "en"
    profile: str = "solo"
    port: int = DOOR_PORT    # the bridge's door; on a server shared by two people, the second one's is 444
    backup: Optional[Dict[str, str]] = None   # the guichet's backup address and this place's token (backup.py)


def read(path: Path) -> CloudSetup:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise CloudError(f"can't read {path}: {exc}") from exc
    if not isinstance(data, dict) or data.get("v") != 1:
        raise CloudError(f"{path} is not a BipAgents cloud setup (v1)")
    pair = str(data.get("pair") or "")
    try:
        secret = base64.urlsafe_b64decode(pair + "=" * (-len(pair) % 4))
    except ValueError as exc:
        raise CloudError("the pairing secret is not base64url") from exc
    if len(secret) < 16:
        raise CloudError("the pairing secret is too short")
    language = str(data.get("language") or "en")[:2].lower()
    profile = str(data.get("profile") or "solo").lower()
    try:
        port = int(data.get("port") or DOOR_PORT)
    except (TypeError, ValueError) as exc:
        raise CloudError("the door's port is not a number") from exc
    if not DOOR_PORT <= port <= 65535:   # the server lets unprivileged accounts listen from 443 up
        raise CloudError(f"the door's port must be {DOOR_PORT} or above")
    backup = data.get("backup")
    if backup is not None:
        url, token = (backup.get("url"), backup.get("token")) if isinstance(backup, dict) else (None, None)
        if not isinstance(url, str) or not url.startswith("https://") or not isinstance(token, str) or len(token) < 16:
            raise CloudError("the backup settings are not an https address and a token")
        backup = {"url": url, "token": token}
    return CloudSetup(pair=pair, language=language if language in LANGUAGES else "en",
                      profile=profile if profile in PROFILES else "solo", port=port, backup=backup)


def _global(address: str) -> bool:
    try:
        return ipaddress.ip_address(address).is_global
    except ValueError:
        return False


def public_address(metadata: Optional[Callable[[], Optional[str]]] = None) -> Optional[str]:
    """This server's public IPv4: the one its main route leaves from (DigitalOcean and OVHcloud put it on the
    interface itself), else what the provider's metadata service says."""
    for target in ("1.1.1.1", "9.9.9.9"):
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
                probe.connect((target, 53))   # no packet is sent: a UDP « connect » only picks the route
                address = probe.getsockname()[0]
        except OSError:
            continue
        if _global(address):
            return address
    found = (metadata or _metadata_address)()
    return found if found and _global(found) else None


def _metadata_address() -> Optional[str]:
    import httpx
    try:   # DigitalOcean's metadata service
        resp = httpx.get("http://169.254.169.254/metadata/v1/interfaces/public/0/ipv4/address", timeout=2)
        return resp.text.strip() if resp.status_code == 200 else None
    except httpx.HTTPError:
        return None


def _write_private(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        json.dump(data, fh)
    os.replace(tmp, path)


def write_setup(path: Path, setup: CloudSetup) -> None:
    """The secret, for the bridge to prove itself and check the app's claim (deleted once claimed)."""
    _write_private(path, {"pair": setup.pair})


class Progress:
    """Each step for the app (the bridge reads it: GET /v1/setup), then « done »."""

    def __init__(self, path: Path, dry: bool = False):
        self.path, self.dry = path, dry

    def __call__(self, step: int, total: int, label: str, done: bool = False) -> None:
        if not self.dry:
            _write_private(self.path, {"step": step, "total": total, "label": label, "done": done})
