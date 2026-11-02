#!/usr/bin/env python3
"""Content-only JSON -> designed, editable Word document (.docx) built on real Word styles
(Title, Subtitle, Heading 1-3, Normal, List Bullet/Number, Caption, Quote, TOC), so it stays editable.

{"style": "artisan",                  # see STYLES below; omit = rapport
 "kind": "devis",                     # devis | facture | lettre | cv | compte_rendu | rapport | newsletter | fiche | autre
 "title": "Devis", "subtitle": "...",
 "from": {"name": "Le Fournil de Léa", "lines": ["12 rue ...", "44000 Nantes"]},
 "to":   {"name": "...", "lines": ["..."]},
 "meta": [["Devis n°", "2026-014"], ["Date", "2 novembre 2026"]],
 "blocks": [ ... ],
 "footer": "SIRET ... · TVA ...",
 "brand": {"colors": ["#0A7D5A"], "logo": "logo.png"},     # optional: the person's colours / logo
 "template": "modele.docx"}                                 # optional: the person's own Word file (its styles are kept)

BLOCKS (one key each)
 {"heading": "...", "level": 1|2|3}      {"text": "..."}  (blank line = new paragraph)
 {"bullets": [...]}  {"numbers": [...]}  {"quote": "...", "author": "..."}  {"callout": "..."}
 {"table": [[header...], [...]]}         (a first cell « Total… » row is bold)
 {"items": [["Désignation", "Qté", "Prix unitaire"], ["Pain", 10, 4.2]], "vat": 5.5}   -> totals HT / TVA / TTC
 {"kv": [["Date", "..."], ...]}          {"actions": [["Action", "Qui", "Quand"], ...]}
 {"image": "photo.jpg", "caption": "...", "width": 0.6}   {"entry": {"title", "org", "dates", "text" | "bullets"}}
 {"toc": true}  {"page_break": true}  {"signature": ["Fait à Nantes, le ...", "Léa Martin"]}
CV: "cv": {"name", "headline", "photo", "contact": [...], "sidebar": [{"heading", "items": [...]}]} + blocks.
Relative image paths are read from the spec's folder.
"""
import argparse
import copy
import json
import os
import re
import shutil
import sys
from decimal import ROUND_HALF_UP, Decimal

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import _deps  # noqa: E402

_deps.need("docx")

from docx import Document
from docx.enum.section import WD_SECTION
from docx.enum.style import WD_STYLE_TYPE
from docx.enum.table import WD_TABLE_ALIGNMENT, WD_CELL_VERTICAL_ALIGNMENT
from docx.enum.text import WD_ALIGN_PARAGRAPH, WD_BREAK
from docx.oxml import OxmlElement
from docx.oxml.ns import qn
from docx.shared import Pt, Mm, RGBColor, Emu

AL = {"l": WD_ALIGN_PARAGRAPH.LEFT, "c": WD_ALIGN_PARAGRAPH.CENTER, "r": WD_ALIGN_PARAGRAPH.RIGHT,
      "j": WD_ALIGN_PARAGRAPH.JUSTIFY}

# h1/h2: the heading treatment; band = shaded paragraph, rule_top / rule_bottom / double / box = borders
STYLES = {
    "editorial": dict(label="Éditorial", hint="Georgia, filets fins, rouge discret — lettre soignée, article, récit, gastronomie",
                      head="Georgia", body="Georgia", lab="Arial", size=10.5, accent="B3261E", accent2="1F3A5F",
                      text="141414", muted="6E685E", alt="F5F1E8", title_size=30, h1=("rule_top", 18, False),
                      h2=("label", 9, True), table="rules", title_align="l", margins=(25, 25, 22, 22)),
    "suisse": dict(label="Suisse", hint="Arial, grille nette, noir + rouge — CV, rapport sobre, fiche technique",
                   head="Arial", body="Arial", lab="Arial", size=10, accent="E30613", accent2="000000",
                   text="000000", muted="6F6F6F", alt="F2F2F2", title_size=30, h1=("rule_top_thick", 15, True),
                   h2=("plain", 11, True), table="rules", title_align="l", margins=(30, 18, 20, 20)),
    "terminal": dict(label="Technique", hint="Arial + Courier New, blocs de code — documentation, spécification, compte rendu tech",
                     head="Arial", body="Arial", lab="Courier New", size=10, accent="1F883D", accent2="0969DA",
                     text="1F2328", muted="59636E", alt="F6F8FA", dark="0E1116", title_size=26,
                     h1=("mono_prefix", 15, True), h2=("mono", 11, True), table="mono", title_align="l",
                     margins=(22, 22, 22, 22)),
    "pop": dict(label="Pop", hint="Trebuchet, aplats jaune/bleu, encadrés noirs — newsletter, affiche, invitation, asso",
                head="Trebuchet MS", body="Trebuchet MS", lab="Trebuchet MS", size=10.5, accent="FF5A36",
                accent2="3D5AFE", accent3="FFC700", text="111111", muted="4A4A4A", alt="FFF4E0", title_size=32,
                h1=("box", 16, True), h2=("plain_accent", 12, True), table="brutal", title_align="l",
                margins=(20, 20, 20, 20)),
    "maison": dict(label="Maison", hint="capitales espacées, filets bronze, centré — devis haut de gamme, menu, invitation, hôtel",
                   head="Times New Roman", body="Georgia", lab="Arial", size=10.5, accent="8A6A3D", accent2="3E4A3D",
                   text="2B2420", muted="8C7B6B", alt="F3EEE6", title_size=26, title_upper=True,
                   h1=("caps_center", 13, False), h2=("label", 8.5, True), table="hairline", title_align="c",
                   margins=(28, 28, 25, 25)),
    "rapport": dict(label="Rapport", hint="Arial, bleu marine, tableaux nets, sommaire — rapport, bilan, compte rendu, budget",
                    head="Arial", body="Arial", lab="Arial", size=10, accent="0B3D91", accent2="00A3A1",
                    text="1A1A1A", muted="6B7280", alt="F1F4F8", title_size=28, h1=("rule_bottom", 16, True),
                    h2=("plain_accent2", 12, True), table="header", title_align="l", margins=(22, 22, 20, 20)),
    "ecole": dict(label="École", hint="Verdana, gros titres en bandeau de couleur — fiche de cours, mot aux parents, asso",
                  head="Verdana", body="Verdana", lab="Verdana", size=10.5, accent="1565C0", accent2="E53935",
                  accent3="FFD54F", text="1B2A3A", muted="5B6B7B", alt="EAF2FB", title_size=26,
                  h1=("band", 14, True), h2=("highlight", 12, True), table="zebra", title_align="l",
                  margins=(20, 20, 20, 20)),
    "artisan": dict(label="Artisan", hint="Georgia + Trebuchet, doubles filets vert et ocre — devis d'artisan, fiche produit, commerce",
                    head="Georgia", body="Trebuchet MS", lab="Trebuchet MS", size=10.5, accent="2F5D50",
                    accent2="C8752B", text="2F2A24", muted="74685A", alt="F4ECDC", title_size=28, title_upper=True,
                    h1=("double", 13, True), h2=("plain_accent2", 11, True), table="double", title_align="c",
                    margins=(22, 22, 22, 22)),
}
DEFAULT = "rapport"
NB = " "


def fr(t):
    t = str(t if t is not None else "")
    t = re.sub(r"« ", "«" + NB, t)
    t = re.sub(r" (»|:|;|!|\?|%|€)", NB + r"\1", t)
    return t


def num(v):
    if isinstance(v, (int, float)):
        return float(v)
    s = str(v).replace("€", "").replace(NB, "").replace(" ", "").replace(" ", "").replace(",", ".")
    s = re.sub(r"[^0-9.\-]", "", s)
    try:
        return float(s)
    except ValueError:
        return None


def money(v, cur="€"):
    s = f"{v:,.2f}".replace(",", " ").replace(".", ",").replace(" ", NB)
    return f"{s}{NB}{cur}"


