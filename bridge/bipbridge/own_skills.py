"""The skills an agent wrote itself (Hermes' `skill_manage`: a « remember how to… », or its own review after a task),
listed in the app's advanced tools, where each one can be turned off. Hermes keeps an agent's skills in its profile's
`skills/` folder, next to the ones it ships (`.bundled_manifest`), the ones installed from its hub (`.hub/lock.json`)
and the ones this bridge gives (shared data, the visible browser, contacts): those are not the agent's own. Turning
one off lists it in the profile's `skills.disabled`, which Hermes reads again when the profile's config changes."""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict, List, Set

import yaml

BRIDGE_SKILLS = {p.name for p in Path(__file__).with_name("skills").iterdir() if p.is_dir()} | {"agent-contacts"}


def _frontmatter(path: Path) -> Dict[str, Any]:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return {}
    end = text.find("\n---", 3) if text.startswith("---") else -1
    if end < 0:
        return {}
    try:
        data = yaml.safe_load(text[3:end])
    except yaml.YAMLError:
        return {}
    return data if isinstance(data, dict) else {}


def _bundled(skills: Path) -> Set[str]:
    try:
        lines = (skills / ".bundled_manifest").read_text(encoding="utf-8").splitlines()
    except OSError:
        return set()
    return {line.split(":", 1)[0].strip() for line in lines if line.strip() and not line.startswith("#")}


def _from_hub(skills: Path) -> Set[str]:
    try:
        data = json.loads((skills / ".hub" / "lock.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return set()
    found = data.get("installed", data.get("skills", data)) if isinstance(data, dict) else data
    if isinstance(found, dict):
        return {str(name) for name in found}
    if isinstance(found, list):
        return {str(item["name"]) for item in found if isinstance(item, dict) and item.get("name")}
    return set()


def _config(profile: Path) -> Dict[str, Any]:
    try:
        data = yaml.safe_load((profile / "config.yaml").read_text(encoding="utf-8")) or {}
    except (OSError, yaml.YAMLError):
        return {}
    return data if isinstance(data, dict) else {}


def _disabled(data: Dict[str, Any]) -> List[str]:
    skills = data.get("skills") if isinstance(data.get("skills"), dict) else {}
    found = skills.get("disabled") if isinstance(skills.get("disabled"), list) else []
    return [str(name) for name in found]


def listing(profile: Path) -> List[Dict[str, Any]]:
    """The agent's own skills, from its profile folder: name, description, enabled."""
    skills = profile / "skills"
    if not skills.is_dir():
        return []
    others = _bundled(skills) | _from_hub(skills) | BRIDGE_SKILLS
    off = set(_disabled(_config(profile)))
    found: Dict[str, Dict[str, Any]] = {}
    for skill_md in sorted(skills.rglob("SKILL.md")):
        parts = skill_md.relative_to(skills).parts
        if any(part.startswith(".") for part in parts):   # .archive, .hub, pending writes
            continue
        if any((skills.joinpath(*parts[:depth]) / "SKILL.md").exists() for depth in range(1, len(parts) - 1)):
            continue   # a file inside another skill's folder
        folder = skill_md.parent.name
        meta = _frontmatter(skill_md)
        name = str(meta.get("name") or folder).strip()
        if not name or folder in others or name in others:
            continue
        found[name] = {"name": name, "description": " ".join(str(meta.get("description") or "").split())[:300],
                       "enabled": name not in off}
    return sorted(found.values(), key=lambda skill: skill["name"].lower())


def set_enabled(profile: Path, changes: Dict[str, bool]) -> None:
    """Turns some of the agent's own skills on or off (others, Hermes' or the hub's, are not this switch's)."""
    own = {skill["name"] for skill in listing(profile)}
    unknown = sorted(set(changes) - own)
    if unknown:
        raise LookupError(", ".join(unknown))
    data = _config(profile)
    off = [name for name in _disabled(data) if changes.get(name) is not True]
    off += [name for name, on in sorted(changes.items()) if not on and name not in off]
    section = data.get("skills") if isinstance(data.get("skills"), dict) else {}
    section["disabled"] = off
    data["skills"] = section
    (profile / "config.yaml").write_text(yaml.safe_dump(data, allow_unicode=True, sort_keys=False), encoding="utf-8")
