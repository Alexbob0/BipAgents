"""The styled documents skills ship with the bridge (bridge-rs/src/style_skills.rs, assets/skills/slides-styles and
docs-styles): at start, each agent it manages gets both skills in its profile, Hermes' `powerpoint` and `docx` turned
off in its `skills.disabled`. On a place of the offer at every start; on an install of the person's own once, then
their list is kept."""
import json

import yaml

from bbkit.servers import wait_until
from conftest import managed_install, profile, wait_registered

MODEL = "model:\n  provider: custom\n  default: glm-5.3-flash\n"


def install(bridges, fakes, offer):
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach",))
    folder = profile(home, "coach")
    (folder / "config.yaml").write_text(MODEL + "skills:\n  disabled: [arxiv, slides-styles]\n")
    if offer:
        settings = bridge.home / ".config" / "bipagents" / "backup.json"
        settings.parent.mkdir(parents=True, exist_ok=True)
        settings.write_text(json.dumps({"url": "https://guichet.example/v1/backup", "token": "place-token"}))
    bridge.start()
    wait_registered(bridge, ["coach"])
    return bridge, folder


def disabled(folder):
    return (yaml.safe_load((folder / "config.yaml").read_text()).get("skills") or {}).get("disabled") or []


def shipped(folder):
    skills = folder / "skills"
    return all((skills / name / "SKILL.md").is_file() and (skills / name / "defaults.json").is_file()
               for name in ("slides-styles", "docs-styles"))


def test_an_offer_place_gets_the_skills_in_place_of_hermes_ones_at_every_start(bridges, fakes):
    bridge, folder = install(bridges, fakes, offer=True)
    assert wait_until(lambda: shipped(folder) and "docx" in disabled(folder), timeout=15), bridge.log_tail(2000)
    skills = folder / "skills"
    slides = (skills / "slides-styles" / "SKILL.md").read_text()
    assert "/root/.hermes/skills/slides-styles/scripts/" in slides
    assert (skills / "slides-styles" / "scripts" / "deck.py").is_file()
    assert (skills / "slides-styles" / "scripts" / "metrics.json").is_file()
    assert (skills / "docs-styles" / "scripts" / "doc.py").is_file()
    assert not list(skills.rglob("__pycache__"))
    assert disabled(folder) == ["arxiv", "powerpoint", "docx"]
    assert yaml.safe_load((folder / "config.yaml").read_text())["model"]["default"] == "glm-5.3-flash"   # kept
    # the person (or Hermes) turned ours off and Hermes' on: the offer's choice is back at the next start
    (folder / "config.yaml").write_text(MODEL + "skills:\n  disabled: [docs-styles]\n")
    bridge.restart()
    wait_registered(bridge, ["coach"])
    assert wait_until(lambda: disabled(folder) == ["powerpoint", "docx"], timeout=15), disabled(folder)


def test_an_install_of_the_person_s_own_switches_once_then_keeps_their_choice(bridges, fakes):
    bridge, folder = install(bridges, fakes, offer=False)
    assert wait_until(lambda: shipped(folder) and "docx" in disabled(folder), timeout=15), bridge.log_tail(2000)
    assert disabled(folder) == ["arxiv", "powerpoint", "docx"]
    assert (folder / "skills" / ".bipagents-style-skills.json").is_file()
    # back to Hermes' skills by choice: left as they keep it, while the skills' files stay up to date
    (folder / "config.yaml").write_text(MODEL + "skills:\n  disabled: [slides-styles, docs-styles]\n")
    (folder / "skills" / "slides-styles" / "SKILL.md").write_text("old\n")
    bridge.restart()
    wait_registered(bridge, ["coach"])
    assert wait_until(lambda: (folder / "skills" / "slides-styles" / "SKILL.md").read_text() != "old\n", timeout=15)
    assert disabled(folder) == ["slides-styles", "docs-styles"]
