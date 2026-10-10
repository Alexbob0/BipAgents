"""Kinds of agents and their starting personality (SOUL.md), shared by the installer and the bridge (agents created
from the app). A kind is not a name: the person names each agent; « Autre » is described by the person."""
from __future__ import annotations

from pathlib import Path
from typing import Any, Dict, List, Optional

import yaml

TEMPLATES_PATH = Path(__file__).with_name("agent_templates.yaml")
CATEGORIES = ["daily", "wellness", "finance", "home", "work", "learning", "creative", "tech"]


def templates() -> List[Dict[str, Any]]:
    return yaml.safe_load(TEMPLATES_PATH.read_text(encoding="utf-8"))


def template(category: str) -> Optional[Dict[str, Any]]:
    return next((t for t in templates() if t["category"] == category), None)


def soul_for(name: str, category: str) -> str:
    found = template(category)
    if not found or not found["soul"]:
        raise ValueError(f"no template for {category!r}")
    return found["soul"].replace("{name}", name)


def custom_soul(name: str, role: str, tone: str = "", rules: str = "", language: str = "français") -> str:
    """« Autre »: the role, tone, rules and language the person wrote, as a SOUL.md."""
    soul = (f"Tu es {name}, un agent personnel de ton utilisateur. Ton rôle : {role.strip().rstrip('.')}.\n"
            f"Ton ton : {(tone or 'clair et bienveillant').strip()}. Tu réponds en {(language or 'français').strip()}, "
            f"de façon concise.\n")
    if rules.strip():
        soul += f"Règles à respecter : {rules.strip().rstrip('.')}.\n"
    soul += "Tu demandes avant toute action irréversible (envoyer, acheter, supprimer).\n"
    return soul


def profile_name(display_name: str, taken=()) -> str:
    """« Léa » → « lea », « Mon coach » → « mon-coach »: the profile (and bridge) id of a name the person chose."""
    import unicodedata
    plain = unicodedata.normalize("NFKD", display_name).encode("ascii", "ignore").decode().lower()
    slug = "-".join("".join(c if c.isalnum() else " " for c in plain).split())[:32] or "agent"
    if slug == "default":
        slug = "agent-default"
    taken = set(taken)
    name, n = slug, 2
    while name in taken:
        name, n = f"{slug}-{n}", n + 1
    return name
