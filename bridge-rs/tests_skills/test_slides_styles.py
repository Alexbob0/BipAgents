"""slides-styles: every layout in every style renders inside its boxes (by the six fonts' real advance widths), charts
and tables stay editable, credits and defaults behave."""
import json
import os
import subprocess
import sys

import pytest
from pptx import Presentation
from pptx.util import Emu

from conftest import SLIDES

import deck  # noqa: E402
import styles  # noqa: E402

FONTS = {"Arial", "Georgia", "Verdana", "Trebuchet MS", "Times New Roman", "Courier New"}
CREDIT = "Jean Dupont · CC BY-SA 4.0 · Wikimedia Commons"

ALL_LAYOUTS = {
    "title": "Le Fournil de Léa",
    "slides": [
        {"layout": "cover", "title": "Le Fournil de Léa", "subtitle": "Boulangerie bio au levain naturel · Nantes",
         "kicker": "Dossier investisseurs", "image": "pain.jpg", "credit": CREDIT},
        {"layout": "agenda", "title": "Au programme"},
        {"layout": "section", "title": "Le projet", "subtitle": "Pourquoi une boulangerie de quartier"},
        {"layout": "bullets", "title": "Une offre simple et locale",
         "bullets": ["Pain au levain naturel, farines de la région", "Viennoiseries pur beurre faites sur place",
                     "Ouvert 7 jours sur 7, de 7 h à 20 h", "Commandes en ligne et retrait en boutique",
                     "Ateliers du samedi pour les enfants du quartier"]},
        {"layout": "two_columns", "title": "Forces et risques",
         "left": {"heading": "Forces", "bullets": ["12 ans de métier", "Quartier sans boulangerie bio"]},
         "right": {"heading": "Risques", "text": "Hausse du prix des farines et de l'énergie, concurrence des chaînes."}},
        {"layout": "big_number", "value": "+18 %", "label": "de fréquentation attendue la deuxième année",
         "context": "D'après la fréquentation de trois boulangeries comparables du centre de Nantes."},
        {"layout": "stats", "title": "Les chiffres clés",
         "stats": [{"value": "160 k€", "label": "chiffre d'affaires 2026", "delta": "+12 %"},
                   {"value": "4", "label": "emplois créés"}, {"value": "350", "label": "clients par jour"}],
         "note": "Prévisions prudentes."},
        {"layout": "chart", "title": "Chiffre d'affaires prévisionnel",
         "chart": {"type": "bar", "categories": ["2026", "2027", "2028"], "series": {"CA": [160, 240, 310]},
                   "unit": "k€", "highlight": "2028"},
         "takeaway": "Le chiffre d'affaires double en deux ans.", "source": "Prévisionnel 2026"},
        {"layout": "chart", "title": "Répartition des ventes",
         "chart": {"type": "doughnut", "categories": ["Pains", "Viennoiseries", "Pâtisseries"],
                   "series": {"Part": [55, 30, 15]}, "unit": "%"}},
        {"layout": "table", "title": "Budget de lancement",
         "rows": [["Poste", "Montant"], ["Four à sole", "38 000 €"], ["Travaux", "25 000 €"], ["Total", "63 000 €"]],
         "note": "Hors taxes.", "source": "Devis reçus en octobre"},
        {"layout": "image_text", "title": "L'équipe", "image": "photo.jpg", "credit": CREDIT,
         "text": "Léa Martin, boulangère depuis douze ans, et Karim Benali, pâtissier : deux artisans complémentaires.",
         "caption": "Léa au fournil", "kicker": "Les fondateurs"},
        {"layout": "image_full", "image": "a.jpg", "title": "Le quartier Saint-Félix", "caption": "Nantes",
         "credit": CREDIT},
        {"layout": "gallery", "title": "Nos produits",
         "images": [{"path": "a.jpg", "caption": "Levain", "credit": CREDIT}, {"path": "b.jpg", "caption": "Brioche"},
                    {"path": "c.jpg", "caption": "Tartes"}]},
        {"layout": "timeline", "title": "Les étapes",
         "steps": [{"label": "Janvier", "text": "Signature du bail"}, {"label": "Mars", "text": "Travaux et four"},
                   {"label": "Avril", "text": "Ouverture"}, {"label": "Juin", "text": "Ateliers enfants"}]},
        {"layout": "people", "title": "Qui fait quoi",
         "people": [{"name": "Léa Martin", "role": "Boulangère", "text": "Levain et pains"},
                    {"name": "Karim Benali", "role": "Pâtissier", "text": "Viennoiseries"}]},
        {"layout": "quote", "quote": "Le bon pain, c'est du temps, de la farine et de l'eau.", "author": "Léa Martin"},
        {"layout": "code", "title": "Commande en ligne", "code": "GET /api/commandes?jour=samedi\n200 OK",
         "caption": "L'API du site"},
        {"layout": "closing", "title": "Merci", "subtitle": "Questions ?", "contact": "lea@exemple.fr"},
    ],
}


