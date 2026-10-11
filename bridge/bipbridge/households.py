"""Agents of two households (two BipAgents installs, e.g. Alex's and Léa's) that may write to each other.

Linking two installs: one person shows an invitation (a QR code: this bridge's address and a one-time code, valid ten
minutes); the other scans it, and their bridge introduces itself with the code. Each bridge then gets a key of its
own for the other, which only lets it hand messages over, and only along links each person turned on:

- on Alex's side, « Vie peut écrire à Cuisine (Léa) » (the link vie → cuisine__lea);
- on Léa's side, « Vie (Alex) peut écrire à Cuisine » (the link vie__alex → cuisine).

An agent writes to `bip/cuisine__lea` (its bridge is its only peer, see peers.py); its bridge checks its own link and
hands the message to Léa's bridge (`/family/p/cuisine/…`), which checks hers and delivers it, signed
« Message from Vie (Alex) (@vie__alex) » so Cuisine can answer the same way. No agent ever gets another's key.
`<data>/households.json` holds this install's name, the linked households (their address, the key we send, a digest
of the key they send) and the pending invitations.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import secrets
import time
import unicodedata
from pathlib import Path
from typing import Any, Dict, List, Optional

from .config import Config

SEPARATOR = "__"                  # « cuisine__lea »: agent cuisine of the household lea
INVITE_SECONDS = 600


class HouseholdError(Exception):
    """Safe to show."""


def _digest(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()


def slug(name: str) -> str:
    plain = unicodedata.normalize("NFKD", name).encode("ascii", "ignore").decode().lower()
    return re.sub(r"[^a-z0-9]+", "-", plain).strip("-")[:24] or "famille"


def split(target: str) -> Optional[tuple]:
    """(agent, household) for « cuisine__lea », None for a local agent."""
    if SEPARATOR not in target:
        return None
    agent, _, household = target.rpartition(SEPARATOR)
    return (agent, household) if agent and household else None


class Households:
    def __init__(self, path: str):
        self.path = Path(path)

    def load(self) -> Dict[str, Any]:
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            data = {}
        return {"name": data.get("name", ""), "households": data.get("households", {}),
                "invites": {k: v for k, v in data.get("invites", {}).items() if v > time.time()}}

    def save(self, data: Dict[str, Any]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_name(self.path.name + ".tmp")
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(data, fh)
        os.replace(tmp, self.path)

    def name(self) -> str:
        return self.load()["name"]

    def set_name(self, name: str) -> None:
        name = name.strip()[:40]
        if not name:
            raise HouseholdError("a name is needed")
        data = self.load()
        data["name"] = name
        self.save(data)

    def invite(self) -> str:
        """A one-time code for the other household's bridge (ten minutes)."""
        if not self.name():
            raise HouseholdError("set this household's name first")
        code = secrets.token_urlsafe(24)
        data = self.load()
        data["invites"][_digest(code)] = time.time() + INVITE_SECONDS
        self.save(data)
        return code

    def accept(self, code: str, name: str, url: str, their_key: str) -> Dict[str, str]:
        """The other bridge introduces itself with our code and the key it will call us with (we keep its digest).
        Returns the key we will call it with, and our name."""
        data = self.load()
        if data["invites"].pop(_digest(code), None) is None:
            raise HouseholdError("this invitation is unknown or expired")
        ours = secrets.token_urlsafe(32)
        self._add(data, name, url, key_out=ours, key_in=their_key)
        self.save(data)
        return {"key": ours, "name": data["name"]}

    def joined(self, name: str, url: str, key_out: str, key_in: str) -> str:
        """We scanned their invitation and they accepted: `key_out` is ours to call them, `key_in` theirs to call
        us."""
        data = self.load()
        household = self._add(data, name, url, key_out=key_out, key_in=key_in)
        self.save(data)
        return household

    def _add(self, data: Dict[str, Any], name: str, url: str, key_out: str, key_in: str) -> str:
        if not name.strip() or not url.startswith(("http://", "https://")) or min(len(key_out), len(key_in)) < 32:
            raise HouseholdError("invalid household")
        household = slug(name)
        if household == slug(data["name"]):
            raise HouseholdError("a household cannot link to itself (or to one with the same name)")
        data["households"][household] = {"name": name.strip()[:40], "url": url.rstrip("/"), "key_out": key_out,
                                         "key_in": _digest(key_in)}
        return household

    def caller(self, token: str) -> Optional[str]:
        digest = _digest(token)
        return next((h for h, v in self.load()["households"].items()
                     if v.get("key_in") and secrets.compare_digest(v["key_in"], digest)), None)

    def get(self, household: str) -> Optional[Dict[str, str]]:
        return self.load()["households"].get(household)

    def remove(self, household: str) -> None:
        data = self.load()
        if data["households"].pop(household, None) is None:
            raise KeyError(household)
        self.save(data)

    def public(self) -> List[Dict[str, str]]:
        return [{"id": h, "name": v["name"], "url": v["url"]} for h, v in sorted(self.load()["households"].items())]


def path_for(config: Config) -> str:
    return str(Path(config.outbox.db_path).parent / "households.json")


def own_url(config: Config) -> Optional[str]:
    """Where the other household's bridge reaches this one (its tailnet address)."""
    return config.public_url
