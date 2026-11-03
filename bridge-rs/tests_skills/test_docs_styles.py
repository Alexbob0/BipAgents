"""docs-styles: every kind in every style builds on real Word styles, quotes add up to the cent (HT, TVA, TTC), the
agent's usual style and the person's look are followed."""
import json
import os
import subprocess
import sys
from decimal import Decimal

import pytest
from docx import Document

import doc  # noqa: E402

FONTS = {"Arial", "Georgia", "Verdana", "Trebuchet MS", "Times New Roman", "Courier New"}
WORD_STYLES = {"Title", "Subtitle", "Heading 1", "Heading 2", "Heading 3", "Normal", "List Bullet", "List Number",
               "Caption", "Quote"}

REPORT = {
    "kind": "rapport", "title": "Bilan 2026", "subtitle": "Le Fournil de Léa",
    "from": {"name": "Le Fournil de Léa", "lines": ["12 rue des Boulangers", "44000 Nantes"]},
    "meta": [["Date", "2 novembre 2026"]],
    "blocks": [
        {"toc": True},
        {"heading": "Résumé", "level": 1}, {"text": "Une année de lancement réussie.\n\nLe chiffre suit le plan."},
        {"heading": "Ventes", "level": 2}, {"bullets": ["Pains : 55 %", "Viennoiseries : 30 %"]},
        {"numbers": ["Ouvrir le dimanche", "Former un apprenti"]},
        {"heading": "Détail", "level": 3},
        {"table": [["Mois", "CA"], ["Janvier", "12 000 €"], ["Total", "160 000 €"]]},
        {"quote": "Le bon pain prend du temps.", "author": "Léa"}, {"callout": "À valider en conseil."},
        {"kv": [["Responsable", "Léa Martin"]]}, {"actions": [["Action", "Qui", "Quand"], ["Devis four", "Karim", "Mars"]]},
        {"image": "photo.jpg", "caption": "Le fournil", "width": 0.5},
        {"entry": {"title": "Boulangère", "org": "Le Fournil", "dates": "2014-2026", "text": "Levain."}},
        {"page_break": True}, {"signature": ["Fait à Nantes", "Léa Martin"]},
    ],
    "footer": "SIRET à compléter",
}
KINDS = ["devis", "facture", "lettre", "cv", "compte_rendu", "rapport", "newsletter", "fiche", "autre"]


def build(workdir, spec, name="doc.docx", style=None):
    path = workdir / "doc.json"
    path.write_text(json.dumps(spec, ensure_ascii=False), encoding="utf-8")
    out = workdir / name
    doc.main([str(path), str(out)] + (["--style", style] if style else []))
    return Document(str(out))


def all_text(d):
    parts = [p.text for p in d.paragraphs]
    for t in d.tables:
        for row in t.rows:
            for cell in row.cells:
                parts.append(cell.text)
                for inner in cell.tables:
                    parts.extend(c.text for r in inner.rows for c in r.cells)
    return "\n".join(parts)


@pytest.mark.parametrize("style", sorted(doc.STYLES))
def test_every_kind_builds_on_real_word_styles_in_every_style(workdir, style):
    assert len(doc.STYLES) == 8
    for kind in KINDS:
        d = build(workdir, dict(REPORT, kind=kind, cv={"name": "Léa Martin", "headline": "Boulangère",
                                                      "contact": ["lea@exemple.fr"],
                                                      "sidebar": [{"heading": "Langues", "items": ["Anglais"]}]}),
                  style=style)
        names = {s.name for s in d.styles}
        assert WORD_STYLES <= names, (kind, WORD_STYLES - names)
        used = {p.style.name for p in d.paragraphs}
        if kind != "cv":
            assert {"Heading 1", "List Bullet"} <= used, (kind, used)
        # the styles the document uses (python-docx's template also lists unused ones, e.g. « Macro Text » in Courier)
        fonts = {d.styles[n].font.name for n in WORD_STYLES if d.styles[n].font.name}
        assert fonts <= FONTS, fonts - FONTS