def build(workdir, spec, name="deck.pptx", style=None):
    path = workdir / "spec.json"
    path.write_text(json.dumps(spec, ensure_ascii=False), encoding="utf-8")
    out = workdir / name
    args = [str(path), str(out)] + (["--style", style] if style else [])
    deck.main(args)
    return Presentation(str(out))


def overflows(prs):
    """Text boxes whose text needs more height than the box has, measured like deck.py measures it."""
    found = []
    for n, slide in enumerate(prs.slides, 1):
        for shape in slide.shapes:
            if not shape.has_text_frame or len(shape.text_frame.text.strip()) <= 2 or shape.rotation:
                continue   # a decorative mark (« “ », a numeral) is drawn as a shape, not set as text
            w, h = Emu(shape.width).inches, Emu(shape.height).inches
            need = 0.0
            for p in shape.text_frame.paragraphs:
                runs = [r for r in p.runs if r.text]
                if not runs:
                    continue
                f = runs[0].font
                size = f.size.pt if f.size else 18
                spc = runs[0]._r.get_or_add_rPr().get("spc")
                track = int(spc) / 100 if spc else 0.0
                text = "".join(r.text for r in runs)
                lines = deck.lines_needed(text, w, size, f.name or "Arial", bool(f.bold), track)
                if lines is None:
                    found.append((n, text[:40], "a word wider than its box"))
                    break
                need += lines * size * (p.line_spacing or 1.0) * 1.2 / 72
            if need > h * 1.08 + 0.05:
                found.append((n, shape.text_frame.text[:40], f"{need:.2f} in for {h:.2f}"))
    return found


@pytest.mark.parametrize("style", sorted(styles.STYLES))
def test_every_layout_fits_its_boxes_in_every_style(workdir, style):
    prs = build(workdir, ALL_LAYOUTS, style=style)
    assert len(prs.slides) == len(ALL_LAYOUTS["slides"])
    assert round(Emu(prs.slide_width).inches, 2) == 13.33 and round(Emu(prs.slide_height).inches, 2) == 7.5
    assert overflows(prs) == []
    used = {r.font.name for s in prs.slides for sh in s.shapes if sh.has_text_frame
            for p in sh.text_frame.paragraphs for r in p.runs if r.font.name}
    assert used and used <= FONTS, used - FONTS


@pytest.mark.parametrize("fixture", ["2-pitch-boulangerie.json", "3-voyage-bretagne.json", "4-budget-octobre.json"])
def test_glm_decks_fit_their_boxes_in_every_style(workdir, fixture):
    spec = json.loads((workdir / fixture).read_text(encoding="utf-8"))
    for style in styles.STYLES:
        prs = build(workdir, spec, style=style)
        assert overflows(prs) == [], (fixture, style)


def test_the_metrics_cover_the_six_fonts_regular_and_bold():
    metrics = json.loads((SLIDES / "metrics.json").read_text(encoding="utf-8"))
    for font in FONTS:
        regular, bold = metrics[font]
        assert len(regular) == len(bold) == len(metrics["chars"])
    for st in styles.STYLES.values():
        assert {st["head"], st["body"], st["lab"]} <= FONTS
    # Verdana is wider than Arial, Courier is monospaced
    assert deck.width_in("Boulangerie", 20, "Verdana") > deck.width_in("Boulangerie", 20, "Arial")
    assert deck.width_in("iiii", 20, "Courier New") == pytest.approx(deck.width_in("MMMM", 20, "Courier New"))


def test_charts_and_tables_stay_editable(workdir):
    prs = build(workdir, ALL_LAYOUTS, style="rapport")
    charts = [sh for s in prs.slides for sh in s.shapes if sh.has_chart]
    assert len(charts) == 2
    bar = charts[0].chart
    assert list(bar.plots[0].categories) == ["2026", "2027", "2028"]
    assert list(bar.plots[0].series[0].values) == [160, 240, 310]
    assert bar.part.chart_workbook.xlsx_part is not None    # the data opens in PowerPoint's sheet
    tables = [sh for s in prs.slides for sh in s.shapes if sh.has_table]
    assert tables and tables[0].table.cell(1, 0).text == "Four à sole"