CENT = Decimal("0.01")


def dec(v):
    """A number as a Decimal (12, "12,5", "1 200 €"), None when there is none."""
    n = num(v)
    return None if n is None else Decimal(repr(n))


def cents(v):
    return v.quantize(CENT, rounding=ROUND_HALF_UP)


def fr_number(v):
    """5.5 -> « 5,5 », 20 -> « 20 »."""
    s = format(v.normalize(), "f") if isinstance(v, Decimal) else str(v)
    return s.replace(".", ",")


def totals(rows, vat=None):
    """A quote's or an invoice's sums, French practice: each line rounded to the cent (quantity × unit price, or its
    last column when it has no quantity), Total HT their sum, TVA = HT × rate rounded to the cent, TTC = HT + TVA.
    {"lines": [Decimal…], "ht", "rate" (None without VAT), "tva", "ttc"}."""
    head = rows[0] if rows else []
    lines = []
    for r in rows[1:]:
        r = list(r)
        if len(head) == 3 and len(r) >= 3:
            q, u = dec(r[1]), dec(r[2])
            lines.append(cents((q or Decimal(0)) * (u or Decimal(0))))
        else:
            lines.append(cents(dec(r[-1]) or Decimal(0)) if r else Decimal(0))
    ht = sum(lines, Decimal(0))
    rate = dec(str(vat).replace(",", ".")) if vat not in (None, "", 0, "0") else None
    tva = cents(ht * rate / 100) if rate is not None else Decimal(0)
    return {"lines": lines, "ht": ht, "rate": rate, "tva": tva, "ttc": ht + tva}


def mix(a, b, t):
    return "".join(f"{round(int(a[i:i + 2], 16) * (1 - t) + int(b[i:i + 2], 16) * t):02X}" for i in (0, 2, 4))


def lum(h):
    c = [int(h[i:i + 2], 16) / 255 for i in (0, 2, 4)]
    c = [v / 12.92 if v <= 0.03928 else ((v + 0.055) / 1.055) ** 2.4 for v in c]
    return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]


# -- low-level XML helpers ----------------------------------------------------------------------------------------

def set_font(target, name, size=None, bold=None, color=None, italic=None, caps=None, spacing=None):
    """target: a style or a run. Explicit fonts (theme fonts removed), so every app shows the same font."""
    f = target.font
    if name:
        f.name = name
        rpr = target.element.get_or_add_rPr() if hasattr(target.element, "get_or_add_rPr") else target._r.get_or_add_rPr()
        rf = rpr.find(qn("w:rFonts"))
        if rf is None:
            rf = OxmlElement("w:rFonts")
            rpr.insert(0, rf)
        for k in ("w:asciiTheme", "w:hAnsiTheme", "w:eastAsiaTheme", "w:cstheme"):
            rf.attrib.pop(qn(k), None)
        for k in ("w:ascii", "w:hAnsi", "w:cs", "w:eastAsia"):
            rf.set(qn(k), name)
    if size:
        f.size = Pt(size)
    if bold is not None:
        f.bold = bold
    if italic is not None:
        f.italic = italic
    if caps is not None:
        f.all_caps = caps
    if color:
        f.color.rgb = RGBColor.from_string(color)
        rpr = target.element.rPr if hasattr(target.element, "rPr") else target._r.rPr
        c = rpr.find(qn("w:color")) if rpr is not None else None
        if c is not None:
            for k in ("w:themeColor", "w:themeShade", "w:themeTint"):
                c.attrib.pop(qn(k), None)
    if spacing is not None:
        rpr = target.element.get_or_add_rPr() if hasattr(target.element, "get_or_add_rPr") else target._r.get_or_add_rPr()
        sp = rpr.find(qn("w:spacing"))
        if sp is None:
            sp = OxmlElement("w:spacing")
            rpr.append(sp)
        sp.set(qn("w:val"), str(int(spacing * 20)))


def pborder(ppr_owner, **sides):
    """sides: top=(val, size_eighths, color, space). ppr_owner: a paragraph or a paragraph style."""
    ppr = ppr_owner.element.get_or_add_pPr() if hasattr(ppr_owner, "element") else ppr_owner._p.get_or_add_pPr()
    b = ppr.find(qn("w:pBdr"))
    if b is None:
        b = OxmlElement("w:pBdr")
        # pBdr goes before shd/tabs/spacing/ind/jc in the schema order
        anchor = None
        for tag in ("w:shd", "w:tabs", "w:suppressAutoHyphens", "w:spacing", "w:ind", "w:jc", "w:rPr"):
            anchor = ppr.find(qn(tag))
            if anchor is not None:
                break
        if anchor is not None:
            anchor.addprevious(b)
        else:
            ppr.append(b)
    for side in ("top", "left", "bottom", "right"):
        if side in sides:
            val, sz, color, space = sides[side]
            el = OxmlElement(f"w:{side}")
            el.set(qn("w:val"), val)
            el.set(qn("w:sz"), str(sz))
            el.set(qn("w:space"), str(space))
            el.set(qn("w:color"), color)
            b.append(el)


def pshade(owner, fill):
    ppr = owner.element.get_or_add_pPr() if hasattr(owner, "element") else owner._p.get_or_add_pPr()
    shd = OxmlElement("w:shd")
    shd.set(qn("w:val"), "clear")
    shd.set(qn("w:color"), "auto")
    shd.set(qn("w:fill"), fill)
    anchor = None
    for tag in ("w:tabs", "w:suppressAutoHyphens", "w:spacing", "w:ind", "w:jc", "w:rPr"):
        anchor = ppr.find(qn(tag))
        if anchor is not None:
            break
    if anchor is not None:
        anchor.addprevious(shd)
    else:
        ppr.append(shd)


def cell_shade(cell, fill):
    tcPr = cell._tc.get_or_add_tcPr()
    shd = OxmlElement("w:shd")
    shd.set(qn("w:val"), "clear")
    shd.set(qn("w:color"), "auto")
    shd.set(qn("w:fill"), fill)
    tcPr.append(shd)


def cell_borders(cell, **sides):
    tcPr = cell._tc.get_or_add_tcPr()
    b = tcPr.find(qn("w:tcBorders"))
    if b is None:
        b = OxmlElement("w:tcBorders")
        tcPr.append(b)
    for side in ("top", "left", "bottom", "right"):
        val, sz, color = sides.get(side, ("nil", 0, "auto"))
        el = OxmlElement(f"w:{side}")
        el.set(qn("w:val"), val)
        el.set(qn("w:sz"), str(sz))
        el.set(qn("w:color"), color)
        b.append(el)


def cell_margins(table, top=60, bottom=60, left=110, right=110):
    tblPr = table._tbl.tblPr
    m = OxmlElement("w:tblCellMar")
    for side, v in (("top", top), ("left", left), ("bottom", bottom), ("right", right)):
        el = OxmlElement(f"w:{side}")
        el.set(qn("w:w"), str(v))
        el.set(qn("w:type"), "dxa")
        m.append(el)
    tblPr.append(m)


def no_borders(table):
    tblPr = table._tbl.tblPr
    b = OxmlElement("w:tblBorders")
    for side in ("top", "left", "bottom", "right", "insideH", "insideV"):
        el = OxmlElement(f"w:{side}")
        el.set(qn("w:val"), "nil")
        b.append(el)
    tblPr.append(b)


def set_widths(table, total_emu, fracs):
    """Fixed column widths that Word, LibreOffice, Pages and Google Docs all respect."""
    table.autofit = False
    tw = int(total_emu / 635)                       # EMU -> twips (dxa)
    tblPr = table._tbl.tblPr
    w = tblPr.find(qn("w:tblW"))
    if w is None:
        w = OxmlElement("w:tblW")
        tblPr.append(w)
    w.set(qn("w:w"), str(tw))
    w.set(qn("w:type"), "dxa")
    lay = OxmlElement("w:tblLayout")
    lay.set(qn("w:type"), "fixed")
    tblPr.append(lay)
    grid = table._tbl.tblGrid
    cols = grid.findall(qn("w:gridCol"))
    for gc, f in zip(cols, fracs):
        gc.set(qn("w:w"), str(int(tw * f)))
    for row in table.rows:
        for cell, f in zip(row.cells, fracs):
            cell.width = Emu(int(total_emu * f))


