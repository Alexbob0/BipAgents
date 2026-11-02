"""Tests of the styled documents skills' scripts (bridge-rs/assets/skills/slides-styles, docs-styles), run against the
pinned libraries of sandbox/requirements.txt:   python -m pytest -q bridge-rs/tests_skills"""
import os
import shutil
import sys
from pathlib import Path

import pytest

os.environ.setdefault("PYTHONDONTWRITEBYTECODE", "1")   # no __pycache__ in the embedded skills
sys.dont_write_bytecode = True

SKILLS = Path(__file__).resolve().parents[1] / "assets" / "skills"
SLIDES = SKILLS / "slides-styles" / "scripts"
DOCS = SKILLS / "docs-styles" / "scripts"
FIXTURES = Path(__file__).resolve().parent / "fixtures"
for folder in (SLIDES, DOCS):
    if str(folder) not in sys.path:
        sys.path.insert(0, str(folder))

PHOTOS = ["pain.jpg", "cancale2.jpg", "dinan.jpg", "msm.jpg", "saintmalo.jpg", "photo.jpg", "a.jpg", "b.jpg", "c.jpg"]


@pytest.fixture
def workdir(tmp_path):
    """A folder with the fixtures' pictures (plain colour photos and a map)."""
    from PIL import Image
    for i, name in enumerate(PHOTOS):
        Image.new("RGB", (1600, 1000), (40 + 20 * i, 90, 140)).save(tmp_path / name, quality=80)
    Image.new("RGB", (1600, 1000), (230, 230, 220)).save(tmp_path / "map.png")
    Image.new("RGBA", (400, 400), (10, 120, 90, 255)).save(tmp_path / "logo.png")
    for f in FIXTURES.glob("*.json"):
        shutil.copy(f, tmp_path / f.name)
    return tmp_path


@pytest.fixture
def skill_copy(tmp_path):
    """A copy of both skills as the bridge writes them in a profile, where `defaults.json` can be set."""
    out = tmp_path / "skills"
    for name in ("slides-styles", "docs-styles"):
        shutil.copytree(SKILLS / name, out / name, ignore=shutil.ignore_patterns("__pycache__"))
    return out
