"""Kinds of agents and their starting personality (SOUL.md), shared by the installer and the bridge (agents created
from the app). A kind is not a name: the person names each agent; « Autre » is described by the person.

Each text exists in French and English (`agent_templates.yaml`); another language gets the English one, so an agent's
personality — and the language it thinks in — follows the person's: the installer's language, or the iPhone's."""
from __future__ import annotations

from pathlib import Path
from typing import Any, Dict, List, Optional

import yaml

TEMPLATES_PATH = Path(__file__).with_name("agent_templates.yaml")
CATEGORIES = ["daily", "wellness", "finance", "home", "work", "learning", "creative", "tech"]
LANGUAGES = ("fr", "en")


def language(code: Optional[str]) -> str:
    """« fr-FR », « en_US », « de »… → the templates' language: French or English (everything else)."""
    return "fr" if (code or "").lower().startswith("fr") else "en"


def _pick(value: Any, lang: str) -> str:
    if isinstance(value, dict):
        return value.get(lang) or value.get("en") or value.get("fr") or ""
    return value or ""


def templates(lang: Optional[str] = "fr") -> List[Dict[str, Any]]:
    """The kinds with their texts in `lang` (a locale code is fine)."""
    lang = language(lang)
    raw = yaml.safe_load(TEMPLATES_PATH.read_text(encoding="utf-8"))
    return [{**t, "label": _pick(t["label"], lang), "description": _pick(t["description"], lang),
             "soul": _pick(t["soul"], lang)} for t in raw]


def template(category: str, lang: Optional[str] = "fr") -> Optional[Dict[str, Any]]:
    return next((t for t in templates(lang) if t["category"] == category), None)


def soul_for(name: str, category: str, lang: Optional[str] = "fr") -> str:
    found = template(category, lang)
    if not found or not found["soul"]:
        raise ValueError(f"no template for {category!r}")
    return found["soul"].replace("{name}", name)


def custom_soul(name: str, role: str, tone: str = "", rules: str = "", language_name: str = "",
                lang: Optional[str] = "fr") -> str:
    """« Autre »: the role, tone, rules and reply language the person wrote, as a SOUL.md (in `lang`)."""
    role, rules = role.strip().rstrip("."), rules.strip().rstrip(".")
    if language(lang) == "fr":
        soul = (f"Tu es {name}, un agent personnel de ton utilisateur. Ton rôle : {role}.\n"
                f"Ton ton : {(tone or 'clair et bienveillant').strip()}. Tu réponds en "
                f"{(language_name or 'français').strip()}, de façon concise.\n")
        if rules:
            soul += f"Règles à respecter : {rules}.\n"
        return soul + "Tu demandes avant toute action irréversible (envoyer, acheter, supprimer).\n"
    soul = (f"You are {name}, your user's personal agent. Your role: {role}.\n"
            f"Your tone: {(tone or 'clear and kind').strip()}. You answer in {(language_name or 'English').strip()}, "
            f"concisely.\n")
    if rules:
        soul += f"Rules to follow: {rules}.\n"
    return soul + "You ask before anything irreversible (sending, buying, deleting).\n"


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