def field(paragraph, instr, placeholder="1"):
    fld = OxmlElement("w:fldSimple")
    fld.set(qn("w:instr"), instr)
    r = OxmlElement("w:r")
    t = OxmlElement("w:t")
    t.text = placeholder
    r.append(t)
    fld.append(r)
    paragraph._p.append(fld)
    return fld


def agent_default():
    """The style this agent uses when the subject calls for none in particular: `defaults.json` next to SKILL.md,
    written by the bridge for the agent's kind (style_skills.rs); else the scripts' own default."""
    try:
        with open(os.path.join(HERE, "..", "defaults.json"), encoding="utf-8") as f:
            name = json.load(f).get("docs")
    except (OSError, ValueError, AttributeError):
        name = None
    return name if name in STYLES else DEFAULT


class Doc:
    def __init__(self, spec, base):
        self.spec, self.base = spec, base
        name = str(spec.get("style") or "").strip().lower()
        self.style_name = name if name in STYLES else agent_default()
        self.st = st = copy.deepcopy(STYLES[self.style_name])
        st.setdefault("accent3", st["accent2"])
        st.setdefault("dark", st["text"])
        self.templated = bool(spec.get("template"))
        self.d = Document(self.path(spec["template"])) if self.templated else Document()
        cols = [c.lstrip("#") for c in (spec.get("brand") or {}).get("colors") or [] if len(c.lstrip("#")) == 6]
        if cols:
            st["accent"] = cols[0]
            st["accent2"] = cols[1] if len(cols) > 1 else st["accent2"]
            st["accent3"] = cols[2] if len(cols) > 2 else st["accent3"]
        logo = (spec.get("brand") or {}).get("logo")
        self.logo = self.path(logo) if logo else None
        if self.templated:
            self.clear_body()
            self.from_template_theme()
        else:
            self.setup_page()
            self.setup_styles()
        self.headings = [b.get("heading") for b in spec.get("blocks", []) if b.get("heading") and b.get("level", 1) <= 2]

    def path(self, p):
        """Relative paths: the spec's folder, then the current folder, then home."""
        if not p or os.path.isabs(p):
            return p
        for root in (self.base, os.getcwd(), os.path.expanduser("~"), "/root", "/workspace"):
            if os.path.exists(os.path.join(root, p)):
                return os.path.join(root, p)
        return os.path.join(self.base, p)

    # -- the person's own Word file -----------------------------------------------------------------------------
    def clear_body(self):
        body = self.d.element.body
        for el in list(body):
            if el.tag != qn("w:sectPr"):
                body.remove(el)

    def from_template_theme(self):
        """Their styles stay as they are; their Heading 1 colour (or theme accent) colours our tables."""
        try:
            h = self.d.styles["Heading 1"]
            if h.font.color is not None and h.font.color.rgb is not None:
                self.st["accent"] = str(h.font.color.rgb)
            if h.font.name:
                self.st["head"] = h.font.name
            n = self.d.styles["Normal"]
            if n.font.name:
                self.st["body"] = n.font.name
        except Exception:
            pass

    # -- page and styles -------------------------------------------------------------------------------------------
    def setup_page(self):
        st = self.st
        for sec in self.d.sections:
            sec.page_width, sec.page_height = Mm(210), Mm(297)
            l, r, t, b = st["margins"]
            sec.left_margin, sec.right_margin, sec.top_margin, sec.bottom_margin = Mm(l), Mm(r), Mm(t), Mm(b)
            sec.header_distance = sec.footer_distance = Mm(10)

    def style(self, name, kind=WD_STYLE_TYPE.PARAGRAPH, base="Normal"):
        try:
            return self.d.styles[name]
        except KeyError:
            s = self.d.styles.add_style(name, kind)
            if kind == WD_STYLE_TYPE.PARAGRAPH:
                s.base_style = self.d.styles[base]
            return s

    def setup_styles(self):
        st = self.st
        n = self.d.styles["Normal"]
        set_font(n, st["body"], st["size"], color=st["text"])
        n.paragraph_format.space_after = Pt(st["size"] * 0.55)
        n.paragraph_format.line_spacing = 1.2
        # document defaults too, so new text typed in Word gets the same font
        rpr_default = self.d.styles.element.find(qn("w:docDefaults"))
        if rpr_default is not None:
            rf = rpr_default.find(".//" + qn("w:rFonts"))
            if rf is not None:
                for k in list(rf.attrib):
                    del rf.attrib[k]
                for k in ("w:ascii", "w:hAnsi", "w:cs", "w:eastAsia"):
                    rf.set(qn(k), st["body"])
        t = self.style("Title")
        set_font(t, st["head"], st["title_size"], bold=st["h1"][2] and st["head"] != "Times New Roman",
                 color=st["text"], caps=st.get("title_upper", False), spacing=3 if st.get("title_upper") else 0)
        t.paragraph_format.space_after = Pt(4)
        t.paragraph_format.alignment = AL[st["title_align"]]
        tp = t.element.get_or_add_pPr()
        old = tp.find(qn("w:pBdr"))
        if old is not None:
            tp.remove(old)
        s = self.style("Subtitle")
        set_font(s, st["body"], st["size"] + 3, color=st["muted"], italic=st["body"] in ("Georgia",), bold=False,
                 spacing=0)
        s.paragraph_format.alignment = AL[st["title_align"]]
        s.paragraph_format.space_after = Pt(14)
        for lvl, (treat, size, bold) in ((1, st["h1"]), (2, st["h2"])):
            h = self.style(f"Heading {lvl}")
            self.heading_style(h, treat, size, bold, lvl)
        h3 = self.style("Heading 3")
        set_font(h3, st["head"], st["size"] + 1, bold=True, color=st["text"], italic=False)
        h3.paragraph_format.space_before = Pt(8)
        h3.paragraph_format.space_after = Pt(2)
        cap = self.style("Caption")
        set_font(cap, st["body"], st["size"] - 1.5, color=st["muted"], italic=True, bold=False)
        q = self.style("Quote")
        set_font(q, st["head"] if st["head"] != "Arial" else st["body"], st["size"] + 3, color=st["accent"],
                 italic=True)
        q.paragraph_format.left_indent = Mm(8)
        q.paragraph_format.space_before = Pt(8)
        q.paragraph_format.space_after = Pt(8)
        pborder(q, left=("single", 18, st["accent"], 12))
        for name in ("List Bullet", "List Number"):
            ls = self.style(name)
            set_font(ls, st["body"], st["size"], color=st["text"])
            ls.paragraph_format.space_after = Pt(3)
        th = self.style("TOC Heading", base="Heading 1")
        for lvl in (1, 2):
            ts = self.style(f"TOC {lvl}")
            set_font(ts, st["body"], st["size"] + (1 if lvl == 1 else 0), bold=lvl == 1, color=st["text"])
            ts.paragraph_format.left_indent = Mm(0 if lvl == 1 else 6)
            ts.paragraph_format.space_after = Pt(4)
        lab = self.style("Label", WD_STYLE_TYPE.CHARACTER)
        set_font(lab, st["lab"], st["size"] - 2, bold=True, color=st["accent"], caps=st["lab"] != "Courier New",
                 spacing=1.2 if st["lab"] != "Courier New" else 0)
        code = self.style("Code")
        set_font(code, "Courier New", st["size"] - 1, color="E6EDF3")
        pshade(code, st.get("dark", "1E1E1E"))
        code.paragraph_format.space_after = Pt(0)
        code.paragraph_format.line_spacing = 1.1
        code.paragraph_format.left_indent = Mm(3)

    def heading_style(self, h, treat, size, bold, lvl):
        st = self.st
        pf = h.paragraph_format
        pf.space_before = Pt(18 if lvl == 1 else 10)
        pf.space_after = Pt(6 if lvl == 1 else 3)
        pf.keep_with_next = True
        color = st["text"]
        font = st["head"]
        caps, spacing, italic = False, 0, False
        if treat in ("label",):
            font, color, caps, spacing = st["lab"], st["accent"], True, 1.5
        elif treat in ("plain_accent",):
            color = st["accent2"]
        elif treat in ("plain_accent2",):
            color = st["accent2"] if self.style_name != "rapport" else st["accent2"]
        elif treat in ("mono", "mono_prefix"):
            font, color = ("Courier New", st["accent"]) if treat == "mono" else (st["head"], st["text"])
        elif treat == "caps_center":
            caps, spacing = True, 3
            color = st["accent"]
            pf.alignment = WD_ALIGN_PARAGRAPH.CENTER
        elif treat == "double":
            caps, spacing, color = True, 1.5, st["accent"]
        elif treat == "band":
            color = "FFFFFF"
        elif treat == "rule_bottom":
            color = st["accent"]
        set_font(h, font, size, bold=bold, color=color, italic=italic, caps=caps, spacing=spacing)
        if treat == "rule_top":
            pborder(h, top=("single", 6, st["text"], 8))
        elif treat == "rule_top_thick":
            pborder(h, top=("single", 24, st["text"], 6))
        elif treat == "rule_bottom":
            pborder(h, bottom=("single", 8, st["accent"], 3))
        elif treat == "double":
            pborder(h, bottom=("double", 6, st["text"], 3))
        elif treat == "caps_center":
            pborder(h, bottom=("single", 4, st["accent"], 4))
        elif treat == "band":
            pshade(h, st["accent"])
            pborder(h, top=("single", 4, st["accent"], 2), bottom=("single", 4, st["accent"], 2),
                    left=("single", 4, st["accent"], 4), right=("single", 4, st["accent"], 4))
        elif treat == "box":
            pshade(h, st["accent3"])
            for side in ("top", "left", "bottom", "right"):
                pborder(h, **{side: ("single", 18, "111111", 3)})
        elif treat == "highlight":
            pborder(h, left=("single", 36, st["accent3"], 6))
        elif treat == "mono_prefix":
            pborder(h, bottom=("single", 4, mix(st["text"], "FFFFFF", 0.75), 3))

    # -- writing ---------------------------------------------------------------------------------------------------
    def para(self, text="", style=None, align=None, after=None, before=None):
        # a style the person's own template lacks (TOC Heading, Quote…) is added rather than failing
        p = self.d.add_paragraph(style=self.style(style) if isinstance(style, str) else style)
        if text:
            self.runs(p, text)
        if align:
            p.alignment = AL[align]
        if after is not None:
            p.paragraph_format.space_after = Pt(after)
        if before is not None:
            p.paragraph_format.space_before = Pt(before)
        return p

    def runs(self, p, text):
        """**bold** inside text becomes bold runs."""
        parts = re.split(r"(\*\*[^*]+\*\*)", fr(text))
        for part in parts:
            if part.startswith("**") and part.endswith("**"):
                p.add_run(part[2:-2]).bold = True
            elif part:
                p.add_run(part)
        return p

    def heading(self, text, level=1):
        st = self.st
        level = max(1, min(3, int(level or 1)))
        if level == 1 and st["h1"][0] == "mono_prefix" and not self.templated:
            p = self.d.add_paragraph(style=self.style("Heading 1"))
            r = p.add_run("## ")
            set_font(r, "Courier New", color=st["accent"])
            p.add_run(fr(text))
            return p
        if level == 1 and st["h1"][0] == "double" and not self.templated:
            p = self.d.add_paragraph(style=self.style("Heading 1"))
            p.add_run(fr(text))
            return p
        return self.d.add_heading(fr(text), level)

    def label_run(self, p, text, color=None):
        r = p.add_run(fr(text))
        if not self.templated:
            r.style = self.d.styles["Label"]
        else:
            r.bold = True
        if color:
            r.font.color.rgb = RGBColor.from_string(color)
        return r

    # -- tables ----------------------------------------------------------------------------------------------------
    def width(self):
        sec = self.d.sections[-1]
        return sec.page_width - sec.left_margin - sec.right_margin

    def table(self, rows, widths=None, numeric_right=True, header=True, kind=None, bold_last=False):
        st = self.st
        kind = kind or st["table"]
        rows = [[fr(c) for c in r] for r in rows]
        nc = max(len(r) for r in rows)
        t = self.d.add_table(rows=len(rows), cols=nc)
        t.alignment = WD_TABLE_ALIGNMENT.CENTER
        no_borders(t)
        cell_margins(t, 70, 70, 110, 110)
        total = self.width()
        if not widths:
            lens = [max(len(r[c]) if c < len(r) else 0 for r in rows) + 6 for c in range(nc)]
            widths = [l / sum(lens) for l in lens]
        set_widths(t, total, widths)
        line = st["accent"] if kind in ("hairline",) else st["text"]
        for r, row in enumerate(rows):
            head = header and r == 0
            tot = (r > 0 and row and str(row[0]).lower().startswith(("total", "net à payer"))) or (bold_last and r == len(rows) - 1)
            for c in range(nc):
                cell = t.cell(r, c)
                cell.vertical_alignment = WD_CELL_VERTICAL_ALIGNMENT.CENTER
                txt = row[c] if c < len(row) else ""
                p = cell.paragraphs[0]
                p.paragraph_format.space_after = Pt(0)
                run = p.add_run(txt)
                isnum = numeric_right and c > 0 and bool(re.search(r"\d", txt)) and len(txt) < 22
                if isnum:
                    p.alignment = WD_ALIGN_PARAGRAPH.RIGHT
                fill, col, bold = None, None, head or tot
                if kind == "header" and head:
                    fill, col = st["accent"], "FFFFFF"
                elif kind == "zebra":
                    if head:
                        fill, col = st["accent"], "FFFFFF"
                    elif r % 2 == 0:
                        fill = st["alt"]
                elif kind == "brutal" and head:
                    fill = st["accent3"]
                elif kind == "double" and head:
                    fill, col = st["alt"], st["accent"]
                elif kind == "mono":
                    set_font(run, "Courier New", st["size"] - 1)
                    if head:
                        col = st["accent"]
                elif kind == "hairline" and head:
                    col = st["accent"]
                    run.font.all_caps = True
                    run.font.size = Pt(st["size"] - 2)
                if tot and kind in ("header", "zebra"):
                    fill = st["alt"]
                if fill:
                    cell_shade(cell, fill)
                if col:
                    run.font.color.rgb = RGBColor.from_string(col)
                run.bold = bold
                if kind == "brutal":
                    cell_borders(cell, top=("single", 12, "111111"), bottom=("single", 12, "111111"),
                                 left=("single", 12, "111111"), right=("single", 12, "111111"))
                elif kind == "double":
                    if head:
                        cell_borders(cell, top=("double", 6, st["text"]), bottom=("single", 6, st["text"]))
                    elif tot:
                        cell_borders(cell, top=("single", 6, st["text"]), bottom=("double", 6, st["text"]))
                    else:
                        cell_borders(cell, bottom=("single", 2, mix(st["text"], "FFFFFF", 0.6)))
                elif kind in ("rules", "hairline", "mono"):
                    if head:
                        cell_borders(cell, bottom=("single", 10 if kind == "rules" else 6, line))
                    elif tot:
                        cell_borders(cell, top=("single", 8, line))
                    else:
                        cell_borders(cell, bottom=("single", 2, mix(line, "FFFFFF", 0.6)))
                elif kind in ("header", "zebra"):
                    if not head:
                        cell_borders(cell, bottom=("single", 2, mix(st["text"], "FFFFFF", 0.75)))
        self.para(after=4)
        return t

    def items(self, rows, vat=None, cur="€"):
        """Quote/invoice lines: computes each line, then HT, TVA, TTC (to the cent, `totals`)."""
        head = [str(c) for c in rows[0]]
        out = [head + (["Total"] if len(head) == 3 else [])]
        calc = totals(rows, vat)
        for r, line in zip(rows[1:], calc["lines"]):
            r = list(r)
            if len(head) == 3 and len(r) >= 3:
                u = dec(r[2])
                out.append([str(r[0]), str(r[1]).replace(".", ","), money(u, cur) if u is not None else str(r[2]),
                            money(line, cur)])
            else:
                out.append([str(c) for c in r])
        self.table(out, widths=[0.52, 0.12, 0.18, 0.18] if len(out[0]) == 4 else None)
        tot = [["Total HT", money(calc["ht"], cur)]]
        if calc["rate"] is not None:
            tot += [[f"TVA {fr_number(calc['rate'])} %", money(calc["tva"], cur)], ["Total TTC", money(calc["ttc"], cur)]]
        # a small right-aligned totals block
        t = self.d.add_table(rows=len(tot), cols=2)
        t.alignment = WD_TABLE_ALIGNMENT.RIGHT
        no_borders(t)
        cell_margins(t, 50, 50, 110, 110)
        w = self.width()
        set_widths(t, int(w * 0.4), [0.55, 0.45])
        for i, (k, v) in enumerate(tot):
            last = i == len(tot) - 1
            for c, txt in enumerate((k, v)):
                cell = t.cell(i, c)
                cell.width = Emu(int(w * (0.22 if c == 0 else 0.18)))
                p = cell.paragraphs[0]
                p.paragraph_format.space_after = Pt(0)
                p.alignment = WD_ALIGN_PARAGRAPH.RIGHT
                r = p.add_run(fr(txt))
                r.bold = last
                if last:
                    r.font.size = Pt(self.st["size"] + 2)
                    if self.st["table"] in ("header", "zebra", "brutal", "double"):
                        cell_shade(cell, self.st["accent"] if self.st["table"] != "brutal" else self.st["accent3"])
                        if self.st["table"] != "brutal":
                            r.font.color.rgb = RGBColor.from_string("FFFFFF")
                    else:
                        cell_borders(cell, top=("double" if self.st["table"] == "double" else "single", 6,
                                                self.st["text"]))
        self.para(after=6)

    def kv(self, rows, shade=True):
        st = self.st
        t = self.d.add_table(rows=len(rows), cols=2)
        no_borders(t)
        cell_margins(t, 50, 50, 110, 110)
        w = self.width()
        set_widths(t, w, [0.26, 0.74])
        for i, row in enumerate(rows):
            k, v = (list(row) + ["", ""])[:2]
            a, b = t.cell(i, 0), t.cell(i, 1)
            a.width, b.width = Emu(int(w * 0.26)), Emu(int(w * 0.74))
            pa, pb = a.paragraphs[0], b.paragraphs[0]
            pa.paragraph_format.space_after = pb.paragraph_format.space_after = Pt(0)
            self.label_run(pa, k)
            self.runs(pb, v)
            if shade and st["table"] in ("header", "zebra", "brutal"):
                cell_shade(a, st["alt"])
            cell_borders(a, bottom=("single", 2, mix(st["text"], "FFFFFF", 0.75)))
            cell_borders(b, bottom=("single", 2, mix(st["text"], "FFFFFF", 0.75)))
        self.para(after=4)

    def callout(self, text):
        st = self.st
        p = self.para(text)
        p.paragraph_format.left_indent = Mm(4)
        p.paragraph_format.right_indent = Mm(4)
        pshade(p, st["alt"] if self.style_name != "pop" else st["accent3"])
        if self.style_name == "pop":
            for side in ("top", "left", "bottom", "right"):
                pborder(p, **{side: ("single", 12, "111111", 6)})
        else:
            pborder(p, left=("single", 24, st["accent"], 8))
        return p

    def image(self, path, caption=None, width=0.6, align="c"):
        path = self.path(path)
        if not path or not os.path.exists(path):
            return
        p = self.para(align=align, after=2)
        p.add_run().add_picture(path, width=Emu(int(self.width() * float(width or 0.6))))
        if caption:
            self.para(caption, "Caption", align=align)

    def entry(self, e):
        st = self.st
        p = self.para(after=0, before=6)
        r = p.add_run(fr(e.get("title", "")))
        r.bold = True
        r.font.size = Pt(st["size"] + 1)
        if e.get("dates"):
            p.add_run("\t")
            self.label_run(p, e["dates"], st["muted"])
            sec = self.d.sections[-1]
            p.paragraph_format.tab_stops.add_tab_stop(self.width() if not self.in_cv else Emu(int(self.width() * 0.66)),
                                                      alignment=2)
        if e.get("org"):
            q = self.para(after=2)
            rr = q.add_run(fr(e["org"]))
            rr.italic = True
            rr.font.color.rgb = RGBColor.from_string(st["accent"])
        if e.get("bullets"):
            for b in e["bullets"]:
                self.para(b, "List Bullet")
        elif e.get("text"):
            self.para(e["text"])

    def toc(self):
        st = self.st
        h = self.para("Sommaire", "TOC Heading")
        p = self.d.add_paragraph()
        r = p.add_run()
        fc = OxmlElement("w:fldChar")
        fc.set(qn("w:fldCharType"), "begin")
        r._r.append(fc)
        r2 = p.add_run()
        it = OxmlElement("w:instrText")
        it.set(qn("xml:space"), "preserve")
        it.text = 'TOC \\o "1-2" \\h \\z \\u'
        r2._r.append(it)
        r3 = p.add_run()
        fc = OxmlElement("w:fldChar")
        fc.set(qn("w:fldCharType"), "separate")
        r3._r.append(fc)
        # pre-filled entries (Word refreshes them with page numbers: right-click > Update field)
        last = p
        for b in self.spec.get("blocks", []):
            if b.get("heading") and int(b.get("level", 1)) <= 2:
                q = self.d.add_paragraph(style=self.style(f"TOC {int(b.get('level', 1))}"))
                q.add_run(fr(b["heading"]))
                last = q
        r4 = last.add_run()
        fc = OxmlElement("w:fldChar")
        fc.set(qn("w:fldCharType"), "end")
        r4._r.append(fc)

    @staticmethod
    def normalize(b):
        """Accept {"type": "...", ...} blocks and other common shapes models write."""
        if not isinstance(b, dict):
            return {"text": str(b)}
        b = dict(b)
        t = str(b.pop("type", "") or b.pop("kind", "")).lower()
        body = b.get("text") or b.get("content") or b.get("value")
        rows = b.get("rows") or b.get("items") or b.get("data")
        if t in ("heading", "title", "h1", "h2", "h3"):
            b = {"heading": body or b.get("heading", ""), "level": b.get("level") or (int(t[1]) if t[:1] == "h" and t[1:].isdigit() else 1)}
        elif t in ("paragraph", "text", "p"):
            b = {"text": body or ""}
        elif t in ("bullets", "list", "bullet_list", "ul"):
            b = {"bullets": rows or []}
        elif t in ("numbers", "numbered", "ol"):
            b = {"numbers": rows or []}
        elif t in ("table",):
            b = {"table": rows or []}
        elif t in ("items", "lines", "quote_items", "invoice"):
            b = {"items": rows or [], "vat": b.get("vat") or b.get("tva")}
        elif t in ("kv", "key_values", "info"):
            b = {"kv": rows or []}
        elif t in ("actions",):
            b = {"actions": rows or []}
        elif t in ("image", "picture"):
            b = {"image": b.get("path") or b.get("image"), "caption": b.get("caption"), "width": b.get("width", 0.6)}
        elif t in ("signature",):
            b = {"signature": b.get("lines") or body or []}
        elif t in ("callout", "note"):
            b = {"callout": body or ""}
        elif t in ("quote",):
            b = {"quote": body or b.get("quote", ""), "author": b.get("author")}
        elif t in ("toc",):
            b = {"toc": True}
        elif t in ("page_break", "pagebreak"):
            b = {"page_break": True}
        elif t in ("entry", "experience", "job"):
            b = {"entry": b.get("entry") or b}
        for key in ("items", "table", "actions", "kv"):
            v = b.get(key)
            if isinstance(v, dict) and key == "kv":
                b[key] = [[k, str(x)] for k, x in v.items()]
            elif isinstance(v, list) and v and isinstance(v[0], dict):
                head = list(v[0].keys())
                b[key] = [head] + [[r.get(h, "") for h in head] for r in v]
        return b

    def block(self, b):
        b = self.normalize(b)
        if "heading" in b:
            self.heading(b["heading"], b.get("level", 1))
        elif "text" in b:
            for chunk in str(b["text"]).split("\n\n"):
                self.para(chunk.replace("\n", " "))
        elif "bullets" in b:
            for it in b["bullets"]:
                self.para(it, "List Bullet")
        elif "numbers" in b:
            for it in b["numbers"]:
                self.para(it, "List Number")
        elif "quote" in b:
            p = self.para(b["quote"], "Quote")
            if b.get("author"):
                q = self.para("— " + b["author"], "Quote")
                q.runs[0].italic = False
                q.runs[0].font.size = Pt(self.st["size"])
        elif "callout" in b:
            self.callout(b["callout"])
        elif "table" in b:
            self.table(b["table"])
        elif "items" in b:
            self.items(b["items"], b.get("vat"), b.get("currency", "€"))
        elif "kv" in b:
            self.kv(b["kv"])
        elif "actions" in b:
            self.table(b["actions"], numeric_right=False)
        elif "image" in b:
            self.image(b["image"], b.get("caption"), b.get("width", 0.6))
        elif "entry" in b:
            self.entry(b["entry"])
        elif "code" in b:
            for line in str(b["code"]).split("\n"):
                p = self.d.add_paragraph(style="Code" if not self.templated else None)
                p.add_run(line or " ")
            self.para(after=4)
        elif b.get("toc"):
            self.toc()
        elif b.get("page_break"):
            self.d.add_paragraph().add_run().add_break(WD_BREAK.PAGE)
        elif "signature" in b:
            sig = b["signature"] if isinstance(b["signature"], list) else [b["signature"]]
            self.para(before=18, after=0)
            for i, line in enumerate(sig):
                p = self.para(line, align="r", after=0)
                if i == len(sig) - 1:
                    p.runs[0].bold = True if p.runs else None

    # -- headers per kind --------------------------------------------------------------------------------------------
    def footer(self):
        st = self.st
        sec = self.d.sections[0]
        f = sec.footer
        p = f.paragraphs[0] if f.paragraphs else f.add_paragraph()
        p.text = ""
        p.alignment = WD_ALIGN_PARAGRAPH.CENTER
        txt = self.spec.get("footer") or ""
        if txt:
            r = p.add_run(fr(txt) + "   ·   ")
            set_font(r, st["body"], st["size"] - 2.5, color=st["muted"])
        r = p.add_run("")
        field(p, "PAGE")
        for r in p.runs:
            set_font(r, st["body"], st["size"] - 2.5, color=st["muted"])
        if self.style_name in ("rapport", "suisse", "terminal") and not self.templated:
            pborder(p, top=("single", 4, mix(st["text"], "FFFFFF", 0.7), 6))

    def logo_para(self, align="l", height=14):
        if self.logo and os.path.exists(self.logo):
            p = self.para(align=align, after=4)
            p.add_run().add_picture(self.logo, height=Mm(height))
            return p

    def masthead(self):
        """Title block of a plain document (rapport, compte_rendu, fiche, autre)."""
        st, sp = self.st, self.spec
        if sp.get("kicker"):
            p = self.para(align=st["title_align"], after=2)
            self.label_run(p, sp["kicker"])
        self.logo_para(st["title_align"])
        if sp.get("title"):
            p = self.para(sp["title"], "Title")
            if self.style_name == "artisan" and not self.templated:
                pborder(p, top=("double", 6, st["text"], 6), bottom=("double", 6, st["text"], 6))
            if self.style_name == "terminal" and not self.templated:
                pshade(p, st["dark"])
                p.runs[0].font.color.rgb = RGBColor.from_string("FFFFFF")
                pre = p.runs[0]
                r0 = p.insert_paragraph_before().add_run("$ cat " + re.sub(r"\W+", "-", sp["title"].lower())[:30] + ".md")
                set_font(r0, "Courier New", st["size"], color=st["accent"])
            if self.style_name == "pop" and not self.templated:
                pshade(p, st["accent3"])
                for side in ("top", "left", "bottom", "right"):
                    pborder(p, **{side: ("single", 18, "111111", 4)})
            if self.style_name == "suisse" and not self.templated:
                pborder(p, top=("single", 36, st["accent"], 10))
            if self.style_name in ("rapport", "ecole") and not self.templated:
                pshade(p, st["accent"])
                for side in ("top", "left", "bottom", "right"):
                    pborder(p, **{side: ("single", 4, st["accent"], 8)})
                for r in p.runs:
                    r.font.color.rgb = RGBColor.from_string("FFFFFF")
            if self.style_name in ("maison",) and not self.templated:
                pborder(p, bottom=("single", 4, st["accent"], 8))
        if sp.get("subtitle"):
            self.para(sp["subtitle"], "Subtitle")
        if self.style_name == "editorial" and not self.templated:
            p = self.para(after=10)
            pborder(p, bottom=("single", 6, st["text"], 1))

    def party_lines(self, cell, party, align="l", big=True):
        st = self.st
        p = cell.paragraphs[0]
        p.alignment = AL[align]
        p.paragraph_format.space_after = Pt(1)
        r = p.add_run(fr(party.get("name", "")))
        r.bold = True
        r.font.size = Pt(st["size"] + (2 if big else 0.5))
        for line in party.get("lines", []):
            q = cell.add_paragraph()
            q.alignment = AL[align]
            q.paragraph_format.space_after = Pt(0)
            q.add_run(fr(line))

    def band(self, name, title):
        """The style's letterhead: a coloured band, centred caps between rules, or a strong rule."""
        st = self.st
        w = self.width()
        v = {"rapport": "band", "ecole": "band", "terminal": "band", "pop": "band", "maison": "center",
             "artisan": "center", "editorial": "rule", "suisse": "rule"}.get(self.style_name, "rule")
        if self.templated:
            v = "plain"
        if v == "band":
            fill = {"terminal": st["dark"], "pop": st["accent3"]}.get(self.style_name, st["accent"])
            tc = "111111" if lum(fill) > 0.5 else "FFFFFF"
            t = self.d.add_table(rows=1, cols=2)
            no_borders(t)
            cell_margins(t, 200, 200, 220, 220)
            set_widths(t, w, [0.6, 0.4])
            a, b = t.cell(0, 0), t.cell(0, 1)
            for c in (a, b):
                cell_shade(c, fill)
                if self.style_name == "pop":
                    cell_borders(c, top=("single", 18, "111111"), bottom=("single", 18, "111111"),
                                 left=("single", 18 if c is a else 0, "111111" if c is a else "auto"),
                                 right=("single", 18 if c is b else 0, "111111" if c is b else "auto"))
                c.vertical_alignment = WD_CELL_VERTICAL_ALIGNMENT.CENTER
            pa = a.paragraphs[0]
            pa.paragraph_format.space_after = Pt(0)
            if self.logo and os.path.exists(self.logo):
                pa.add_run().add_picture(self.logo, height=Mm(12))
                pa = a.add_paragraph()
            if self.style_name == "terminal":
                r = pa.add_run("~/ ")
                set_font(r, "Courier New", st["size"] + 2, color="7EE787")
            r = pa.add_run(fr(name))
            set_font(r, st["head"], st["size"] + 5, bold=True, color=tc)
            pb = b.paragraphs[0]
            pb.alignment = WD_ALIGN_PARAGRAPH.RIGHT
            pb.paragraph_format.space_after = Pt(0)
            r = pb.add_run(fr(title))
            set_font(r, st["head"], st["title_size"] * 0.85, bold=True, color=tc)
            self.para(after=6)
            return
        if v == "center":
            self.logo_para("c")
            p = self.para(align="c", after=2)
            r = p.add_run(fr(name))
            set_font(r, st["head"], st["size"] + 7, bold=self.style_name == "artisan", color=st["text"], caps=True,
                     spacing=3)
            kind = "double" if self.style_name == "artisan" else "single"
            pborder(p, bottom=(kind, 6 if kind == "double" else 4, st["accent"], 6))
            if self.style_name == "artisan":
                pborder(p, top=("double", 6, st["accent"], 6))
                for r in p.runs:
                    r.font.color.rgb = RGBColor.from_string(st["accent"])
            if title:
                q = self.para(align="c", before=8, after=10)
                r = q.add_run(fr(("—  " + title + "  —") if self.style_name == "artisan" else title))
                set_font(r, st["lab"], st["size"] + (2 if self.style_name == "artisan" else 0), bold=True,
                         color=st["accent2"] if self.style_name == "artisan" else st["accent"], caps=True, spacing=4)
            return
        if v == "rule":
            t = self.d.add_table(rows=1, cols=2)
            no_borders(t)
            cell_margins(t, 120, 60, 0, 0)
            set_widths(t, w, [0.55, 0.45])
            a, b = t.cell(0, 0), t.cell(0, 1)
            thick = 36 if self.style_name == "suisse" else 8
            col = st["accent"] if self.style_name == "suisse" else st["text"]
            for c in (a, b):
                cell_borders(c, top=("single", thick, col))
                c.vertical_alignment = WD_CELL_VERTICAL_ALIGNMENT.BOTTOM
            pa = a.paragraphs[0]
            if self.logo and os.path.exists(self.logo):
                pa.add_run().add_picture(self.logo, height=Mm(12))
                pa = a.add_paragraph()
            if self.style_name == "editorial":
                self.label_run(pa, name, st["text"])
            else:
                r = pa.add_run(fr(name))
                set_font(r, st["head"], st["size"] + 4, bold=True, color=st["text"])
            pb = b.paragraphs[0]
            pb.alignment = WD_ALIGN_PARAGRAPH.RIGHT
            r = pb.add_run(fr(title))
            set_font(r, st["head"], st["title_size"] * (1.15 if self.style_name == "editorial" else 1.0),
                     bold=self.style_name == "suisse", color=st["text"])
            self.para(after=4)
            return
        self.logo_para("l")
        p = self.para(name, after=0)
        if title:
            self.para(title, "Title")

    def devis_head(self):
        st, sp = self.st, self.spec
        fr_ = sp.get("from") or {}
        self.band(fr_.get("name", ""), sp.get("title") or ("Facture" if sp.get("kind") == "facture" else "Devis"))
        t = self.d.add_table(rows=1, cols=2)
        no_borders(t)
        w = self.width()
        set_widths(t, w, [0.55, 0.45])
        a, b = t.cell(0, 0), t.cell(0, 1)
        pa = a.paragraphs[0]
        pa.paragraph_format.space_after = Pt(0)
        for i, line in enumerate(fr_.get("lines", [])):
            q = pa if i == 0 else a.add_paragraph()
            q.paragraph_format.space_after = Pt(0)
            rr = q.add_run(fr(line))
            rr.font.color.rgb = RGBColor.from_string(st["muted"])
        p = b.paragraphs[0]
        p.paragraph_format.space_after = Pt(0)
        first = True
        for k, v in sp.get("meta", []):
            q = p if first else b.add_paragraph()
            first = False
            q.alignment = WD_ALIGN_PARAGRAPH.RIGHT
            q.paragraph_format.space_after = Pt(0)
            self.label_run(q, k + "  ", st["muted"])
            q.add_run(fr(v))
        self.para(after=8)
        if sp.get("to"):
            t2 = self.d.add_table(rows=1, cols=2)
            no_borders(t2)
            cell_margins(t2, 110, 110, 160, 160)
            l, r = t2.cell(0, 0), t2.cell(0, 1)
            set_widths(t2, w, [0.5, 0.5])
            lp = l.paragraphs[0]
            if sp.get("subtitle"):
                self.label_run(lp, "Objet")
                l.add_paragraph().add_run(fr(sp["subtitle"]))
            hp = r.paragraphs[0]
            self.label_run(hp, "Client")
            r.add_paragraph()
            self.party_lines_cell_append(r, sp["to"], big=False)
            if st["table"] == "double":
                cell_shade(r, st["alt"])
                cell_borders(r, top=("double", 6, st["accent"]), bottom=("double", 6, st["accent"]),
                             left=("double", 6, st["accent"]), right=("double", 6, st["accent"]))
            elif st["table"] == "hairline":
                cell_borders(r, top=("single", 4, st["accent"]), bottom=("single", 4, st["accent"]),
                             left=("single", 4, st["accent"]), right=("single", 4, st["accent"]))
            elif st["table"] == "brutal":
                cell_borders(r, top=("single", 12, "111111"), bottom=("single", 12, "111111"),
                             left=("single", 12, "111111"), right=("single", 12, "111111"))
            else:
                cell_shade(r, st["alt"])
            self.para(after=8)

    def party_lines_cell_append(self, cell, party, big=True):
        st = self.st
        p = cell.paragraphs[-1]
        p.paragraph_format.space_after = Pt(1)
        r = p.add_run(fr(party.get("name", "")))
        r.bold = True
        r.font.size = Pt(st["size"] + (2 if big else 0.5))
        for line in party.get("lines", []):
            q = cell.add_paragraph()
            q.paragraph_format.space_after = Pt(0)
            q.add_run(fr(line))

    def letter_head(self):
        st, sp = self.st, self.spec
        fr_ = sp.get("from") or {}
        self.band(fr_.get("name", ""), sp.get("title") or "")
        for line in fr_.get("lines", []):
            q = self.para(line, after=0)
            if self.style_name in ("maison", "artisan"):
                q.alignment = WD_ALIGN_PARAGRAPH.CENTER
            for rr in q.runs:
                rr.font.color.rgb = RGBColor.from_string(st["muted"])
        sep = self.para(after=18)
        pborder(sep, bottom=("double" if self.style_name == "artisan" else "single", 4, st["accent"], 4))
        to = sp.get("to") or {}
        for i, line in enumerate([to.get("name", "")] + to.get("lines", [])):
            q = self.para(line, after=0)
            q.paragraph_format.left_indent = Mm(95)
            if i == 0 and q.runs:
                q.runs[0].bold = True
        for k, v in sp.get("meta", []):
            q = self.para(f"{v}" if k.lower() in ("date", "lieu et date", "le") else f"{k} {v}", after=0, before=14)
            q.paragraph_format.left_indent = Mm(95)
        if sp.get("subtitle"):
            q = self.para(before=22, after=12)
            r = q.add_run("Objet : ")
            r.bold = True
            q.add_run(fr(sp["subtitle"]))

    def cv(self):
        """Two columns: a coloured sidebar (photo, contact, skills) and the main column (blocks)."""
        st, sp = self.st, self.spec
        cv = sp.get("cv") or {}
        self.in_cv = True
        if not cv.get("sidebar"):     # short lists (skills, languages, diplomas) go to the side column
            blocks = [self.normalize(b) for b in sp.get("blocks", [])]
            keep, side, i = [], [], 0
            while i < len(blocks):
                b = blocks[i]
                nxt = blocks[i + 1] if i + 1 < len(blocks) else {}
                after = blocks[i + 2] if i + 2 < len(blocks) else {"heading": ""}
                items = nxt.get("bullets") or []
                if "heading" in b and items and "heading" in after and all(len(str(x)) <= 42 for x in items):
                    side.append({"heading": b["heading"], "items": items})
                    i += 2
                    continue
                keep.append(b)
                i += 1
            cv["sidebar"], sp["blocks"] = side, keep
        w = self.width()
        side_left = self.style_name not in ("suisse", "editorial")
        t = self.d.add_table(rows=1, cols=2)
        no_borders(t)
        cell_margins(t, 160, 160, 200, 200)
        side, main = (t.cell(0, 0), t.cell(0, 1)) if side_left else (t.cell(0, 1), t.cell(0, 0))
        set_widths(t, w, [0.32, 0.68] if side_left else [0.68, 0.32])
        sfill = {"pop": st["accent3"], "ecole": st["alt"], "terminal": st["dark"], "rapport": st["accent"],
                 "artisan": st["alt"], "maison": st["alt"]}.get(self.style_name)
        dark = sfill and lum(sfill) < 0.3
        if sfill:
            cell_shade(side, sfill)
        elif self.style_name in ("suisse", "editorial"):
            cell_borders(side, left=("single", 6 if self.style_name == "editorial" else 18, st["text"]))
        tc = "FFFFFF" if dark else st["text"]
        ac = st["accent3"] if dark and self.style_name == "rapport" else (st["accent"] if not dark else "7EE787")
        if self.style_name == "pop":
            ac = "111111"
        sp_ = side.paragraphs[0]
        if cv.get("photo") and os.path.exists(self.path(cv["photo"])):
            sp_.add_run().add_picture(self.path(cv["photo"]), width=Emu(int(w * 0.24)))
            sp_ = side.add_paragraph()
        first = True
        for blk in [{"heading": "Contact", "items": cv.get("contact", [])}] + (cv.get("sidebar") or []):
            if not blk.get("items"):
                continue
            hp = sp_ if first else side.add_paragraph()
            first = False
            hp.paragraph_format.space_before = Pt(10)
            hp.paragraph_format.space_after = Pt(3)
            r = hp.add_run(fr(blk.get("heading", "")))
            set_font(r, st["lab"], st["size"] - 1, bold=True, color=ac, caps=st["lab"] != "Courier New",
                     spacing=1.2)
            for it in blk["items"]:
                q = side.add_paragraph()
                q.paragraph_format.space_after = Pt(2)
                rr = q.add_run(fr(it))
                set_font(rr, st["body"], st["size"] - 0.5, color=tc)
        # main column
        mp = main.paragraphs[0]
        r = mp.add_run(fr(cv.get("name") or sp.get("title", "")))
        set_font(r, st["head"], st["title_size"], bold=st["h1"][2] and st["head"] != "Times New Roman",
                 color=st["text"], caps=st.get("title_upper", False), spacing=2 if st.get("title_upper") else 0)
        mp.paragraph_format.space_after = Pt(2)
        if cv.get("headline") or sp.get("subtitle"):
            q = main.add_paragraph()
            rr = q.add_run(fr(cv.get("headline") or sp.get("subtitle")))
            set_font(rr, st["body"], st["size"] + 2.5, color=st["accent"], italic=st["body"] == "Georgia")
            q.paragraph_format.space_after = Pt(10)
            pborder(q, bottom=("single", 6, st["accent"] if self.style_name != "artisan" else st["text"], 6))
        # blocks go into the main cell: temporarily redirect add_paragraph/add_table
        self._cell = main
        for b in sp.get("blocks", []):
            self.cell_block(main, self.normalize(b))
        self._cell = None

    def cell_block(self, cell, b):
        st = self.st
        if "heading" in b:
            p = cell.add_paragraph(style=self.style(f"Heading {max(1, min(3, int(b.get('level', 1))))}"))
            if st["h1"][0] == "mono_prefix" and int(b.get("level", 1)) == 1:
                r = p.add_run("## ")
                set_font(r, "Courier New", color=st["accent"])
            p.add_run(fr(b["heading"]))
            p.paragraph_format.space_before = Pt(10)
        elif "entry" in b:
            e = b["entry"]
            p = cell.add_paragraph()
            p.paragraph_format.space_before = Pt(6)
            p.paragraph_format.space_after = Pt(0)
            r = p.add_run(fr(e.get("title", "")))
            r.bold = True
            r.font.size = Pt(st["size"] + 1)
            if e.get("dates"):
                p.add_run("\t")
                self.label_run(p, e["dates"], st["muted"])
                p.paragraph_format.tab_stops.add_tab_stop(Emu(int(self.width() * 0.68 - 254000)), alignment=2)
            if e.get("org"):
                q = cell.add_paragraph()
                q.paragraph_format.space_after = Pt(2)
                rr = q.add_run(fr(e["org"]))
                rr.italic = True
                rr.font.color.rgb = RGBColor.from_string(st["accent"])
            for x in e.get("bullets", []) or []:
                cell.add_paragraph(style=self.style("List Bullet")).add_run(fr(x))
            if e.get("text"):
                cell.add_paragraph().add_run(fr(e["text"]))
        elif "bullets" in b:
            for x in b["bullets"]:
                cell.add_paragraph(style=self.style("List Bullet")).add_run(fr(x))
        elif "text" in b:
            cell.add_paragraph().add_run(fr(b["text"]))

    # -- build -------------------------------------------------------------------------------------------------------
    def build(self, out):
        sp = self.spec
        for k in ("from", "to"):
            if isinstance(sp.get(k), str):
                lines = [x.strip() for x in sp[k].split("\n") if x.strip()]
                sp[k] = {"name": lines[0] if lines else "", "lines": lines[1:]}
        if isinstance(sp.get("meta"), dict):
            sp["meta"] = [[k, str(v)] for k, v in sp["meta"].items()]
        kind = (sp.get("kind") or "autre").lower()
        self.in_cv = False
        if kind in ("devis", "facture"):
            self.devis_head()
        elif kind == "lettre":
            self.letter_head()
        elif kind == "cv":
            self.cv()
        elif kind == "rapport" and len(sp.get("blocks", [])) > 12:
            self.masthead()
            if sp.get("from"):
                self.para((sp["from"].get("name") or ""), after=0)
            self.d.add_paragraph().add_run().add_break(WD_BREAK.PAGE)
            if not any(b.get("toc") for b in sp.get("blocks", [])):
                self.toc()
                self.d.add_paragraph().add_run().add_break(WD_BREAK.PAGE)
        else:
            self.masthead()
            if sp.get("meta"):
                self.kv(sp["meta"])
        if kind == "newsletter":
            sec = self.d.add_section(WD_SECTION.CONTINUOUS)
            cols = sec._sectPr.xpath("./w:cols")
            c = cols[0] if cols else OxmlElement("w:cols")
            c.set(qn("w:num"), "2")
            c.set(qn("w:space"), "567")
            if not cols:
                sec._sectPr.append(c)
        if kind != "cv":
            for b in sp.get("blocks", []):
                self.block(b)
        self.footer()
        self.d.core_properties.title = sp.get("title") or ""
        self.d.save(out)


def main(argv=None):
    styles = "\n".join(f" {k:<10} {v['hint']}" for k, v in STYLES.items())
    ap = argparse.ArgumentParser(description="Content-only JSON -> designed .docx",
                                 epilog=__doc__ + "\nSTYLES\n" + styles,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("spec")
    ap.add_argument("output")
    ap.add_argument("--style", help="override the spec's style (same content, another look)")
    a = ap.parse_args(argv)
    with open(a.spec, encoding="utf-8") as f:
        spec = json.load(f)
    if a.style:
        spec["style"] = a.style
    d = Doc(spec, os.path.dirname(os.path.abspath(a.spec)))
    d.build(a.output)
    out = {"ok": True, "output": a.output, "style": d.style_name}
    if shutil.which("soffice") or shutil.which("libreoffice"):   # a preview only where LibreOffice is installed
        out["look"] = f"python3 {os.path.join(HERE, 'look.py')} {a.output} --outdir render"
    print(json.dumps(out, ensure_ascii=False))


if __name__ == "__main__":
    sys.exit(main())