def test_quote_totals_are_right_to_the_cent():
    rows = [["Désignation", "Qté", "Prix unitaire HT"], ["Pain de campagne", 12, 6.5], ["Mini-viennoiseries", 150, 0.9],
            ["Pièce montée", 1, 210.0], ["Livraison", 1, 45.0]]
    t = doc.totals(rows, 5.5)
    assert t["lines"] == [Decimal("78.00"), Decimal("135.00"), Decimal("210.00"), Decimal("45.00")]
    assert (t["ht"], t["tva"], t["ttc"]) == (Decimal("468.00"), Decimal("25.74"), Decimal("493.74"))
    # half a cent rounds up, floats never leak (0.1 + 0.2)
    t = doc.totals([["D", "Q", "PU"], ["a", 3, 1.15], ["b", 1, 0.1], ["c", 1, 0.2]], "20")
    assert (t["ht"], t["tva"], t["ttc"]) == (Decimal("3.75"), Decimal("0.75"), Decimal("4.50"))
    t = doc.totals([["D", "Q", "PU"], ["x", 1, "81,45 €"]], "5,5")
    assert (t["tva"], t["ttc"]) == (Decimal("4.48"), Decimal("85.93"))           # 4.47975 -> 4.48
    t = doc.totals([["D", "Q", "PU"], ["x", "2", "1 200,50"]], None)
    assert (t["ht"], t["rate"], t["tva"], t["ttc"]) == (Decimal("2401.00"), None, 0, Decimal("2401.00"))
    t = doc.totals([["Désignation", "Montant"], ["Forfait", "350"], ["Option", 49.9]], 20)
    assert (t["ht"], t["tva"], t["ttc"]) == (Decimal("399.90"), Decimal("79.98"), Decimal("479.88"))


def test_the_quote_shows_its_totals(workdir):
    spec = json.loads((workdir / "5-devis-boulangerie.json").read_text(encoding="utf-8"))
    text = all_text(build(workdir, spec, style="artisan")).replace(" ", " ").replace(" ", " ")
    assert "Total HT" in text and "468,00 €" in text
    assert "TVA 5,5 %" in text and "25,74 €" in text
    assert "Total TTC" in text and "493,74 €" in text


@pytest.mark.parametrize("fixture", ["5-devis-boulangerie.json", "6-compte-rendu.json", "7-cv.json"])
def test_glm_documents_build_in_every_style(workdir, fixture):
    spec = json.loads((workdir / fixture).read_text(encoding="utf-8"))
    for style in doc.STYLES:
        d = build(workdir, spec, style=style)
        assert all_text(d).strip()


def test_brand_colour_logo_and_template(workdir):
    d = build(workdir, dict(REPORT, brand={"colors": ["#0A7D5A"], "logo": "logo.png"}), "brand.docx")
    assert "0A7D5A" in d.styles["Heading 1"].element.xml or "0A7D5A" in d.element.xml
    assert d.inline_shapes or any("graphic" in p._p.xml for p in d.paragraphs)
    # the person's own Word file as a template: its styles kept
    tpl = Document()
    tpl.styles["Normal"].font.name = "Calibri"
    tpl.save(str(workdir / "modele.docx"))
    d = build(workdir, dict(REPORT, template="modele.docx"), "tpl.docx")
    assert d.styles["Normal"].font.name == "Calibri"
    assert "Bilan 2026" in all_text(d)


def test_the_agents_usual_style_comes_from_defaults_json(skill_copy, workdir):
    scripts = skill_copy / "docs-styles" / "scripts"
    (workdir / "d.json").write_text(json.dumps({"kind": "lettre", "title": "Lettre"}), encoding="utf-8")
    run = lambda: json.loads(subprocess.run(  # noqa: E731
        [sys.executable, str(scripts / "doc.py"), str(workdir / "d.json"), str(workdir / "d.docx")],
        capture_output=True, text=True, check=True, env=dict(os.environ, PATH="/usr/bin:/bin")).stdout)
    assert run()["style"] == doc.DEFAULT
    (skill_copy / "docs-styles" / "defaults.json").write_text('{"docs": "ecole"}\n', encoding="utf-8")
    assert run()["style"] == "ecole"
    (skill_copy / "docs-styles" / "defaults.json").write_text('{"docs": "bauhaus"}\n', encoding="utf-8")
    assert run()["style"] == doc.DEFAULT   # no such Word style: the scripts' own default