def test_tinted_photos_say_modifiee_in_their_cc_credit(workdir):
    notes = lambda prs, i: prs.slides[i].notes_slide.notes_text_frame.text  # noqa: E731
    grey = build(workdir, ALL_LAYOUTS, style="suisse")          # photos in black and white
    assert f"{CREDIT} (modifiée)" in notes(grey, 0)
    duo = build(workdir, ALL_LAYOUTS, style="terminal")         # duotone
    assert "(modifiée)" in notes(duo, 10)
    plain = build(workdir, ALL_LAYOUTS, style="editorial")      # photos as they are
    assert CREDIT in notes(plain, 0) and "modifiée" not in notes(plain, 0)
    assert deck.modified("Anonyme · CC0 · Wikimedia Commons") == "Anonyme · CC0 · Wikimedia Commons"
    assert deck.modified("X · CC BY 4.0") == "X · CC BY 4.0 (modifiée)"
    assert deck.modified("X · CC BY-SA 3.0 (modifiée)") == "X · CC BY-SA 3.0 (modifiée)"


def test_brand_colours_logo_and_style_override(workdir):
    spec = dict(ALL_LAYOUTS, style="pop", brand={"colors": ["#0A7D5A", "#F2C14E"], "logo": "logo.png"})
    prs = build(workdir, spec)
    xml = "".join(sh._element.xml for s in prs.slides for sh in s.shapes)
    assert "0A7D5A" in xml
    assert any(sh.shape_type == 13 for sh in prs.slides[0].shapes)   # the logo (a picture) on the cover
    out = subprocess.run([sys.executable, str(SLIDES / "deck.py"), str(workdir / "spec.json"), str(workdir / "o.pptx"),
                          "--style", "maison"], capture_output=True, text=True, check=True)
    assert json.loads(out.stdout)["style"] == "maison"


def test_the_agents_usual_style_comes_from_defaults_json(skill_copy, workdir):
    scripts = skill_copy / "slides-styles" / "scripts"
    spec = {"slides": [{"layout": "cover", "title": "Sans style"}]}
    (workdir / "s.json").write_text(json.dumps(spec), encoding="utf-8")
    run = lambda: json.loads(subprocess.run(  # noqa: E731
        [sys.executable, str(scripts / "deck.py"), str(workdir / "s.json"), str(workdir / "s.pptx")],
        capture_output=True, text=True, check=True, env=dict(os.environ, PATH="/usr/bin:/bin")).stdout)
    assert run()["style"] == styles.DEFAULT
    (skill_copy / "slides-styles" / "defaults.json").write_text('{"slides": "terminal"}\n', encoding="utf-8")
    answer = run()
    assert answer["style"] == "terminal"
    assert "look" not in answer   # no LibreOffice there: no preview step for the agent
    spec["style"] = "Carnet"
    (workdir / "s.json").write_text(json.dumps(spec), encoding="utf-8")
    assert run()["style"] == "carnet"


def test_photos_are_asked_with_the_applications_user_agent():
    import photo
    assert photo.UA.startswith("BipAgents/") and "(https://bipagents.com)" in photo.UA
    assert "Python-urllib/" in photo.UA


def test_libraries_are_not_installed_when_present(monkeypatch):
    import _deps
    calls = []
    monkeypatch.setattr(_deps.subprocess, "run", lambda *a, **k: calls.append(a))
    _deps.need("pptx", "PIL")
    assert calls == []
    pins = (SLIDES.parents[3].parent / "sandbox" / "requirements.txt").read_text(encoding="utf-8")
    for pin in _deps.PINS.values():
        assert pin in pins, pin   # the image and the fallback install the same versions


def test_the_overflow_check_catches_a_box_too_small():
    from pptx.util import Inches, Pt
    prs = Presentation()
    slide = prs.slides.add_slide(prs.slide_layouts[6])
    box = slide.shapes.add_textbox(Inches(1), Inches(1), Inches(2), Inches(0.4))
    run = box.text_frame.paragraphs[0].add_run()
    run.text = "Un titre beaucoup trop long pour une si petite boîte de deux pouces"
    run.font.size, run.font.name = Pt(24), "Georgia"
    assert overflows(prs)
