#!/usr/bin/env python3
"""Content-only JSON -> designed, fully editable .pptx (native charts and tables). No positions, no colours.

{"style": "editorial",            # see STYLES below; omit = suisse
 "title": "Le Fournil de Léa",    # deck name (footers)
 "brand": {"colors": ["#0A7D5A", "#F2C14E"], "logo": "logo.png"},   # optional: the person's colours / logo
 "template": "modele.pptx",       # optional: the person's own .pptx (its colours, fonts and master are kept)
 "slides": [ ... one object per slide, "layout" + its fields; every slide may add "notes" ... ]}

LAYOUTS (fields)
 cover        title, subtitle, kicker, image
 section      title, subtitle, image                     (numbered automatically)
 agenda       title, items[]                             (omit items = the section titles)
 bullets      title, bullets[] (≤ 5), image
 two_columns  title, left{heading, bullets[] | text}, right{...}
 big_number   value, label, context
 stats        title, stats[{value, label, delta}] (2-4), note
 chart        title, chart{type: bar|bar_h|line|pie|doughnut|stacked, categories[], series{name: [numbers]},
              unit, highlight}, takeaway, source
 table        title, rows[[header...], [...]], note, source  (a first cell « Total… » row is bold)
 image_text   title, text | bullets[], image, caption, kicker
 image_full   image, title, caption
 gallery      title, images[{path, caption}] (2-4)
 timeline     title, steps[{label, text}] (3-6), image (e.g. a map: then steps go down the left)
 people       title, people[{name, role, text, photo}] (1-6)
 quote        quote, author
 code         title, code, caption
 closing      title, subtitle, contact
Short text: titles ≤ 60 characters, bullets ≤ 90. Relative image paths are read from the spec's folder.
"""
import argparse
import copy
import json
import math
import os
import re
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import _deps  # noqa: E402

_deps.need("pptx", "PIL")

from lxml import etree
from pptx import Presentation
from pptx.chart.data import CategoryChartData
from pptx.dml.color import RGBColor
from pptx.enum.chart import XL_CHART_TYPE, XL_LEGEND_POSITION, XL_LABEL_POSITION, XL_MARKER_STYLE
from pptx.enum.shapes import MSO_SHAPE
from pptx.enum.text import PP_ALIGN, MSO_ANCHOR
from pptx.oxml.ns import qn
from pptx.util import Inches, Pt, Emu

from styles import STYLES, DEFAULT  # noqa: E402


def agent_default():
    """The style this agent uses when the subject calls for none in particular: `defaults.json` next to SKILL.md,
    written by the bridge for the agent's kind (style_skills.rs); else the scripts' own default."""
    try:
        with open(os.path.join(HERE, "..", "defaults.json"), encoding="utf-8") as f:
            name = json.load(f).get("slides")
    except (OSError, ValueError, AttributeError):
        name = None
    return name if name in STYLES else DEFAULT

AL = {"l": PP_ALIGN.LEFT, "c": PP_ALIGN.CENTER, "r": PP_ALIGN.RIGHT}
AN = {"t": MSO_ANCHOR.TOP, "m": MSO_ANCHOR.MIDDLE, "b": MSO_ANCHOR.BOTTOM}
ROMAN = ["I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X", "XI", "XII"]


def rgb(h):
    return RGBColor.from_string(h.lstrip("#").upper())


def mix(a, b, t):
    a, b = a.lstrip("#"), b.lstrip("#")
    return "".join(f"{round(int(a[i:i + 2], 16) * (1 - t) + int(b[i:i + 2], 16) * t):02X}" for i in (0, 2, 4))


def lum(h):
    h = h.lstrip("#")
    c = [int(h[i:i + 2], 16) / 255 for i in (0, 2, 4)]
    c = [v / 12.92 if v <= 0.03928 else ((v + 0.055) / 1.055) ** 2.4 for v in c]
    return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]


def on(h):
    return "FFFFFF" if lum(h) < 0.4 else "111111"


NB = "\u00a0"


def fr(t):
    """French typography: no line break before « : ; ! ? % € » or inside « 12 000 »."""
    t = str(t or "")
    t = re.sub(r"« ", "«" + NB, t)
    t = re.sub(r" (»|:|;|!|\?|%|€|k€|M€|\$)", NB + r"\1", t)
    t = re.sub(r"(?<=\d) (?=\d{3}\b)", NB, t)
    return t


_M = json.load(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "metrics.json")))
_IDX = {c: i for i, c in enumerate(_M["chars"])}


def num(x):
    """A number from 12, "12", "12,5", "1 200 €"; None when there is none."""
    if isinstance(x, (int, float)):
        return float(x)
    t = re.sub(r"[^0-9,.\-]", "", str(x or "").replace("\u00a0", "").replace(" ", "")).replace(",", ".")
    try:
        return float(t)
    except ValueError:
        return None


def width_in(t, size, font="Arial", bold=False, track=0.0):
    """Width of a string in inches, from the real advance widths of the font (metrics.json)."""
    table = _M.get(font, _M["Arial"])[1 if bold else 0]
    avg = table[_IDX["n"]]
    em = sum(table[_IDX[c]] if c in _IDX else (avg * 1.1) for c in t)
    return em * size / 1000 / 72 + track * len(t) / 72


def lines_needed(text, w_in, size, font="Arial", bold=False, track=0.0):
    """Lines the text takes; None when one word cannot fit a line (then the size must go down)."""
    w_in *= 0.97
    space = width_in(" ", size, font, bold, track)
    n = 0
    for para in str(text).split("\n"):
        cur, k = 0.0, 1
        for wd in para.split(" "):
            if not wd:
                continue
            ww = width_in(wd, size, font, bold, track)
            if ww > w_in:
                return None
            if cur and cur + space + ww > w_in:
                k, cur = k + 1, ww
            else:
                cur += ww + (space if cur else 0)
        n += k
    return n


def fit(text, w_in, h_in, size, font="Arial", bold=False, upper=False, track=0.0, spacing=1.0, minimum=9):
    text = str(text).upper() if upper else str(text)
    while size > minimum:
        n = lines_needed(text, w_in, size, font, bold, track)
        if n is not None and n * size * spacing * 1.2 / 72 <= h_in:
            return size
        size -= 1 if size < 30 else 2
    return minimum


def modified(credit):
    """A Creative Commons BY (or BY-SA) photo the style tinted or turned grey: its licence asks to say it was changed."""
    credit = str(credit)
    if re.search(r"\bCC[ -]?BY", credit, re.I) and not re.search(r"modifi", credit, re.I):
        return credit + " (modifiée)"
    return credit


def leaves(v):
    """The texts and numbers inside a field's value."""
    if isinstance(v, dict):
        return [x for k in v for x in leaves(v[k])]
    if isinstance(v, (list, tuple)):
        return [x for k in v for x in leaves(k)]
    return [] if v in (None, "") else [v]


class Deck:
    def __init__(self, spec, base):
        self.spec, self.base = spec, base
        name = str(spec.get("style") or "").strip().lower()
        self.style_name = name if name in STYLES else agent_default()
        st = copy.deepcopy(STYLES[self.style_name])
        self.fx = st.pop("fx")
        self.paint = True
        self.prs = Presentation(self.path(spec["template"])) if spec.get("template") else Presentation()
        if spec.get("template"):
            self.from_template(st)
        else:
            self.prs.slide_width, self.prs.slide_height = Inches(13.333), Inches(7.5)
        self.W, self.H = self.prs.slide_width / 914400, self.prs.slide_height / 914400
        self.apply_brand(st, spec.get("brand") or {})
        self.st = st
        self.mx = 1.0 if st.get("center") else 0.75
        self.blank = self.pick_blank()
        self.tmp = tempfile.mkdtemp(prefix="deck-")
        self.n = 0
        self.missing = []
        self.problems = []   # slides left empty or fields left out (check)
        slides = spec.get("slides", [])
        self.total = len(slides)
        self.sections = [s.get("title", "") for s in slides if s.get("layout") == "section"]
        self.sec_i = 0
        self.dname = spec.get("title") or next((s.get("title") for s in slides if s.get("layout") == "cover"), "")

    # -- the person's own look ---------------------------------------------------------------------------------
    def path(self, p):
        """Relative paths: the spec's folder, then the current folder, then home (where photo.py may have saved)."""
        if not p or os.path.isabs(p):
            return p
        for root in (self.base, os.getcwd(), os.path.expanduser("~"), "/root", "/workspace"):
            cand = os.path.join(root, p)
            if os.path.exists(cand):
                return cand
        return os.path.join(self.base, p)

    def drop_missing(self, d):
        """Pictures that do not exist are left out (the layout then works without them) and reported."""
        for key in ("image",):
            if d.get(key) and not self.has_img(d[key]):
                self.missing.append(d.pop(key))
        if d.get("images"):
            keep = []
            for im in d["images"]:
                path = im.get("path") if isinstance(im, dict) else im
                (keep.append(im) if self.has_img(path) else self.missing.append(path))
            d["images"] = keep
        for pp in d.get("people") or []:
            if isinstance(pp, dict) and pp.get("photo") and not self.has_img(pp["photo"]):
                self.missing.append(pp.pop("photo"))
        return d

    def from_template(self, st):
        """Their .pptx: drop its slides, keep masters; their theme colours and fonts replace the style's."""
        sld = self.prs.slides._sldIdLst
        for sid in list(sld):
            self.prs.part.drop_rel(sid.get(qn("r:id")))
            sld.remove(sid)
        self.paint = False
        try:
            from pptx.opc.constants import RELATIONSHIP_TYPE as RT
            theme = self.prs.slide_masters[0].part.part_related_by(RT.THEME)
            x = etree.fromstring(theme.blob)
        except Exception:
            return
        ns = {"a": "http://schemas.openxmlformats.org/drawingml/2006/main"}

        def col(tag):
            el = x.find(f".//a:clrScheme/a:{tag}", ns)
            if el is None or not len(el):
                return None
            c = el[0]
            return c.get("lastClr") or c.get("val")
        maj = x.find(".//a:majorFont/a:latin", ns)
        mnr = x.find(".//a:minorFont/a:latin", ns)
        if maj is not None and maj.get("typeface"):
            st["head"] = maj.get("typeface")
        if mnr is not None and mnr.get("typeface"):
            st["body"] = st["lab"] = mnr.get("typeface")
        dk, lt = col("dk1") or st["text"], col("lt1") or st["bg"]
        acc = [col(f"accent{i}") for i in range(1, 7)]
        acc = [a for a in acc if a]
        st.update(text=dk, bg=lt, muted=mix(dk, lt, 0.45), alt=mix(lt, dk, 0.07), line=dk)
        if acc:
            st.update(accent=acc[0], accent2=acc[1 % len(acc)], accent3=acc[2 % len(acc)], palette=acc,
                      on_accent=on(acc[0]))

    def apply_brand(self, st, brand):
        cols = [c.lstrip("#") for c in brand.get("colors") or [] if len(c.lstrip("#")) == 6]
        if cols:
            st["accent"] = cols[0]
            st["accent2"] = cols[1] if len(cols) > 1 else st["accent2"]
            st["accent3"] = cols[2] if len(cols) > 2 else st["accent3"]
            st["palette"] = cols + [p for p in st["palette"] if p not in cols]
            st["on_accent"] = on(cols[0])
        self.logo = self.path(brand.get("logo")) if brand.get("logo") else None

    def pick_blank(self):
        lays = list(self.prs.slide_layouts)
        for l in lays:
            if l.name.lower() in ("blank", "vide"):
                return l
        return min(lays, key=lambda l: len(l.placeholders))

    # -- primitives --------------------------------------------------------------------------------------------
    def new(self, bg=None, decor=True):
        s = self.prs.slides.add_slide(self.blank)
        for ph in list(s.placeholders):
            ph._element.getparent().remove(ph._element)
        if self.paint or bg:
            s.background.fill.solid()
            s.background.fill.fore_color.rgb = rgb(bg or self.st["bg"])
        self.n += 1
        if decor and not bg:
            self.decor(s)
        return s

    def rect(self, s, x, y, w, h, fill=None, line=None, lw=1.0, shape=MSO_SHAPE.RECTANGLE, rot=0):
        r = s.shapes.add_shape(shape, Inches(x), Inches(y), Inches(w), Inches(h))
        if fill:
            r.fill.solid()
            r.fill.fore_color.rgb = rgb(fill)
        else:
            r.fill.background()
        if line:
            r.line.color.rgb = rgb(line)
            r.line.width = Pt(lw)
        else:
            r.line.fill.background()
        r.shadow.inherit = False
        if rot:
            r.rotation = rot
        r.text_frame.text = ""
        return r

    def hline(self, s, x, y, w, color, pt=1.0):
        return self.rect(s, x, y, w, max(pt, 0.5) / 72, color)

    def vline(self, s, x, y, h, color, pt=1.0):
        return self.rect(s, x, y, max(pt, 0.5) / 72, h, color)

    def brutal(self, s, x, y, w, h, fill, shadow=0.1, lw=2.25, shape=MSO_SHAPE.RECTANGLE, rot=0):
        self.rect(s, x + shadow, y + shadow, w, h, self.st["line"], shape=shape, rot=rot)
        return self.rect(s, x, y, w, h, fill, self.st["line"], lw, shape=shape, rot=rot)

    def text(self, s, x, y, w, h, txt, size=18, color=None, font=None, bold=False, italic=False, align="l",
             anchor="t", upper=False, track=0.0, spacing=1.0, fitting=True, minimum=9, rot=0):
        txt = fr(txt)
        if upper:
            txt = txt.upper()
        font = font or self.st["body"]
        if fitting:
            size = fit(txt, w, h, size, font, bold, False, track, spacing, minimum)
        tb = s.shapes.add_textbox(Inches(x), Inches(y), Inches(w), Inches(h))
        tf = tb.text_frame
        tf.word_wrap = True
        tf.vertical_anchor = AN[anchor]
        tf.margin_left = tf.margin_right = tf.margin_top = tf.margin_bottom = 0
        for i, line in enumerate(txt.split("\n")):
            p = tf.paragraphs[0] if i == 0 else tf.add_paragraph()
            p.alignment = AL[align]
            p.line_spacing = spacing
            r = p.add_run()
            r.text = line
            f = r.font
            f.size, f.bold, f.italic, f.name = Pt(size), bold, italic, font
            f.color.rgb = rgb(color or self.st["text"])
            if track:
                r._r.get_or_add_rPr().set("spc", str(int(track * 100)))
        if rot:
            tb.rotation = rot
        return size

    def label(self, s, x, y, w, txt, color=None, size=11, align="l", h=0.3):
        st = self.st
        mono = st["lab"] == "Courier New"
        size = size * 1.15
        return self.text(s, x, y, w, h, txt, size, color or st["accent"], st["lab"], bold=not mono, align=align,
                         upper=not mono, track=0 if mono else 2.2, fitting=True, minimum=8)

    def head(self, s, x, y, w, h, txt, size=None, color=None, align="l", anchor="t", spacing=0.95, minimum=14):
        st = self.st
        return self.text(s, x, y, w, h, txt, size or st["head_size"], color, st["head"], st.get("head_bold"),
                         align=align, anchor=anchor, upper=st.get("head_upper"), track=st.get("head_track", 0),
                         spacing=spacing, minimum=minimum)

    def treat(self, path, mode):
        """A copy of the picture with the style's treatment (grey, duotone, warm)."""
        if mode in (None, "none", "frame", "brutal") or not path:
            return path
        try:
            from PIL import Image, ImageOps, ImageEnhance
            im = Image.open(path).convert("RGB")
            if mode == "gray":
                im = ImageEnhance.Contrast(ImageOps.grayscale(im)).enhance(1.15).convert("RGB")
            elif mode == "duotone":
                g = ImageEnhance.Contrast(ImageOps.grayscale(im)).enhance(1.1)
                im = ImageOps.colorize(g, "#" + self.st["bg"], "#" + mix(self.st["accent3"], "FFFFFF", 0.35))
            out = os.path.join(self.tmp, f"{mode}-{abs(hash(path))}.jpg")
            im.save(out, quality=88)
            self.treated = True   # the slide's photo credits say « modifiée » (build)
            return out
        except Exception:
            return path

    def picture(self, s, path, x, y, w, h, mode=None, circle=False):
        path = self.path(path)
        if not path or not os.path.exists(path):
            self.rect(s, x, y, w, h, self.st["alt"])
            self.text(s, x, y + h / 2 - 0.2, w, 0.4, "(image)", 12, self.st["muted"], align="c", fitting=False)
            return None
        mode = self.fx["image"] if mode is None else mode
        pic = s.shapes.add_picture(self.treat(path, mode), Inches(x), Inches(y))
        iw, ih = pic.image.size
        box, img = w / h, iw / ih
        if img > box:
            c = (1 - box / img) / 2
            pic.crop_left = pic.crop_right = c
        else:
            c = (1 - img / box) / 2
            pic.crop_top = pic.crop_bottom = c
        pic.left, pic.top, pic.width, pic.height = Inches(x), Inches(y), Inches(w), Inches(h)
        if circle:
            pic.auto_shape_type = MSO_SHAPE.OVAL
        return pic

    def framed(self, s, path, x, y, w, h, rot=0):
        """The style's way of showing a picture inside the page (not full-bleed)."""
        m = self.fx["image"]
        if m == "brutal":
            self.rect(s, x + 0.12, y + 0.12, w, h, self.st["line"], rot=rot)
            p = self.picture(s, path, x, y, w, h)
            if p is not None:
                p.line.color.rgb = rgb(self.st["line"])
                p.line.width = Pt(2.25)
                p.rotation = rot
            return p
        p = self.picture(s, path, x, y, w, h)
        if m == "frame":
            self.rect(s, x - 0.14, y - 0.14, w + 0.28, h + 0.28, None, self.st["line"], 0.75)
        return p

    def gradient(self, s, x, y, w, h, color="000000", a0=0, a1=75):
        r = self.rect(s, x, y, w, h, color)
        spPr = r.fill._xPr
        for el in spPr.findall(qn("a:solidFill")):
            spPr.remove(el)
        g = etree.SubElement(spPr, qn("a:gradFill"), rotWithShape="1")
        lst = etree.SubElement(g, qn("a:gsLst"))
        for pos, a in ((0, a0), (100000, a1)):
            gs = etree.SubElement(lst, qn("a:gs"), pos=str(pos))
            c = etree.SubElement(gs, qn("a:srgbClr"), val=color)
            etree.SubElement(c, qn("a:alpha"), val=str(a * 1000))
        etree.SubElement(g, qn("a:lin"), ang="5400000", scaled="0")
        ln = spPr.find(qn("a:ln"))
        if ln is not None:
            spPr.remove(ln)
            spPr.append(ln)
        return r

    def alpha(self, shape, percent):
        srgb = shape.fill._xPr.find(qn("a:solidFill")).find(qn("a:srgbClr"))
        etree.SubElement(srgb, qn("a:alpha"), val=str(int(percent * 1000)))

    def logo_at(self, s, x, y, h=0.55, right=True):
        if self.logo and os.path.exists(self.logo):
            from PIL import Image
            iw, ih = Image.open(self.logo).size
            w = h * iw / ih
            s.shapes.add_picture(self.logo, Inches(x - w if right else x), Inches(y), Inches(w), Inches(h))

    # -- decor, titles, footers ----------------------------------------------------------------------------------
    def decor(self, s, ruled=False):
        st, W, H, d = self.st, self.W, self.H, self.fx["decor"]
        if d == "frame":
            self.rect(s, 0.22, 0.22, W - 0.44, H - 0.44, None, st["line"], 1.25)
            self.rect(s, 0.3, 0.3, W - 0.6, H - 0.6, None, st["line"], 0.5)
        elif d == "notebook":
            self.vline(s, 0.5, 0, H, "E57373", 1.25)

    def geo_glyph(self, s, x, y, size, i):
        st = self.st
        cols = [st["accent"], st["accent2"], st["accent3"], st["text"]]
        shapes = [MSO_SHAPE.OVAL, MSO_SHAPE.RECTANGLE, MSO_SHAPE.ISOSCELES_TRIANGLE, MSO_SHAPE.OVAL]
        self.rect(s, x, y, size, size, cols[i % 4], shape=shapes[i % 4])

    def title(self, s, title, kicker=None):
        """Draws the style's title treatment; returns the y where content can start."""
        st, W, mx, t = self.st, self.W, self.mx, self.fx["title"]
        cw = W - 2 * mx
        if t == "rule":
            self.hline(s, mx, 0.5, cw, st["line"], 0.75)
            self.label(s, mx, 0.66, cw, kicker or self.dname, st["accent"], 10)
            self.head(s, mx, 1.0, cw * 0.85, 1.05, title, anchor="t")
            return 2.25
        if t == "grid":
            col = cw / 12
            self.hline(s, mx, 0.45, cw, st["line"], 1.5)
            self.text(s, mx, 0.62, col * 2, 0.4, f"{self.n:02d}", 14, st["accent"], st["head"], True, fitting=False)
            if kicker:
                self.text(s, mx, 1.0, col * 2.6, 0.6, kicker, 11, st["muted"], st["body"])
            self.head(s, mx + col * 3, 0.6, col * 9, 1.25, title, anchor="t")
            return 2.15
        if t == "prompt":
            sec = f"{self.sec_i:02d}" if self.sec_i else "00"
            self.text(s, mx, 0.5, cw, 0.35, f"// {sec}  {kicker or self.dname}".rstrip(), 13, st["accent"],
                      "Courier New", fitting=True)
            self.head(s, mx, 0.9, cw, 1.0, title)
            return 2.1
        if t == "sticker":
            if kicker:
                kw = min(cw, 0.6 + len(kicker) * 0.11)
                self.brutal(s, mx, 0.42, kw, 0.42, st["accent3"], 0.05, 1.5, rot=-2)
                self.text(s, mx, 0.42, kw, 0.42, kicker, 13, st["text"], st["lab"], True, align="c", anchor="m",
                          upper=True, track=1, rot=-2)
            self.head(s, mx, 0.95, cw, 1.1, title)
            return 2.2
        if t == "centered":
            self.label(s, mx, 0.6, cw, kicker or self.dname, st["muted"], 10, "c")
            self.head(s, mx, 0.95, cw, 0.75, title, align="c", anchor="m")
            self.hline(s, W / 2 - 0.6, 1.9, 1.2, st["accent"], 0.75)
            return 2.35
        if t == "action":
            self.label(s, W - mx - 4, 0.3, 4, kicker or self.dname, st["accent2"], 9, "r")
            self.head(s, mx, 0.55, cw, 0.95, title, anchor="b")
            self.hline(s, mx, 1.62, cw, st["line"], 1.0)
            return 1.9
        if t == "marker":
            size = fit(title, cw, 1.0, st["head_size"], st["head"], True)
            lw = min(cw, width_in(fr(title), size, st["head"], True) + 0.3)
            n = lines_needed(fr(title), cw, size, st["head"], True) or 2
            if n == 1:
                self.rect(s, mx - 0.12, 0.62 + size / 72 * 0.45, lw, size / 72 * 0.75, st["accent3"], rot=-0.8)
            if kicker:
                self.label(s, mx, 0.3, cw, kicker, st["accent"], 11)
            self.head(s, mx, 0.6, cw, 1.0, title)
            return 1.95
        if t == "place":
            self.label(s, mx, 0.55, cw, kicker or self.dname, st["accent"], 11)
            self.head(s, mx, 0.9, cw, 1.0, title)
            return 2.1
        if t == "label":
            self.label(s, mx + 0.1, 0.62, cw, kicker or self.dname, st["accent2"], 11)
            self.head(s, mx + 0.1, 0.95, cw - 0.2, 0.9, title, anchor="t")
            self.hline(s, mx + 0.1, 1.95, 1.4, st["line"], 0.75)
            self.hline(s, mx + 0.1, 2.0, 1.4, st["line"], 0.75)
            self.rect(s, mx + 1.6, 1.89, 0.17, 0.17, st["accent2"], shape=MSO_SHAPE.DIAMOND)
            return 2.35
        if t == "geo":
            self.geo_glyph(s, mx, 0.68, 0.42, self.n)
            if kicker:
                self.label(s, mx + 0.7, 0.35, cw, kicker, st["muted"], 10)
            self.head(s, mx + 0.7, 0.55, cw - 0.7, 1.1, title)
            return 2.0
        self.head(s, mx, 0.6, cw, 1.0, title)
        return 2.0

    def footer(self, s, source=None, dark=False):
        st, W, H, mx, f = self.st, self.W, self.H, self.mx, self.fx["footer"]
        c = st["muted"] if not dark else "DDDDDD"
        y = H - 0.48
        if f == "running":
            self.label(s, mx, y, 6, self.dname, c, 8)
            self.text(s, W - mx - 1, y, 1, 0.3, f"{self.n:02d}", 10, c, st["lab"], True, align="r", fitting=False)
        elif f == "grid":
            self.text(s, mx, y, 6, 0.3, self.dname, 10, c, st["body"], fitting=False)
            self.text(s, W - mx - 1, y, 1, 0.3, str(self.n), 10, st["text"] if not dark else "FFFFFF",
                      st["body"], True, align="r", fitting=False)
        elif f == "path":
            slug = "".join(ch if ch.isalnum() else "-" for ch in self.dname.lower()).strip("-")[:32] or "deck"
            self.text(s, mx, y, 6, 0.3, f"~/{slug}", 10, c, "Courier New", fitting=False)
            self.text(s, W - mx - 2, y, 2, 0.3, f"{self.n:02d}/{self.total:02d}", 10, c, "Courier New",
                      align="r", fitting=False)
        elif f == "pill":
            self.rect(s, W - mx - 0.42, y - 0.08, 0.42, 0.42, st["text"], shape=MSO_SHAPE.OVAL)
            self.text(s, W - mx - 0.42, y - 0.08, 0.42, 0.42, str(self.n), 11, st["bg"], st["body"], True,
                      align="c", anchor="m", fitting=False)
        elif f == "centered":
            self.text(s, W / 2 - 0.5, y, 1, 0.3, str(self.n), 10, c, st["head"], align="c", fitting=False)
        elif f == "source":
            if source:
                self.text(s, mx, y - 0.05, W - 2 * mx - 1.5, 0.35, "Source : " + source, 9, c, st["body"])
            else:
                self.text(s, mx, y, 6, 0.3, self.dname, 9, c, st["body"], fitting=False)
            self.rect(s, W - mx - 0.55, y + 0.07, 0.12, 0.12, st["accent"])
            self.text(s, W - mx - 0.4, y, 0.4, 0.3, str(self.n), 10, st["text"], st["body"], True, align="r",
                      fitting=False)
            self.logo_at(s, W - mx - 0.8, y - 0.02, 0.28)
        elif f == "plain":
            self.text(s, W - mx - 1, y, 1, 0.3, str(self.n), 12, st["accent"], st["body"], True, align="r",
                      fitting=False)
        elif f == "minimal":
            self.text(s, W - mx - 1, y, 1, 0.3, str(self.n), 9, c, st["lab"], align="r", fitting=False)

    # -- bullets and small blocks ------------------------------------------------------------------------------
    def items(self, s, x, y, w, h, items, size=None, color=None):
        if self.st.get("center"):
            return self.items_centered(s, x, y, w, h, items, size, color)
        """A list with the style's markers, one row per item (≤ 6)."""
        st, b = self.st, self.fx["bullets"]
        items = [str(i) for i in (items or [])][:6]
        if not items:
            return
        size = size or st["body_size"]
        ind = 0.62 if b in ("numbers", "block", "geo") else 0.42
        # one size for all rows: the longest must fit its share of the height
        share = h / len(items)
        size = min(fit(fr(it), w - ind, share * 0.82, size, st["body"], spacing=1.05) for it in items)
        need = [(lines_needed(fr(it), w - ind, size, st["body"]) or 3) * size * 1.3 / 72 + size / 72 * 0.9
                for it in items]
        gap = min(0.35, max(0.08, (h - sum(need)) / max(1, len(items))))
        cy = y
        for i, it in enumerate(items):
            rh = need[i]
            mk = size / 72
            col = color or st["palette"][i % len(st["palette"])]
            acc = color or st["accent"]
            if b == "numbers":
                d = mk * 1.5
                self.rect(s, x, cy - 0.02, d, d, col, shape=MSO_SHAPE.OVAL)
                self.text(s, x, cy - 0.02, d, d, str(i + 1), size * 0.8, "FFFFFF", st["body"], True, align="c",
                          anchor="m", fitting=False)
            elif b == "block":
                self.brutal(s, x, cy + mk * 0.1, mk * 1.1, mk * 1.1, col, 0.04, 1.25)
            elif b == "geo" and color:
                self.rect(s, x, cy + mk * 0.15, mk * 0.95, mk * 0.95, color, shape=MSO_SHAPE.OVAL)
            elif b == "geo":
                self.geo_glyph(s, x, cy + mk * 0.15, mk * 0.95, i)
            elif b == "diamond":
                self.rect(s, x + 0.02, cy + mk * 0.4, mk * 0.5, mk * 0.5, color or st["accent2"], shape=MSO_SHAPE.DIAMOND)
            elif b == "square":
                self.rect(s, x, cy + mk * 0.45, mk * 0.38, mk * 0.38, acc)
            elif b == "chevron":
                self.text(s, x, cy, 0.4, mk * 1.3, ">", size, acc, "Courier New", True, fitting=False)
            elif b == "tick":
                self.hline(s, x, cy + mk * 0.62, 0.22, acc, 1)
            else:  # dash
                self.hline(s, x, cy + mk * 0.62, 0.24, acc, 2)
            self.text(s, x + ind, cy, w - ind, rh, it, size, color, st["body"], spacing=1.05, fitting=False)
            cy += rh + gap

    def items_centered(self, s, x, y, w, h, items, size=None, color=None):
        """Centred lines separated by short hairlines (styles with a centred axis)."""
        st = self.st
        items = [str(i) for i in (items or [])][:6]
        if not items:
            return
        size = size or st["body_size"]
        share = h / len(items)
        size = min(fit(fr(it), w, share * 0.7, size, st["body"], spacing=1.05) for it in items)
        for i, it in enumerate(items):
            cy = y + i * share
            if i:
                self.hline(s, x + w / 2 - 0.15, cy - 0.02, 0.3, st["accent"], 0.75)
            self.text(s, x, cy + 0.12, w, share - 0.15, it, size, color, st["body"], align="c", fitting=False)

    def panel(self, s, x, y, w, h, i=0):
        """The style's card / column container; returns the inner box (x, y, w, h)."""
        st, c = self.st, self.fx["card"]
        p = 0.35
        if c == "outline":
            self.rect(s, x, y, w, h, st["alt"], st["line"], 1)
        elif c == "brutal":
            self.brutal(s, x, y, w, h, [st["alt"], st["accent3"], "FFFFFF", st["accent"]][i % 4])
        elif c == "tile":
            self.rect(s, x, y, w, h, st["alt"])
            self.hline(s, x, y, w, st["accent"] if i % 2 == 0 else st["accent2"], 4)
        elif c == "tab":
            self.rect(s, x, y, w, h, st["alt"])
        elif c == "double":
            self.rect(s, x, y, w, h, None, st["line"], 1.25)
            self.rect(s, x + 0.07, y + 0.07, w - 0.14, h - 0.14, None, st["line"], 0.5)
            p = 0.4
        elif c == "solid":
            self.rect(s, x, y, w, h, [st["accent2"], st["accent"], st["text"], st["accent3"]][i % 4])
        elif c == "toprule":
            self.hline(s, x, y, w, st["line"], 3)
            p = 0.0
        else:   # rules, hairline: no box
            p = 0.0
        return x + p, y + (p or 0.25), w - 2 * p, h - (p or 0.25) - p

    def panel_text(self, i):
        c = self.fx["card"]
        if c == "solid":
            return "FFFFFF" if i % 4 != 3 else self.st["text"]
        return None

    # -- layouts -----------------------------------------------------------------------------------------------
    def cover(self, d):
        st, W, H, mx, v = self.st, self.W, self.H, self.mx, self.fx["cover"]
        img = d.get("image")
        title, sub, kick = d.get("title", ""), d.get("subtitle", ""), d.get("kicker", "")
        if v == "bleed" and img:
            s = self.new(decor=False)
            self.picture(s, img, 0, 0, W, H)
            self.gradient(s, 0, H * 0.35, W, H * 0.65, "000000", 0, 78)
            if kick:
                self.hline(s, mx, H - 3.35, 0.8, "FFFFFF", 1)
                self.label(s, mx, H - 3.2, 8, kick, "FFFFFF", 12)
            self.head(s, mx, H - 2.85, W * 0.72, 1.7, title, 66, "FFFFFF", anchor="b")
            if sub:
                self.text(s, mx, H - 1.05, W * 0.7, 0.6, sub, 20, "EEEEEE", st["body"], italic=st["body"] == "Georgia")
        elif v in ("bleed", "type"):
            s = self.new()
            self.hline(s, mx, 0.6, W - 2 * mx, st["line"], 0.75)
            self.label(s, mx, 0.75, 8, kick or "", st["accent"], 12)
            self.head(s, mx, 1.4, W * 0.8, 3.6, title, 80, anchor="b", spacing=0.9)
            self.hline(s, mx, 5.3, W - 2 * mx, st["line"], 0.75)
            self.text(s, mx, 5.5, W * 0.6, 1.0, sub, 22, st["muted"], st["body"], italic=st["body"] == "Georgia")
        elif v == "poster":
            s = self.new()
            col = (W - 2 * mx) / 12
            tw = col * (7 if img else 11)
            if img:
                self.picture(s, img, mx + col * 7.5, 0, W - mx - col * 7.5, H)
            self.rect(s, mx, 0.6, 0.9, 0.9, st["accent"])
            if kick:
                self.text(s, mx + col * 3, 0.62, col * 4, 0.6, kick, 13, st["text"], st["body"], True)
            self.head(s, mx, 1.9, tw, 3.9, title, 88, anchor="b", spacing=0.88)
            self.text(s, mx, 6.0, tw, 0.9, sub, 20, st["muted"], st["body"])
        elif v == "window":
            s = self.new()
            x, y, w, h = mx, 0.7, W - 2 * mx, H - 1.4
            self.rect(s, x, y, w, h, st["alt"], st["line"], 1)
            self.rect(s, x, y, w, 0.42, mix(st["alt"], "000000", 0.3), st["line"], 1)
            for i, c in enumerate(("FF5F56", "FFBD2E", "27C93F")):
                self.rect(s, x + 0.25 + i * 0.28, y + 0.13, 0.16, 0.16, c, shape=MSO_SHAPE.OVAL)
            tw = w * (0.55 if img else 0.9)
            if img:
                self.picture(s, img, x + w * 0.6, y + 0.42, w * 0.4, h - 0.42)
            self.text(s, x + 0.5, y + 0.85, tw, 0.4, "$ " + (kick or "present --deck"), 15, st["accent"],
                      "Courier New")
            self.head(s, x + 0.5, y + 1.4, tw - 0.3, 2.6, title, 58, anchor="t")
            self.text(s, x + 0.5, y + h - 1.35, tw - 0.3, 0.9, sub, 18, st["muted"], "Courier New")
            self.rect(s, x + 0.5, y + h - 0.45, 0.16, 0.28, st["accent"])
        elif v == "blocks":
            s = self.new()
            tw = W * (0.58 if img else 0.86)
            self.brutal(s, mx, 1.2, tw, H - 2.4, st["accent"], 0.14, 3)
            if kick:
                self.brutal(s, mx + 0.4, 0.85, min(tw - 0.8, 0.7 + len(kick) * 0.12), 0.5, st["accent3"], 0.06, 2,
                            rot=-2)
                self.text(s, mx + 0.4, 0.85, min(tw - 0.8, 0.7 + len(kick) * 0.12), 0.5, kick, 15, st["text"],
                          st["lab"], True, align="c", anchor="m", upper=True, track=1, rot=-2)
            self.head(s, mx + 0.45, 1.75, tw - 0.9, 2.8, title, 64, st["on_accent"], anchor="b", spacing=0.92)
            self.text(s, mx + 0.45, 4.75, tw - 0.9, 1.0, sub, 20, st["on_accent"], st["body"])
            if img:
                self.framed(s, img, W * 0.64, 1.0, W * 0.36 - mx, H - 2.3, rot=2.5)
            else:
                self.brutal(s, W - mx - 1.6, H - 2.2, 1.4, 1.4, st["accent2"], 0.1, 2.5, shape=MSO_SHAPE.OVAL)
        elif v == "panel":
            s = self.new(decor=False)
            if img:
                self.picture(s, img, 0, 0, W, H)
            pw, ph = 6.6, 3.5
            px, py = (W - pw) / 2, (H - ph) / 2
            self.rect(s, px, py, pw, ph, st["bg"])
            self.rect(s, px + 0.15, py + 0.15, pw - 0.3, ph - 0.3, None, st["accent"], 0.75)
            if not img:
                self.rect(s, W / 2 - 0.45, py - 1.25, 0.9, 0.9, None, st["accent"], 0.75, shape=MSO_SHAPE.OVAL)
                self.text(s, W / 2 - 0.45, py - 1.25, 0.9, 0.9, (title or "M")[0], 30, st["accent"], st["head"],
                          align="c", anchor="m", fitting=False)
            self.label(s, px + 0.4, py + 0.5, pw - 0.8, kick or "", st["muted"], 10, "c")
            self.head(s, px + 0.5, py + 0.9, pw - 1.0, 1.3, title, 34, align="c", anchor="m", spacing=1.05)
            self.hline(s, W / 2 - 0.5, py + 2.35, 1.0, st["accent"], 0.75)
            self.text(s, px + 0.5, py + 2.5, pw - 1.0, 0.7, sub, 15, st["muted"], st["body"], italic=True, align="c")
        elif v == "panel_left":
            s = self.new()
            pw = W * 0.45
            self.rect(s, 0, 0, pw, H, st["accent"])
            if img:
                self.picture(s, img, pw, 0, W - pw, H)
            else:
                for i in range(7):     # a small rising-bars motif
                    bh = 0.5 + i * 0.42
                    self.rect(s, pw + 1.0 + i * 0.75, H - 1.2 - bh, 0.42, bh,
                              st["accent2"] if i == 6 else mix(st["accent"], "FFFFFF", 0.82))
            self.label(s, mx, 1.0, pw - 2 * mx, kick or "", st["accent2"] if not img else "FFFFFF", 12)
            self.head(s, mx, 1.5, pw - 1.3 * mx, 3.4, title, 46, "FFFFFF", anchor="b", spacing=0.95)
            self.hline(s, mx, 5.15, 0.8, st["accent2"], 3)
            self.text(s, mx, 5.4, pw - 1.4 * mx, 1.2, sub, 18, "FFFFFF", st["body"])
            self.logo_at(s, W - mx, 0.5, 0.6)
        elif v == "notebook":
            s = self.new()
            tw = W * (0.5 if img else 0.85)
            if kick:
                kw = 0.5 + len(kick) * 0.13
                self.rect(s, mx + 0.3, 1.1, kw, 0.5, st["accent"])
                self.text(s, mx + 0.3, 1.1, kw, 0.5, kick, 15, "FFFFFF", st["body"], True, align="c", anchor="m")
            size = fit(title, tw, 2.6, 54, st["head"], True)
            n = lines_needed(fr(title), tw, size, st["head"], True) or 2
            for k in range(n):
                self.rect(s, mx + 0.2, 2.0 + k * size * 1.2 / 72 + size / 72 * 0.5, tw * (0.95 if k < n - 1 else 0.7),
                          size / 72 * 0.62, st["accent3"], rot=-0.6)
            self.text(s, mx + 0.3, 2.0, tw, 2.8, title, size, st["text"], st["head"], True, fitting=False)
            self.text(s, mx + 0.3, 2.2 + n * size * 1.2 / 72, tw, 1.2, sub, 22, st["muted"], st["body"])
            if img:
                x, y, w, h = W * 0.6, 1.2, W * 0.4 - mx, 4.6
                self.rect(s, x - 0.12, y - 0.12, w + 0.24, h + 0.24, "FFFFFF", "D0D7E2", 0.75, rot=2)
                p = self.picture(s, img, x, y, w, h)
                if p is not None:
                    p.rotation = 2
                for tx, ty in ((x + 0.2, y - 0.3), (x + w - 1.2, y + h - 0.1)):
                    t = self.rect(s, tx, ty, 1.0, 0.36, st["accent3"], rot=-8)
                    self.alpha(t, 70)
        elif v == "badge":
            s = self.new()
            tx, tw = (W * 0.5 + 0.4, W * 0.5 - 0.4 - mx) if img else (mx, W - 2 * mx)
            if img:
                self.picture(s, img, 0.3, 0.3, W * 0.5 - 0.3, H - 0.6)
            cx = tx + tw / 2
            self.rect(s, cx - 0.75, 0.75, 1.5, 1.5, None, st["accent2"], 1.5, shape=MSO_SHAPE.OVAL)
            self.rect(s, cx - 0.66, 0.84, 1.32, 1.32, None, st["accent2"], 0.5, shape=MSO_SHAPE.OVAL)
            self.text(s, cx - 0.6, 1.1, 1.2, 0.8, kick or (title or "·")[0], 10 if kick else 30, st["accent2"],
                      st["lab"] if kick else st["head"], True, align="c", anchor="m", upper=bool(kick), track=1 if kick else 0)
            self.head(s, tx + 0.2, 2.5, tw - 0.4, 2.2, title, 50, align="c", anchor="m")
            for k, yy in enumerate((4.85, 4.9)):
                self.hline(s, cx - 1.2, yy, 2.4, st["line"], 0.75)
            self.rect(s, cx - 0.1, 4.78, 0.2, 0.2, st["accent2"], shape=MSO_SHAPE.DIAMOND)
            self.text(s, tx + 0.2, 5.15, tw - 0.4, 1.1, sub, 19, st["muted"], st["body"], italic=True, align="c")
        elif v == "geo":
            s = self.new(decor=False)
            cx = W * 0.62
            self.rect(s, cx + 1.3, 3.4, 4.2, 4.1, st["accent2"])
            if img:
                self.picture(s, img, cx + 0.2, 0.6, 3.9, 3.9, circle=True)
            else:
                self.rect(s, cx + 0.2, 0.6, 3.9, 3.9, st["accent"], shape=MSO_SHAPE.OVAL)
            self.rect(s, cx + 3.1, 0.9, 1.3, 1.3, st["accent3"])
            self.rect(s, cx - 0.3, 5.4, 3.0, 0.32, st["text"])
            self.rect(s, cx + 0.2, 4.2, 1.6, 1.4, st["accent3"], shape=MSO_SHAPE.RIGHT_TRIANGLE)
            self.label(s, mx, 1.0, 6, kick or "", st["accent"], 12)
            self.head(s, mx, 1.5, cx - mx - 0.4, 3.8, title, 66, anchor="b", spacing=0.9)
            self.text(s, mx, 5.5, cx - mx - 0.8, 1.0, sub, 20, st["muted"], st["body"])
        else:
            s = self.new()
            self.head(s, mx, 2, W - 2 * mx, 2, title, 60)
        if v not in ("panel_left",):
            self.logo_at(s, W - mx, 0.45, 0.55)
        return s

    def section(self, d):
        st, W, H, mx, v = self.st, self.W, self.H, self.mx, self.fx["section"]
        self.sec_i += 1
        i = self.sec_i
        title, sub, img = d.get("title", ""), d.get("subtitle", ""), d.get("image")
        if v == "numeral":
            s = self.new()
            self.text(s, mx, 1.0, 5, 5, f"{i:02d}", 230, st["accent"], st["head"], fitting=False, anchor="m")
            self.hline(s, 5.6, 2.6, W - 5.6 - mx, st["line"], 0.75)
            self.head(s, 5.6, 2.8, W - 5.6 - mx, 2.2, title, 52)
            self.text(s, 5.6, 5.0, W - 5.6 - mx, 1.2, sub, 20, st["muted"], st["body"], italic=True)
        elif v == "flood":
            s = self.new(st["accent"])
            self.text(s, mx, 0.6, 3, 0.6, f"{i:02d}", 28, st["on_accent"], st["head"], True, fitting=False)
            self.head(s, mx, 2.0, W - 2 * mx, 3.4, title, 76, st["on_accent"], anchor="b", spacing=0.9)
            self.text(s, mx, 5.65, W * 0.6, 1.0, sub, 20, st["on_accent"], st["body"])
        elif v == "prompt":
            s = self.new()
            self.text(s, mx, 1.3, 6, 1.8, f"{i:02d}/", 110, st["accent"], "Courier New", True, fitting=False)
            self.head(s, mx, 3.25, W - 2 * mx, 1.6, title, 52)
            self.text(s, mx, 4.95, W * 0.7, 1.0, ("# " + sub) if sub else "", 18, st["muted"], "Courier New")
            self.rect(s, mx, 6.1, 0.22, 0.38, st["accent"])
        elif v == "badge":
            s = self.new(st["accent2"])
            self.brutal(s, mx, 1.4, 4.2, 4.2, st["accent3"], 0.16, 3, shape=MSO_SHAPE.OVAL)
            self.text(s, mx, 1.4, 4.2, 4.2, str(i), 150, st["text"], st["head"], True, align="c", anchor="m",
                      fitting=False)
            self.head(s, mx + 5.0, 1.8, W - mx * 2 - 5.0, 2.8, title, 60, "FFFFFF", anchor="b")
            self.text(s, mx + 5.0, 4.8, W - mx * 2 - 5.0, 1.2, sub, 22, "FFFFFF", st["body"])
        elif v == "roman":
            s = self.new()
            self.text(s, mx, 1.4, W - 2 * mx, 1.0, ROMAN[(i - 1) % 12], 54, st["accent"], st["head"], align="c",
                      fitting=False)
            self.hline(s, W / 2 - 0.6, 2.75, 1.2, st["accent"], 0.75)
            self.head(s, mx, 3.05, W - 2 * mx, 1.5, title, 40, align="c", anchor="t", spacing=1.1)
            self.text(s, mx + 1.5, 4.7, W - 2 * mx - 3, 1.0, sub, 18, st["muted"], st["body"], italic=True, align="c")
        elif v == "tracker":
            s = self.new(st["accent"])
            self.text(s, mx, 1.2, 3, 1.2, f"{i:02d}", 72, st["accent2"], st["head"], True, fitting=False)
            self.head(s, mx, 2.6, W * 0.5 - mx, 2.6, title, 44, "FFFFFF")
            self.text(s, mx, 5.3, W * 0.5 - mx, 1.2, sub, 18, mix(st["accent"], "FFFFFF", 0.7), st["body"])
            x0 = W * 0.58
            n = max(1, len(self.sections))
            rh = min(0.7, (H - 2.4) / n)
            for k, name in enumerate(self.sections):
                cur = k + 1 == i
                y = 1.2 + k * rh
                self.hline(s, x0, y, W - mx - x0, mix(st["accent"], "FFFFFF", 0.35), 0.5)
                self.text(s, x0, y + 0.1, 0.6, rh - 0.1, f"{k + 1:02d}", 14, st["accent2"] if cur else
                          mix(st["accent"], "FFFFFF", 0.5), st["body"], True, fitting=False)
                self.text(s, x0 + 0.65, y + 0.1, W - mx - x0 - 0.65, rh - 0.1, name, 16,
                          "FFFFFF" if cur else mix(st["accent"], "FFFFFF", 0.55), st["body"], cur)
        elif v == "chapter":
            col = st["palette"][(i - 1) % len(st["palette"])]
            s = self.new(col)
            self.rect(s, mx, 1.6, 2.6, 2.6, "FFFFFF", shape=MSO_SHAPE.OVAL)
            self.text(s, mx, 1.6, 2.6, 2.6, str(i), 96, col, st["head"], True, align="c", anchor="m", fitting=False)
            self.text(s, mx + 3.2, 1.7, 6, 0.5, f"Partie {i}", 20, "FFFFFF", st["body"], True)
            self.head(s, mx + 3.2, 2.3, W - 2 * mx - 3.2, 2.2, title, 48, "FFFFFF")
            self.text(s, mx + 3.2, 4.6, W - 2 * mx - 3.2, 1.2, sub, 22, "FFFFFF", st["body"])
        elif v == "day":
            s = self.new()
            tw = W * 0.42 if img else W - 2 * mx
            if img:
                self.picture(s, img, W * 0.5, 0, W * 0.5, H)
            self.label(s, mx, 2.0, tw, f"— {i:02d} —", st["accent"], 14)
            self.head(s, mx, 2.5, tw, 2.4, title, 64, anchor="t")
            self.text(s, mx, 5.0, tw, 1.2, sub, 20, st["muted"], st["body"], italic=True)
        elif v == "badge_line":
            s = self.new()
            self.text(s, mx, 1.5, W - 2 * mx, 0.6, f"N° {i:02d}", 22, st["accent2"], st["head"], italic=True,
                      align="c", fitting=False)
            for yy in (2.3, 2.36):
                self.hline(s, W / 2 - 1.5, yy, 3.0, st["line"], 0.75)
            self.head(s, mx + 0.5, 2.6, W - 2 * mx - 1, 2.0, title, 54, align="c", anchor="m")
            for yy in (4.8, 4.86):
                self.hline(s, W / 2 - 1.5, yy, 3.0, st["line"], 0.75)
            self.rect(s, W / 2 - 0.12, 4.71, 0.24, 0.24, st["accent2"], shape=MSO_SHAPE.DIAMOND)
            self.text(s, mx + 1, 5.15, W - 2 * mx - 2, 1.0, sub, 19, st["muted"], st["body"], italic=True, align="c")
        elif v == "geo":
            s = self.new(st["accent3"])
            self.rect(s, mx, 1.5, 4.0, 4.0, st["text"], shape=MSO_SHAPE.OVAL)
            self.text(s, mx, 1.5, 4.0, 4.0, str(i), 140, st["accent3"], st["head"], True, align="c", anchor="m",
                      fitting=False)
            self.rect(s, mx + 3.0, 4.6, 1.5, 1.5, st["accent"])
            self.head(s, mx + 5.0, 1.8, W - 2 * mx - 5.0, 3.0, title, 60, st["text"], anchor="b")
            self.text(s, mx + 5.0, 5.0, W - 2 * mx - 5.0, 1.2, sub, 20, st["text"], st["body"])
        else:
            s = self.new(st["accent"])
            self.head(s, mx, 2.5, W - 2 * mx, 2, title, 54, st["on_accent"])
        return s

    def agenda(self, d):
        st, W, H, mx = self.st, self.W, self.H, self.mx
        s = self.new()
        y0 = self.title(s, d.get("title", "Sommaire"), d.get("kicker"))
        items = d.get("items") or self.sections or []
        n = max(1, len(items[:8]))
        rh = min(1.05, (H - y0 - 0.9) / n)
        for k, it in enumerate(items[:8]):
            y = y0 + k * rh
            self.hline(s, mx, y, W - 2 * mx, st["line"] if self.fx["card"] != "outline" else st["muted"], 0.5)
            num = ROMAN[k] if self.fx["section"] == "roman" else f"{k + 1:02d}"
            self.text(s, mx, y + 0.12, 1.2, rh - 0.15, num, 28, st["accent"], st["head"] if st["lab"] != "Courier New"
                      else "Courier New", True, fitting=False)
            self.text(s, mx + 1.3, y + 0.12, W - 2 * mx - 1.3, rh - 0.15, it, 30, None, st["head"],
                      st.get("head_bold"), upper=st.get("head_upper"), track=st.get("head_track", 0))
        self.footer(s)
        return s

    def bullets(self, d):
        st, W, H, mx = self.st, self.W, self.H, self.mx
        s = self.new()
        img = d.get("image")
        y0 = self.title(s, d.get("title", ""), d.get("kicker"))
        bw = W * 0.5 - mx if img else W - 2 * mx - 1.0
        if img:
            if self.fx["image"] in ("frame", "brutal"):
                self.framed(s, img, W * 0.56, y0, W * 0.44 - mx, H - y0 - 0.9)
            else:
                self.picture(s, img, W * 0.56, 0 if self.fx["title"] in ("rule",) else y0, W * 0.44,
                             H if self.fx["title"] in ("rule",) else H - y0 - 0.75)
        self.items(s, mx, y0 + 0.1, bw, H - y0 - 1.0, d.get("bullets", []))
        self.footer(s)
        return s

    def two_columns(self, d):
        st, W, H, mx = self.st, self.W, self.H, self.mx
        s = self.new()
        y0 = self.title(s, d.get("title", ""), d.get("kicker"))
        gap = 0.5
        cw = (W - 2 * mx - gap) / 2
        ch = H - y0 - 0.85
        for i, key in enumerate(("left", "right")):
            col = d.get(key) or {}
            x = mx + i * (cw + gap)
            ix, iy, iw, ih = self.panel(s, x, y0, cw, ch, i)
            tc = self.panel_text(i)
            if self.fx["card"] in ("rules", "hairline") and i == 1:
                self.vline(s, mx + cw + gap / 2, y0, ch, st["line"] if self.fx["card"] == "rules" else st["accent"], 0.5)
            hd = col.get("heading", "")
            if self.fx["card"] == "tab":
                c = st["palette"][i % len(st["palette"])]
                self.rect(s, x, y0, cw, 0.62, c)
                self.text(s, x + 0.3, y0, cw - 0.6, 0.62, hd, 22, "FFFFFF", st["head"], True, anchor="m")
                iy, ih = y0 + 0.9, ch - 1.1
                ix, iw = x + 0.3, cw - 0.6
            elif self.fx["card"] == "outline":
                self.text(s, ix, iy, iw, 0.5, f"[ {hd} ]", 18, st["accent"] if i == 0 else st["accent3"],
                          "Courier New", True)
                iy, ih = iy + 0.7, ih - 0.7
            else:
                self.text(s, ix, iy, iw, 0.6, hd, 24, tc or (st["accent"] if self.fx["card"] in ("rules", "hairline")
                          else None), st["head"], st.get("head_bold", False) or self.fx["card"] != "hairline",
                          align="c" if self.fx["card"] == "hairline" else "l", upper=st.get("head_upper"),
                          track=st.get("head_track", 0) / 2)
                iy, ih = iy + 0.8, ih - 0.8
            if col.get("bullets"):
                self.items(s, ix, iy, iw, ih, col["bullets"], st["body_size"] - 1, tc)
            else:
                self.text(s, ix, iy, iw, ih, col.get("text", ""), st["body_size"], tc, st["body"], spacing=1.15,
                          align="c" if self.fx["card"] == "hairline" else "l")
        self.footer(s)
        return s

    def big_number(self, d):
        st, W, H, mx = self.st, self.W, self.H, self.mx
        s = self.new()
        val = str(d.get("value", ""))
        mono = st["lab"] == "Courier New"
        f = "Courier New" if mono else st["head"]
        c = st["center"] if "center" in st else False
        al = "c" if c else "l"
        if self.fx["stats"] == "brutal":
            self.brutal(s, mx, 0.9, W * 0.62, 3.6, st["accent3"], 0.14, 3)
            self.text(s, mx + 0.3, 1.0, W * 0.62 - 0.6, 3.4, val, 200, st["text"], f, True, anchor="m", minimum=40)
        elif self.fx["stats"] == "circles":
            self.rect(s, mx, 0.8, 4.2, 4.2, st["accent"], shape=MSO_SHAPE.OVAL)
            self.text(s, mx + 0.2, 0.8, 3.8, 4.2, val, 120, "FFFFFF", f, True, align="c", anchor="m", minimum=30)
        elif self.fx["stats"] == "solid":
            self.rect(s, mx, 0.8, W * 0.55, 3.8, st["accent"])
            self.text(s, mx + 0.3, 0.8, W * 0.55 - 0.6, 3.8, val, 200, "FFFFFF", f, True, anchor="m", minimum=40)
        else:
            self.text(s, mx, 0.7, W - 2 * mx, 3.9, val, 220, st["accent"], f, st.get("head_bold", True) or True,
                      align=al, anchor="b", minimum=40, spacing=0.9)
        side = self.fx["stats"] in ("circles",)
        x = mx + 4.8 if side else mx
        w = W - mx - x
        self.hline(s, x if not c else W / 2 - 0.6, 4.95, 1.2 if c else 1.0, st["accent"], 1.5)
        self.text(s, x, 5.15, w if not side else w, 0.8, d.get("label", ""), 30, None, st["head"],
                  st.get("head_bold"), align=al, upper=st.get("head_upper"), track=st.get("head_track", 0) / 2)
        self.text(s, x, 6.0, w * (1 if c else 0.75), 0.9, d.get("context", ""), 17, st["muted"], st["body"], align=al)
        self.footer(s)
        return s

    def stats(self, d):
        st, W, H, mx, v = self.st, self.W, self.H, self.mx, self.fx["stats"]
        s = self.new()
        y0 = self.title(s, d.get("title", ""), d.get("kicker"))
        items = (d.get("stats") or [])[:4] or [{"value": "", "label": ""}]
        n = len(items)
        gap = 0.4 if v not in ("rules", "hairline", "huge") else 0.5
        cw = (W - 2 * mx - gap * (n - 1)) / n
        ch = min(3.6, H - y0 - (1.75 if d.get("note") else 1.0))
        mono = st["lab"] == "Courier New"
        vf = "Courier New" if mono else st["head"]
        for i, it in enumerate(items):
            x = mx + i * (cw + gap)
            col = st["palette"][i % len(st["palette"])]
            val, lab = str(it.get("value", "")), str(it.get("label", ""))
            delta = str(it.get("delta") or "")
            if v in ("circles", "stamps"):
                dsz = min(cw - 0.3, ch - 1.3, 2.9)
                cx = x + (cw - dsz) / 2
                if v == "circles":
                    self.rect(s, cx, y0, dsz, dsz, col, shape=MSO_SHAPE.OVAL)
                    tcol = "FFFFFF"
                else:
                    self.rect(s, cx, y0, dsz, dsz, None, st["accent2"], 1.5, shape=MSO_SHAPE.OVAL)
                    self.rect(s, cx + 0.09, y0 + 0.09, dsz - 0.18, dsz - 0.18, None, st["accent2"], 0.5,
                              shape=MSO_SHAPE.OVAL)
                    tcol = st["accent"]
                self.text(s, cx + 0.2, y0, dsz - 0.4, dsz, val, 54, tcol, vf, True, align="c", anchor="m", minimum=14)
                self.text(s, x, y0 + dsz + 0.2, cw, ch - dsz - 0.2, lab, st["body_size"] - 2, None, st["body"], align="c")
                continue
            ix, iy, iw, ih = x, y0, cw, ch
            tcol = None
            if v == "brutal":
                self.brutal(s, x, y0, cw, ch, [st["accent3"], "FFFFFF", st["accent"], st["accent2"]][i % 4])
                tcol = "FFFFFF" if i % 4 == 3 else st["text"]
                ix, iy, iw, ih = x + 0.35, y0 + 0.3, cw - 0.7, ch - 0.5
            elif v == "kpi":
                self.rect(s, x, y0, cw, ch, st["alt"])
                self.hline(s, x, y0, cw, st["accent"], 4)
                ix, iy, iw, ih = x + 0.3, y0 + 0.35, cw - 0.6, ch - 0.5
            elif v == "mono":
                self.rect(s, x, y0, cw, ch, st["alt"], st["line"], 1)
                ix, iy, iw, ih = x + 0.3, y0 + 0.3, cw - 0.6, ch - 0.5
            elif v == "solid":
                self.rect(s, x, y0, cw, ch, [st["accent"], st["accent2"], st["accent3"], st["text"]][i % 4])
                tcol = "FFFFFF" if i % 4 != 2 else st["text"]
                ix, iy, iw, ih = x + 0.3, y0 + 0.3, cw - 0.6, ch - 0.5
            elif v == "huge":
                self.hline(s, x, y0, cw, st["line"], 3)
                iy = y0 + 0.2
            elif v == "hairline":
                if i:
                    self.vline(s, x - gap / 2, y0 + 0.2, ch - 0.4, st["accent"], 0.5)
            else:   # rules
                self.hline(s, x, y0, cw, st["line"], 0.75)
                iy = y0 + 0.2
            al = "c" if v == "hairline" else "l"
            vcol = tcol or (st["text"] if v == "huge" and i else st["accent"] if v != "kpi" else st["accent"])
            vh = min(1.7, ih * 0.5)
            used = self.text(s, ix, iy, iw, vh, val, 96 if v == "huge" else 64, vcol, vf,
                             v not in ("hairline", "rules") or st.get("head_bold", False), align=al, anchor="b",
                             minimum=18, spacing=0.9)
            yy = iy + vh + 0.15
            if delta:
                good = not delta.strip().startswith(("-", "−"))
                dcol = st.get("good", "1E8E3E") if good else st.get("bad", "C0392B")
                if it.get("good") is not None:
                    dcol = st.get("good", "1E8E3E") if it["good"] else st.get("bad", "C0392B")
                arrow = "▲ " if good else "▼ "
                self.text(s, ix, yy, iw, 0.4, arrow + delta, 15, tcol or dcol, st["body"], True, align=al)
                yy += 0.5
            self.text(s, ix, yy, iw, iy + ih - yy, lab, st["body_size"] - 2, tcol, st["body"], align=al, spacing=1.1)
        if d.get("note"):
            self.text(s, mx, H - 1.45, W - 2 * mx, 0.6, d["note"], 16, st["muted"], st["body"],
                      italic=st["body"] == "Georgia", align="c" if st.get("center") else "l")
        self.footer(s)
        return s

    def chart(self, d):
        st, W, H, mx, v = self.st, self.W, self.H, self.mx, self.fx["chart"]
        s = self.new()
        y0 = self.title(s, d.get("title", ""), d.get("kicker"))
        c = d.get("chart") or {}
        side = d.get("takeaway")
        sw = 3.6 if side else 0
        cw = W - 2 * mx - sw - (0.3 if side else 0)
        kind = c.get("type", "bar")
        types = {"bar": XL_CHART_TYPE.COLUMN_CLUSTERED, "bar_h": XL_CHART_TYPE.BAR_CLUSTERED,
                 "line": XL_CHART_TYPE.LINE_MARKERS, "pie": XL_CHART_TYPE.PIE,
                 "doughnut": XL_CHART_TYPE.DOUGHNUT, "stacked": XL_CHART_TYPE.COLUMN_STACKED}
        data = CategoryChartData()
        cats = [str(x) for x in c.get("categories", [])]
        data.categories = cats
        series = c.get("series") or {}
        if isinstance(series, list):
            series = {x.get("name", f"S{i + 1}"): x.get("values", []) for i, x in enumerate(series)}
        series = {k: [num(x) for x in v] for k, v in series.items()}
        for name, values in series.items():
            data.add_series(str(name), values)
        ch_h = H - y0 - 0.75
        if not any(isinstance(v, (int, float)) and v != 0 for vals in series.values() for v in vals):
            self.rect(s, mx, y0, cw, ch_h, st["alt"], st["muted"], 1)
            self.text(s, mx + 0.5, y0, cw - 1, ch_h, "Graphique à compléter : " + ", ".join(series) +
                      (" (" + " · ".join(cats) + ")" if cats else ""), 20, st["muted"], st["body"], align="c", anchor="m")
            if side:
                self.takeaway(s, W - mx - sw, y0 + 0.1, sw, ch_h - 0.3, side)
            self.footer(s, d.get("source"))
            return s
        frame = s.shapes.add_chart(types.get(kind, XL_CHART_TYPE.COLUMN_CLUSTERED), Inches(mx), Inches(y0),
                                   Inches(cw), Inches(ch_h), data)
        ch = frame.chart
        big = v == "big"
        mono = st["lab"] == "Courier New"
        ch.font.size = Pt(18 if big else 15)
        ch.font.name = "Courier New" if mono else st["body"]
        ch.font.color.rgb = rgb(st["muted"])
        pal = st["palette"]
        plot = ch.plots[0]
        outline = v == "bold"
        unit = c.get("unit")
        if kind in ("pie", "doughnut"):
            ch.has_legend = True
            ch.legend.position = XL_LEGEND_POSITION.RIGHT
            ch.legend.include_in_layout = False
            ch.legend.font.size = Pt(15 if big else 13)
            ch.legend.font.color.rgb = rgb(st["text"])
            for i, pt in enumerate(plot.series[0].points):
                pt.format.fill.solid()
                pt.format.fill.fore_color.rgb = rgb(pal[i % len(pal)])
                pt.format.line.color.rgb = rgb(st["line"] if outline else st["bg"])
                pt.format.line.width = Pt(2 if outline else 1.5)
            plot.has_data_labels = True
            dl = plot.data_labels
            dl.number_format, dl.number_format_is_linked = '0%', False
            dl.show_percentage, dl.show_value = True, False
            dl.font.size = Pt(14)
            dl.font.bold = True
            dl.font.color.rgb = rgb("FFFFFF")
            if kind == "doughnut":
                hole = plot._element.find(qn("c:holeSize"))
                if hole is None:
                    hole = etree.SubElement(plot._element, qn("c:holeSize"))
                hole.set("val", "62")
        else:
            ch.has_legend = len(series) > 1
            if ch.has_legend:
                ch.legend.position = XL_LEGEND_POSITION.TOP
                ch.legend.include_in_layout = False
                ch.legend.font.color.rgb = rgb(st["text"])
                ch.legend.font.size = Pt(16 if big else 14)
            if kind != "line":
                plot.gap_width = 55 if v != "highlight" else 40
                plot.overlap = -8 if kind != "stacked" else 100
            single = len(series) == 1
            hl = c.get("highlight")
            for i, ser in enumerate(plot.series):
                col = pal[i % len(pal)]
                if kind == "line":
                    ser.format.line.color.rgb = rgb(col)
                    ser.format.line.width = Pt(3.5 if outline else 2.75)
                    ser.smooth = False
                    ser.marker.style = XL_MARKER_STYLE.CIRCLE
                    ser.marker.size = 8
                    ser.marker.format.fill.solid()
                    ser.marker.format.fill.fore_color.rgb = rgb(col)
                    ser.marker.format.line.color.rgb = rgb(st["bg"])
                    continue
                ser.format.fill.solid()
                if v == "highlight" and single:
                    base = mix(st["text"], st["bg"], 0.72) if lum(st["bg"]) > 0.5 else mix(st["text"], st["bg"], 0.5)
                    ser.format.fill.fore_color.rgb = rgb(base)
                    vals = list(series.values())[0]
                    idx = cats.index(str(hl)) if hl is not None and str(hl) in cats else (
                        max(range(len(vals)), key=lambda k: vals[k] or 0) if vals else 0)
                    if vals:
                        p = ser.points[idx]
                        p.format.fill.solid()
                        p.format.fill.fore_color.rgb = rgb(st["accent"])
                elif v == "highlight" and i == len(series) - 1:
                    ser.format.fill.fore_color.rgb = rgb(st["accent"])
                elif v == "highlight":
                    ser.format.fill.fore_color.rgb = rgb(mix(st["text"], st["bg"], 0.7 - 0.25 * i))
                else:
                    ser.format.fill.fore_color.rgb = rgb(col)
                if outline:
                    ser.format.line.color.rgb = rgb(st["line"])
                    ser.format.line.width = Pt(1.75)
            va, ca = ch.value_axis, ch.category_axis
            va.has_major_gridlines = v not in ("highlight", "bold")
            if va.has_major_gridlines:
                va.major_gridlines.format.line.color.rgb = rgb(st["line"] if v == "dark" else mix(st["bg"], st["text"], 0.12))
                va.major_gridlines.format.line.width = Pt(0.75)
            va.format.line.fill.background()
            va.tick_labels.font.color.rgb = rgb(st["muted"])
            if v in ("highlight",) and single:
                va.visible = False
            ca.format.line.color.rgb = rgb(st["line"] if v != "dark" else st["muted"])
            ca.format.line.width = Pt(2.25 if outline else 1)
            ca.tick_labels.font.color.rgb = rgb(st["text"])
            ca.tick_labels.font.size = Pt(18 if big else 15)
            if unit:
                fmt = '#,##0" ' + unit.replace('"', '') + '"'
                va.tick_labels.number_format = fmt
                va.tick_labels.number_format_is_linked = False
            if (single or v == "highlight") and kind != "line" and len(cats) <= 12:
                plot.has_data_labels = True
                dl = plot.data_labels
                dl.font.size = Pt(17 if big else 15)
                dl.font.bold = v in ("highlight", "bold")
                dl.font.color.rgb = rgb(st["text"])
                dl.position = XL_LABEL_POSITION.OUTSIDE_END if kind != "stacked" else XL_LABEL_POSITION.CENTER
                if unit:
                    dl.number_format = '#,##0" ' + unit.replace('"', '') + '"'
                    dl.number_format_is_linked = False
                if not single and v == "highlight":
                    plot.has_data_labels = False
        if side:
            x = W - mx - sw
            self.takeaway(s, x, y0 + 0.1, sw, ch_h - 0.3, side)
        self.footer(s, d.get("source"))
        return s

    def takeaway(self, s, x, y, w, h, txt):
        st, c = self.st, self.fx["card"]
        if c == "brutal":
            self.brutal(s, x, y, w, h, st["accent3"])
            self.text(s, x + 0.3, y + 0.3, w - 0.6, h - 0.6, txt, 22, st["text"], st["head"], True, anchor="m")
        elif c == "solid":
            self.rect(s, x, y, w, h, st["accent2"])
            self.text(s, x + 0.3, y + 0.3, w - 0.6, h - 0.6, txt, 22, "FFFFFF", st["head"], True, anchor="m")
        elif c in ("tile", "tab", "outline"):
            self.rect(s, x, y, w, h, st["alt"], st["line"] if c == "outline" else None, 1)
            self.vline(s, x, y, h, st["accent"], 4)
            self.text(s, x + 0.35, y + 0.3, w - 0.6, h - 0.6, txt, 20, None,
                      "Courier New" if c == "outline" else st["body"], c != "outline", anchor="m", spacing=1.1)
        elif c == "double":
            self.panel(s, x, y, w, h)
            self.text(s, x + 0.4, y + 0.4, w - 0.8, h - 0.8, txt, 21, None, st["head"], True, align="c", anchor="m")
        else:
            self.hline(s, x, y + 0.2, w, st["accent"], 1.5 if c != "hairline" else 0.75)
            self.text(s, x, y + 0.45, w, h - 0.6, txt, 24, None, st["head"], st.get("head_bold", False),
                      italic=st["head"] in ("Georgia",) and not st.get("head_bold"), spacing=1.1)

    def cell_border(self, cell, sides, color, w_pt):
        tcPr = cell._tc.get_or_add_tcPr()
        for tag in ("a:lnL", "a:lnR", "a:lnT", "a:lnB"):
            for el in tcPr.findall(qn(tag)):
                tcPr.remove(el)
        for k, tag in enumerate(("a:lnL", "a:lnR", "a:lnT", "a:lnB")):
            side = tag[-1]
            ln = etree.Element(qn(tag), w=str(int(Pt(w_pt) if side in sides else 0)), cap="flat", cmpd="sng")
            if side in sides:
                f = etree.SubElement(ln, qn("a:solidFill"))
                etree.SubElement(f, qn("a:srgbClr"), val=color)
            else:
                etree.SubElement(ln, qn("a:noFill"))
            tcPr.insert(k, ln)

    def table(self, d):
        st, W, H, mx, v = self.st, self.W, self.H, self.mx, self.fx["table"]
        s = self.new()
        y0 = self.title(s, d.get("title", ""), d.get("kicker"))
        rows = [[str(c) for c in r] for r in (d.get("rows") or [[""]])]
        nr, nc = len(rows), max(len(r) for r in rows)
        avail = H - y0 - (1.4 if d.get("note") else 0.9)
        rh = min(0.72, avail / nr)
        size = max(10, min(20, int(rh * 72 * 0.42)))
        font = "Courier New" if v == "mono" else st["body"]
        shape = s.shapes.add_table(nr, nc, Inches(mx), Inches(y0), Inches(W - 2 * mx), Inches(rh * nr))
        tbl = shape.table
        tblPr = tbl._tbl.tblPr
        sid = tblPr.find(qn("a:tableStyleId"))
        if sid is None:
            sid = etree.SubElement(tblPr, qn("a:tableStyleId"))
        sid.text = "{2D5ABB26-0587-4C30-8999-92F81FD0307C}"
        tbl.first_row = tbl.horz_banding = False
        # first column wider when it holds words
        widths = [max(len(r[c]) if c < len(r) else 0 for r in rows) + 4 for c in range(nc)]
        tot = sum(widths)
        for c in range(nc):
            tbl.columns[c].width = Emu(int((W - 2 * mx) * 914400 * widths[c] / tot))
        for r in range(nr):
            tbl.rows[r].height = Inches(rh)
            total_row = r > 0 and rows[r] and rows[r][0].lower().startswith("total")
            for c in range(nc):
                cell = tbl.cell(r, c)
                txt = rows[r][c] if c < len(rows[r]) else ""
                cell.text = txt
                cell.margin_left = cell.margin_right = Inches(0.12)
                cell.margin_top = cell.margin_bottom = Inches(0.03)
                cell.vertical_anchor = MSO_ANCHOR.MIDDLE
                head = r == 0
                fill, tcol, bold = None, st["text"], head or total_row
                if v == "header" or (v == "zebra" and head):
                    fill, tcol = (st["accent"], "FFFFFF") if head else (None, st["text"])
                if v == "zebra" and not head and r % 2 == 0:
                    fill = st["alt"]
                if v == "brutal" and head:
                    fill = st["accent3"]
                if v == "mono" and head:
                    tcol = st["accent"]
                if v == "hairline" and head:
                    tcol = st["accent"]
                if total_row and v in ("header", "zebra"):
                    fill = st["alt"]
                if fill:
                    cell.fill.solid()
                    cell.fill.fore_color.rgb = rgb(fill)
                else:
                    cell.fill.background()
                num = c > 0 and any(ch.isdigit() for ch in txt) and len(txt) < 24
                for p in cell.text_frame.paragraphs:
                    p.alignment = PP_ALIGN.RIGHT if num else PP_ALIGN.LEFT
                    for run in p.runs:
                        run.font.size = Pt(size if not (head and v == "hairline") else size - 3)
                        run.font.name = font
                        run.font.bold = bold
                        run.font.color.rgb = rgb(tcol)
                        if head and v in ("hairline",):
                            run.text = run.text.upper()
                            run._r.get_or_add_rPr().set("spc", "150")
                if v == "brutal":
                    self.cell_border(cell, "LRTB", st["line"], 2)
                elif v == "mono":
                    self.cell_border(cell, "B", st["line"], 1)
                elif v == "double" and (head or total_row):
                    self.cell_border(cell, "TB" if head else "T", st["line"], 1.5 if head else 0.75)
                elif v == "double":
                    self.cell_border(cell, "B", mix(st["line"], st["bg"], 0.6), 0.5)
                elif v in ("rules", "hairline", "header"):
                    lc = st["accent"] if v == "hairline" else st["line"]
                    if head and v != "header":
                        self.cell_border(cell, "B", lc, 1.5 if v == "rules" else 0.75)
                    elif total_row:
                        self.cell_border(cell, "T", lc, 1.25)
                    else:
                        self.cell_border(cell, "B", mix(lc, st["bg"], 0.65), 0.5)
                else:
                    self.cell_border(cell, "", st["line"], 0)
        if d.get("note"):
            self.text(s, mx, H - 1.3, W - 2 * mx, 0.5, d["note"], 15, st["muted"], st["body"], italic=True)
        self.footer(s, d.get("source"))
        return s

    def image_full(self, d):
        st, W, H, mx, m = self.st, self.W, self.H, self.mx, self.fx["image"]
        title, cap = d.get("title", ""), d.get("caption", "")
        if m == "frame":
            s = self.new(decor=False)
            self.picture(s, d.get("image"), 0.45, 0.45, W - 0.9, H - 1.55)
            self.label(s, 0.45, H - 0.88, 4, title, st["accent"], 11)
            self.text(s, 4.6, H - 0.92, W - 5.05, 0.5, cap, 14, st["muted"], st["body"], italic=True, align="r")
            return s
        s = self.new(decor=False)
        self.picture(s, d.get("image"), 0, 0, W, H)
        if m == "gray":
            bw = min(W - 2 * mx, 0.9 + max(len(title), len(cap) * 0.55) * 0.24)
            self.rect(s, mx, H - 2.1, bw, 1.4, "FFFFFF")
            self.head(s, mx + 0.3, H - 1.95, bw - 0.6, 0.65, title, 30)
            self.text(s, mx + 0.3, H - 1.25, bw - 0.6, 0.4, cap, 14, st["muted"], st["body"])
            self.rect(s, mx, H - 2.1, 0.12, 1.4, st["accent"])
            return s
        if m == "brutal":
            tw = min(W - 2 * mx, 1.0 + len(title) * 0.3)
            self.brutal(s, mx, H - 1.9, tw, 0.9, st["accent3"], 0.1, 2.5, rot=-1.5)
            self.text(s, mx + 0.25, H - 1.9, tw - 0.5, 0.9, title, 32, st["text"], st["head"], True, anchor="m",
                      rot=-1.5)
            if cap:
                self.text(s, mx, H - 0.8, W * 0.7, 0.4, cap, 14, "FFFFFF", st["body"], True)
            return s
        self.gradient(s, 0, H * 0.5, W, H * 0.5, "000000", 0, 70)
        if m == "duotone":
            self.text(s, mx, H - 1.75, W - 2 * mx, 0.8, title, 32, "FFFFFF", st["head"], True, anchor="b")
            self.text(s, mx, H - 0.85, W - 2 * mx, 0.4, "# " + cap if cap else "", 13, st["accent"], "Courier New")
            return s
        self.label(s, mx, H - 2.0, 8, d.get("kicker") or "", st["accent"] if st["bg"] < "8" else "FFFFFF", 11)
        self.head(s, mx, H - 1.7, W * 0.75, 0.95, title, 40, "FFFFFF", anchor="b")
        self.text(s, mx, H - 0.68, W * 0.75, 0.4, cap, 15, "E8E8E8", st["body"], italic=st["body"] == "Georgia")
        return s

    def has_img(self, p):
        p = self.path(p) if p else None
        return bool(p and os.path.exists(p))

    def image_text(self, d):
        st, W, H, mx = self.st, self.W, self.H, self.mx
        if not self.has_img(d.get("image")):
            return self.statement(d)
        s = self.new()
        left = self.n % 2 == 0
        iw = W * 0.5
        framed = self.fx["image"] in ("frame", "brutal")
        if framed:
            ix = 0.75 if left else W * 0.5 + 0.1
            self.framed(s, d.get("image"), ix, 0.85, iw - 0.85, H - 1.9)
        else:
            self.picture(s, d.get("image"), 0 if left else W - iw, 0, iw, H)
        if d.get("caption"):
            cx = (0.75 if framed else mx) if left else W - iw + (0.1 if framed else mx)
            if framed:
                self.text(s, cx, H - 0.92, iw - 0.85, 0.35, d["caption"], 12, st["muted"], st["body"], italic=True)
            else:
                self.rect(s, (0 if left else W - iw), H - 0.55, iw, 0.55, "000000").fill.fore_color.rgb
                self.text(s, cx, H - 0.47, iw - 2 * mx, 0.35, d["caption"], 12, "FFFFFF", st["body"], italic=True)
        tx = (W * 0.5 + 0.6) if left else mx
        tw = W * 0.5 - 0.6 - mx
        y = 1.0
        if d.get("kicker"):
            self.label(s, tx, y, tw, d["kicker"], st["accent"], 11)
        self.head(s, tx, y + 0.4, tw, 1.9, d.get("title", ""), min(40, st["head_size"]), anchor="t")
        self.hline(s, tx, y + 2.45, 0.8, st["accent"], 2 if self.fx["card"] != "hairline" else 0.75)
        if d.get("bullets"):
            self.items(s, tx, y + 2.75, tw, H - y - 3.6, d["bullets"], st["body_size"] - 1)
        else:
            self.text(s, tx, y + 2.75, tw, H - y - 3.6, d.get("text", ""), st["body_size"], None, st["body"],
                      spacing=1.2)
        self.footer(s)
        return s

    def statement(self, d):
        """image_text without a picture: the title on a block of the style's colour, the text beside it."""
        st, W, H, mx = self.st, self.W, self.H, self.mx
        s = self.new()
        dark = lum(st["bg"]) < 0.2
        fill = st["alt"] if dark or self.fx["card"] in ("rules", "hairline", "double") else st["accent"]
        tc = st["text"] if fill == st["alt"] else st["on_accent"]
        bw = W * 0.42
        if self.fx["card"] == "brutal":
            self.brutal(s, mx, 0.9, bw - mx, H - 1.8, st["accent3"], 0.12, 2.5)
            tc = st["text"]
        elif self.fx["card"] == "double":
            self.panel(s, mx, 0.9, bw - mx, H - 1.8)
        else:
            self.rect(s, 0, 0, bw, H, fill)
        if d.get("kicker"):
            self.label(s, mx + 0.3, 1.4, bw - mx - 0.6, d["kicker"], tc if fill != st["alt"] else st["accent"], 11)
        self.head(s, mx + 0.3, 1.8, bw - mx - 0.7, 3.6, d.get("title", ""), 44, tc, anchor="t")
        tx, tw = bw + 0.7, W - bw - 0.7 - mx
        if d.get("bullets"):
            self.items(s, tx, 1.4, tw, H - 2.4, d["bullets"])
        else:
            self.text(s, tx, 1.4, tw, H - 2.4, d.get("text", ""), st["body_size"] + 2, None, st["body"],
                      anchor="m", spacing=1.25)
        self.footer(s)
        return s

    def gallery(self, d):
        st, W, H, mx = self.st, self.W, self.H, self.mx
        imgs = [im if isinstance(im, dict) else {"path": im} for im in (d.get("images") or [])][:4]
        s = self.new()
        y0 = self.title(s, d.get("title", ""), d.get("kicker"))
        n = max(1, len(imgs))
        capc = st["muted"]
        it = st["body"] == "Georgia"
        if self.fx["gallery"] == "mosaic" and n >= 3:
            gap = 0.12
            bw = (W - 2 * mx) * 0.6
            hh = H - y0 - 1.0
            self.picture(s, imgs[0].get("path"), mx, y0, bw, hh)
            k = n - 1
            sh = (hh - gap * (k - 1)) / k
            for j, im in enumerate(imgs[1:]):
                self.picture(s, im.get("path"), mx + bw + gap, y0 + j * (sh + gap), W - 2 * mx - bw - gap, sh)
            caps = " · ".join(f"{i + 1}. {im['caption']}" for i, im in enumerate(imgs) if im.get("caption"))
            self.text(s, mx, H - 0.88, W - 2 * mx - 1.2, 0.35, caps, 12, capc, st["body"], italic=it)
        else:
            gap = 0.3
            cw = (W - 2 * mx - gap * (n - 1)) / n
            ih = H - y0 - 1.55
            for i, im in enumerate(imgs):
                x = mx + i * (cw + gap)
                if self.fx["image"] in ("frame", "brutal"):
                    self.framed(s, im.get("path"), x + 0.12, y0 + 0.12, cw - 0.24, ih - 0.24,
                                rot=(-1.5 if i % 2 else 1.5) if self.fx["image"] == "brutal" else 0)
                else:
                    self.picture(s, im.get("path"), x, y0, cw, ih)
                if im.get("caption"):
                    self.text(s, x, y0 + ih + 0.15, cw, 0.5, im["caption"], 14, st["text"], st["body"],
                              italic=it, align="c" if st.get("center") else "l")
        self.footer(s)
        return s

    def timeline(self, d):
        st, W, H, mx, v = self.st, self.W, self.H, self.mx, self.fx["timeline"]
        s = self.new()
        y0 = self.title(s, d.get("title", ""), d.get("kicker"))
        steps = (d.get("steps") or [])[:6]
        n = max(1, len(steps))
        img = d.get("image")
        if img and not steps:          # a map alone: show it large
            if self.fx["image"] in ("frame", "brutal"):
                self.framed(s, img, mx + 0.15, y0, W - 2 * mx - 0.3, H - y0 - 0.95)
            else:
                self.picture(s, img, mx, y0, W - 2 * mx, H - y0 - 0.8)
            self.footer(s)
            return s
        if img or v in ("vertical", "log"):
            tw = W * 0.46 - mx if img else W - 2 * mx
            if img:
                if self.fx["image"] in ("frame", "brutal"):
                    self.framed(s, img, W * 0.5, y0, W * 0.5 - mx, H - y0 - 0.9)
                else:
                    self.picture(s, img, W * 0.5, y0 - 0.2, W * 0.5 - mx, H - y0 - 0.6)
            rh = (H - y0 - 0.85) / n
            mono = v == "log"
            if not mono:
                self.vline(s, mx + 0.14, y0 + 0.15, rh * (n - 1), st["muted"], 1)
            for i, stp in enumerate(steps):
                y = y0 + i * rh
                col = st["palette"][i % len(st["palette"])] if v not in ("line", "vertical") else st["accent"]
                if mono:
                    self.text(s, mx, y, 1.6, rh, f"[{stp.get('label', '')}]", 15, st["accent"], "Courier New", True)
                    self.text(s, mx + 1.7, y, tw - 1.7, rh - 0.05, stp.get("text", ""), st["body_size"] - 2, None, st["body"])
                else:
                    self.rect(s, mx + 0.02, y + 0.06, 0.24, 0.24, col, shape=MSO_SHAPE.OVAL)
                    self.step_label(s, mx + 0.5, y + 0.0, tw - 0.5, 0.32, stp.get("label", ""), col)
                    self.text(s, mx + 0.5, y + 0.34, tw - 0.5, rh - 0.4, stp.get("text", ""), st["body_size"] - 2, None, st["body"],
                              spacing=1.05)
            self.footer(s)
            return s
        cw = (W - 2 * mx) / n
        if v == "numbers":
            for i, stp in enumerate(steps):
                x = mx + i * cw
                self.hline(s, x, y0 + 0.1, cw - 0.25, st["line"], 2 if i else 4)
                self.text(s, x, y0 + 0.25, cw - 0.25, 1.1, f"{i + 1:02d}", 54, st["accent"] if i == 0 else st["text"],
                          st["head"], True, fitting=False)
                self.text(s, x, y0 + 1.45, cw - 0.25, 0.5, stp.get("label", ""), 17, None, st["body"], True)
                self.text(s, x, y0 + 2.0, cw - 0.25, H - y0 - 2.9, stp.get("text", ""), st["body_size"] - 3, st["muted"], st["body"])
        elif v == "blocks":
            for i, stp in enumerate(steps):
                x = mx + i * cw
                col = [st["accent"], st["accent3"], st["accent2"], "FFFFFF"][i % 4]
                tc = "FFFFFF" if col == st["accent2"] else st["text"]
                self.brutal(s, x, y0 + 0.1, cw - 0.3, H - y0 - 1.2, col, 0.09, 2)
                self.text(s, x + 0.2, y0 + 0.3, cw - 0.7, 0.5, stp.get("label", ""), 20, tc, st["head"], True)
                self.text(s, x + 0.2, y0 + 0.95, cw - 0.7, H - y0 - 2.4, stp.get("text", ""), st["body_size"] - 3, tc, st["body"])
        elif v == "chevrons":
            for i, stp in enumerate(steps):
                x = mx + i * cw
                col = mix(st["accent"], "FFFFFF", 0.12 * i) if i < n - 1 else st["accent2"]
                self.rect(s, x, y0 + 0.1, cw + 0.15, 0.85, col, shape=MSO_SHAPE.CHEVRON if i else MSO_SHAPE.PENTAGON)
                self.text(s, x + 0.35, y0 + 0.1, cw - 0.6, 0.85, stp.get("label", ""), 16, "FFFFFF", st["body"], True,
                          anchor="m")
                self.text(s, x + 0.1, y0 + 1.2, cw - 0.35, H - y0 - 2.1, stp.get("text", ""), st["body_size"] - 3, None, st["body"])
        elif v == "steps":
            y = y0 + 0.7
            self.hline(s, mx + cw / 2, y, W - 2 * mx - cw, st["line"], 4)
            for i, stp in enumerate(steps):
                cx = mx + i * cw + cw / 2
                col = st["palette"][i % len(st["palette"])]
                self.rect(s, cx - 0.55, y - 0.55, 1.1, 1.1, col, shape=MSO_SHAPE.OVAL)
                self.text(s, cx - 0.55, y - 0.55, 1.1, 1.1, str(i + 1), 30, "FFFFFF", st["head"], True, align="c",
                          anchor="m", fitting=False)
                self.text(s, cx - cw / 2 + 0.1, y + 0.75, cw - 0.2, 0.55, stp.get("label", ""), 18, col, st["head"],
                          True, align="c")
                self.text(s, cx - cw / 2 + 0.1, y + 1.35, cw - 0.2, H - y - 2.2, stp.get("text", ""), st["body_size"] - 3, None,
                          st["body"], align="c")
        else:   # line
            y = y0 + min(1.6, (H - y0) * 0.3)
            self.hline(s, mx, y + 0.1, W - 2 * mx, st["line"] if self.fx["card"] != "hairline" else st["accent"], 0.75)
            for i, stp in enumerate(steps):
                x = mx + i * cw
                self.rect(s, x, y, 0.2, 0.2, st["accent"] if self.fx["decor"] != "frame" else st["accent2"],
                          shape=MSO_SHAPE.OVAL if self.fx["decor"] != "frame" else MSO_SHAPE.DIAMOND)
                self.step_label(s, x, y - 0.75, cw - 0.25, 0.65, stp.get("label", ""), st["accent"])
                self.text(s, x, y + 0.45, cw - 0.3, H - y - 1.3, stp.get("text", ""), st["body_size"] - 2, None, st["body"],
                          spacing=1.1)
        self.footer(s)
        return s

    def step_label(self, s, x, y, w, h, label, color):
        """Short labels (« Jour 1 ») as the style's small caps; long ones as plain bold text."""
        if len(str(label)) <= 18:
            return self.label(s, x, y + h - 0.32, w, label, color, 11)
        return self.text(s, x, y, w, h, label, 15, color, self.st["body"], True, anchor="b")

    def people(self, d):
        st, W, H, mx, v = self.st, self.W, self.H, self.mx, self.fx["people"]
        s = self.new()
        y0 = self.title(s, d.get("title", ""), d.get("kicker"))
        ps = (d.get("people") or [])[:6]
        n = max(1, len(ps))
        cw = (W - 2 * mx) / n
        dsz = min(cw - 0.5, 2.3, H - y0 - 2.6)
        for i, p in enumerate(ps):
            x = mx + i * cw
            cx = x + (cw - dsz) / 2
            circle = v == "circle"
            col = st["palette"][i % len(st["palette"])]
            if p.get("photo"):
                self.picture(s, p["photo"], cx, y0, dsz, dsz, circle=circle)
            else:
                ini = "".join(w[0] for w in str(p.get("name", "?")).split()[:2]).upper()
                if self.fx["card"] == "brutal":
                    self.brutal(s, cx, y0, dsz, dsz, col, 0.08, 2, shape=MSO_SHAPE.OVAL)
                else:
                    self.rect(s, cx, y0, dsz, dsz, col if v == "circle" or self.fx["card"] != "rules" else st["alt"],
                              shape=MSO_SHAPE.OVAL if circle else MSO_SHAPE.RECTANGLE)
                self.text(s, cx, y0, dsz, dsz, ini, 44, on(col) if circle or self.fx["card"] != "rules" else st["accent"],
                          st["head"], True, align="c", anchor="m", fitting=False)
            ty = y0 + dsz + 0.25
            self.text(s, x + 0.1, ty, cw - 0.2, 0.45, p.get("name", ""), 22, None, st["head"], True, align="c")
            self.label(s, x + 0.1, ty + 0.5, cw - 0.2, p.get("role", ""), st["accent"], 10, "c")
            self.text(s, x + 0.15, ty + 0.9, cw - 0.3, H - ty - 1.6, p.get("text", ""), st["body_size"] - 4, st["muted"], st["body"],
                      align="c")
        self.footer(s)
        return s

    def quote(self, d):
        st, W, H, mx, v = self.st, self.W, self.H, self.mx, self.fx["quote"]
        q, a = d.get("quote", ""), d.get("author", "")
        if v == "mark":
            s = self.new(st["alt"] if st["bg"] > "8" else None)
            self.text(s, mx - 0.1, 0.3, 2.5, 2.5, "“", 240, st["accent"], "Georgia", fitting=False)
            self.text(s, mx + 0.9, 1.9, W - 2 * mx - 1.8, 3.4, q, 42, None, "Georgia", italic=True, anchor="m",
                      spacing=1.1)
            self.hline(s, mx + 0.9, 5.6, 0.6, st["accent"], 1)
            self.label(s, mx + 0.9, 5.75, 8, a, st["muted"], 11)
        elif v == "plain":
            s = self.new()
            self.text(s, mx, 1.1, W - 2 * mx - 1, 4.2, f"« {q} »", 48, None, st["head"], True, anchor="b", spacing=1.0)
            self.hline(s, mx, 5.6, 0.9, st["accent"], 4)
            self.text(s, mx, 5.8, 8, 0.5, a, 18, st["muted"], st["body"])
        elif v == "code":
            s = self.new()
            self.rect(s, mx, 1.3, W - 2 * mx, 4.4, st["alt"], st["line"], 1)
            self.vline(s, mx, 1.3, 4.4, st["accent"], 4)
            self.text(s, mx + 0.5, 1.6, W - 2 * mx - 1, 3.4, "> " + q, 34, st["text"], st["head"], True, anchor="m")
            self.text(s, mx + 0.5, 5.0, W - 2 * mx - 1, 0.5, "-- " + a, 16, st["accent"], "Courier New")
        elif v == "block":
            s = self.new()
            self.brutal(s, mx, 1.0, W - 2 * mx - 0.2, 4.4, st["accent3"], 0.15, 3)
            self.text(s, mx + 0.6, 1.3, W - 2 * mx - 1.4, 3.8, f"« {q} »", 42, st["text"], st["head"], True, anchor="m")
            aw = min(W - 2 * mx, 0.8 + len(a) * 0.13)
            self.brutal(s, mx + 0.6, 5.1, aw, 0.55, "FFFFFF", 0.06, 2)
            self.text(s, mx + 0.6, 5.1, aw, 0.55, a, 16, st["text"], st["body"], True, align="c", anchor="m")
        elif v == "centered":
            s = self.new()
            self.hline(s, W / 2 - 0.6, 1.5, 1.2, st["accent"], 0.75)
            self.text(s, mx + 0.6, 1.8, W - 2 * mx - 1.2, 3.2, q, 36, None, "Georgia", italic=True, align="c",
                      anchor="m", spacing=1.15)
            self.hline(s, W / 2 - 0.6, 5.25, 1.2, st["accent"], 0.75)
            self.label(s, mx, 5.5, W - 2 * mx, a, st["muted"], 11, "c")
        elif v == "callout":
            s = self.new()
            self.vline(s, mx, 1.4, 3.6, st["accent"], 8)
            self.text(s, mx + 0.5, 1.4, W - 2 * mx - 1.5, 3.6, f"« {q} »", 36, st["accent"], st["head"], True,
                      anchor="m", spacing=1.05)
            self.text(s, mx + 0.5, 5.3, 8, 0.5, a, 17, st["muted"], st["body"])
        elif v == "bubble":
            s = self.new()
            b = self.rect(s, mx + 0.4, 1.0, W - 2 * mx - 1.2, 3.9, st["alt"], shape=MSO_SHAPE.RECTANGULAR_CALLOUT)
            try:
                b.adjustments[0], b.adjustments[1] = -0.38, 0.68
            except Exception:
                pass
            self.text(s, mx + 0.9, 1.3, W - 2 * mx - 2.2, 3.3, q, 32, None, st["body"], True, align="c", anchor="m")
            ini = "".join(w[0] for w in a.split()[:2]).upper() or "?"
            self.rect(s, mx + 0.4, 5.55, 0.9, 0.9, st["accent2"], shape=MSO_SHAPE.OVAL)
            self.text(s, mx + 0.4, 5.55, 0.9, 0.9, ini, 22, "FFFFFF", st["body"], True, align="c", anchor="m",
                      fitting=False)
            self.text(s, mx + 1.5, 5.75, 8, 0.5, a, 18, None, st["body"], True)
        elif v == "geo":
            s = self.new(decor=False)
            self.rect(s, mx, 0.9, 2.2, 2.2, st["accent"], shape=MSO_SHAPE.OVAL)
            self.text(s, mx, 0.9, 2.2, 2.2, "“", 150, "FFFFFF", "Georgia", align="c", anchor="m", fitting=False)
            self.rect(s, W - 2.2, H - 1.4, 2.2, 1.4, st["accent2"])
            self.text(s, mx + 2.6, 1.2, W - 2 * mx - 3.2, 3.8, q, 40, None, st["head"], True, anchor="m")
            self.hline(s, mx + 2.6, 5.4, 1.0, st["text"], 4)
            self.text(s, mx + 2.6, 5.6, 8, 0.5, a, 18, st["muted"], st["body"])
        else:
            s = self.new()
            self.text(s, mx, 1.5, W - 2 * mx, 3.5, q, 40, None, st["head"])
        return s

    def code(self, d):
        st, W, H, mx = self.st, self.W, self.H, self.mx
        s = self.new()
        y0 = self.title(s, d.get("title", ""), d.get("kicker"))
        dark = st["bg"] if lum(st["bg"]) < 0.2 else "1E1E1E"
        panel = st["alt"] if lum(st["bg"]) < 0.2 else "1E1E1E"
        code = str(d.get("code", "")).rstrip("\n")
        lines = code.split("\n")[:18]
        h = H - y0 - (1.4 if d.get("caption") else 0.9)
        self.rect(s, mx, y0, W - 2 * mx, h, panel, st["line"] if dark == st["bg"] else None, 1)
        size = max(10, min(18, int((h - 0.5) * 72 / (len(lines) * 1.35))))
        nums = "\n".join(str(i + 1) for i in range(len(lines)))
        self.text(s, mx + 0.25, y0 + 0.25, 0.5, h - 0.4, nums, size, "6E7681", "Courier New", align="r", fitting=False)
        self.text(s, mx + 0.95, y0 + 0.25, W - 2 * mx - 1.2, h - 0.4, "\n".join(lines), size, "E6EDF3", "Courier New",
                  fitting=True, minimum=9)
        if d.get("caption"):
            self.text(s, mx, H - 1.25, W - 2 * mx, 0.5, d["caption"], 15, st["muted"], st["body"])
        self.footer(s)
        return s

    def closing(self, d):
        st, W, H, mx, v = self.st, self.W, self.H, self.mx, self.fx["closing"]
        title, sub, contact = d.get("title", "Merci"), d.get("subtitle", ""), d.get("contact", "")
        img = d.get("image")
        if v == "window":
            s = self.new()
            x, y, w, h = mx, 1.2, W - 2 * mx, H - 2.4
            self.rect(s, x, y, w, h, st["alt"], st["line"], 1)
            self.rect(s, x, y, w, 0.42, mix(st["alt"], "000000", 0.3), st["line"], 1)
            for i, c in enumerate(("FF5F56", "FFBD2E", "27C93F")):
                self.rect(s, x + 0.25 + i * 0.28, y + 0.13, 0.16, 0.16, c, shape=MSO_SHAPE.OVAL)
            self.head(s, x + 0.5, y + 0.9, w - 1, 1.4, title, 60)
            self.text(s, x + 0.5, y + 2.4, w - 1, 0.8, sub, 20, st["muted"], st["body"])
            self.text(s, x + 0.5, y + h - 0.9, w - 1, 0.5, ("$ " + contact) if contact else "$ exit", 16, st["accent"],
                      "Courier New")
        elif v == "blocks":
            s = self.new(st["accent2"])
            self.brutal(s, mx, 1.4, W - 2 * mx - 0.3, 3.0, st["accent"], 0.15, 3)
            self.head(s, mx + 0.5, 1.6, W - 2 * mx - 1.3, 2.6, title, 80, st["on_accent"], anchor="m")
            self.text(s, mx, 4.8, W - 2 * mx, 0.8, sub, 22, "FFFFFF", st["body"], True)
            if contact:
                cw = min(W - 2 * mx, 0.8 + len(contact) * 0.13)
                self.brutal(s, mx, 5.75, cw, 0.6, st["accent3"], 0.07, 2)
                self.text(s, mx, 5.75, cw, 0.6, contact, 16, st["text"], st["body"], True, align="c", anchor="m")
        elif v == "monogram":
            s = self.new()
            self.rect(s, W / 2 - 0.55, 1.1, 1.1, 1.1, None, st["accent"], 0.75, shape=MSO_SHAPE.OVAL)
            self.text(s, W / 2 - 0.55, 1.1, 1.1, 1.1, (self.dname or title or "M")[0], 34, st["accent"], st["head"],
                      align="c", anchor="m", fitting=False)
            self.head(s, mx, 2.6, W - 2 * mx, 1.3, title, 44, align="c", anchor="m")
            self.hline(s, W / 2 - 0.6, 4.1, 1.2, st["accent"], 0.75)
            self.text(s, mx + 1, 4.35, W - 2 * mx - 2, 0.9, sub, 18, st["muted"], st["body"], italic=True, align="c")
            self.label(s, mx, 5.6, W - 2 * mx, contact, st["text"], 11, "c")
        elif v == "panel_left":
            s = self.new(st["accent"])
            self.label(s, mx, 1.4, 8, self.dname, st["accent2"], 12)
            self.head(s, mx, 1.9, W * 0.7, 2.2, title, 60, "FFFFFF", anchor="t")
            self.hline(s, mx, 4.3, 0.8, st["accent2"], 3)
            self.text(s, mx, 4.55, W * 0.7, 1.0, sub, 20, "FFFFFF", st["body"])
            self.text(s, mx, 5.8, W * 0.7, 0.5, contact, 16, mix(st["accent"], "FFFFFF", 0.7), st["body"])
            self.logo_at(s, W - mx, H - 1.1, 0.6)
        elif v == "notebook":
            s = self.new()
            size = fit(title, W - 2 * mx, 1.6, 66, st["head"], True)
            lw = min(W - 2 * mx, width_in(fr(title), size, st["head"], True) + 0.4)
            self.rect(s, W / 2 - lw / 2, 2.3 + size / 72 * 0.45, lw, size / 72 * 0.7, st["accent3"], rot=-1)
            self.text(s, mx, 2.2, W - 2 * mx, 1.6, title, size, None, st["head"], True, align="c", fitting=False)
            self.text(s, mx + 1, 4.2, W - 2 * mx - 2, 0.9, sub, 22, st["muted"], st["body"], align="c")
            self.text(s, mx + 1, 5.3, W - 2 * mx - 2, 0.6, contact, 18, st["accent"], st["body"], True, align="c")
        elif v == "bleed" and img:
            s = self.new(decor=False)
            self.picture(s, img, 0, 0, W, H)
            self.gradient(s, 0, H * 0.3, W, H * 0.7, "000000", 0, 80)
            self.head(s, mx, H - 3.0, W * 0.7, 1.4, title, 60, "FFFFFF", anchor="b")
            self.text(s, mx, H - 1.45, W * 0.7, 0.6, sub, 20, "EEEEEE", st["body"], italic=True)
            self.label(s, mx, H - 0.75, 8, contact, st["accent"], 11)
        elif v == "badge":
            s = self.new()
            dsz = 3.4
            cx = W / 2 - dsz / 2
            self.rect(s, cx, 0.9, dsz, dsz, None, st["accent2"], 1.75, shape=MSO_SHAPE.OVAL)
            self.rect(s, cx + 0.12, 1.02, dsz - 0.24, dsz - 0.24, None, st["accent2"], 0.5, shape=MSO_SHAPE.OVAL)
            self.head(s, cx + 0.4, 0.9, dsz - 0.8, dsz, title, 46, align="c", anchor="m")
            self.text(s, mx + 1, 4.65, W - 2 * mx - 2, 0.8, sub, 19, st["muted"], st["body"], italic=True, align="c")
            self.label(s, mx, 5.6, W - 2 * mx, contact, st["accent"], 12, "c")
        elif v == "geo":
            s = self.new(decor=False)
            self.rect(s, W - 4.6, 0.0, 4.6, 4.6, st["accent2"], shape=MSO_SHAPE.OVAL)
            self.rect(s, W - 2.6, 3.6, 2.6, 2.6, st["accent3"])
            self.rect(s, mx, H - 1.2, 4.5, 0.3, st["accent"])
            self.head(s, mx, 1.3, W * 0.55, 2.6, title, 80, anchor="b")
            self.text(s, mx, 4.1, W * 0.5, 0.9, sub, 22, st["muted"], st["body"])
            self.text(s, mx, 5.2, W * 0.5, 0.5, contact, 17, st["text"], st["body"], True)
        else:   # type / poster / bleed without picture
            dark = v == "bleed"
            s = self.new()
            if v == "poster":
                self.rect(s, mx, 0.6, 0.9, 0.9, st["accent"])
            else:
                self.hline(s, mx, 1.0, W - 2 * mx, st["line"], 0.75)
            self.head(s, mx, 1.6, W * 0.8, 2.6, title, 88 if v == "poster" else 72, anchor="b", spacing=0.9)
            self.text(s, mx, 4.5, W * 0.65, 0.9, sub, 22, st["muted"], st["body"], italic=st["body"] == "Georgia")
            self.label(s, mx, 5.7, 8, contact, st["accent"], 12) if not dark else \
                self.text(s, mx, 5.7, 8, 0.5, contact, 16, st["accent"], st["body"])
        if v != "panel_left":
            self.logo_at(s, W - mx, H - 1.15, 0.6)
        return s

    @staticmethod
    def normalize(d):
        """Accept the field names models often use instead of ours (GLM-5.3 Flash on the token lab, 5/11: a timeline's
        `items[{when, text}]` and a quote's `text` left two of five slides empty)."""
        d = dict(d)
        lay = d.get("layout")

        def first(*keys, src=None):
            src = d if src is None else src
            for k in keys:
                if src.get(k) not in (None, "", [], {}):
                    return src[k]
            return None

        for a in ("photo", "picture", "img"):
            if a in d and "image" not in d:
                d["image"] = d.pop(a)
        lists = ("items", "points", "list", "content", "lines")
        if lay in ("bullets", "image_text", None) or lay not in Deck.FIELDS:
            v = first("bullets", *lists)
            if isinstance(v, list):
                d["bullets"] = v
        if lay in ("image_text", "image_full", "two_columns") and not d.get("text"):
            v = first("body", "paragraph", "description", "content")
            if isinstance(v, str):
                d["text"] = v
        if lay == "agenda" and not d.get("items"):
            d["items"] = first("bullets", "points", "sections", "list") or []
        if lay in ("cover", "section", "closing", "title") and not d.get("subtitle"):
            d["subtitle"] = d.get("text") or d.get("description") or ""
        if lay == "closing" and not d.get("contact"):
            d["contact"] = first("email", "contacts") or ""
        if lay == "timeline":
            steps = []
            for st in first("steps", "events", "items", "milestones", "timeline", "bullets", "points") or []:
                st = st if isinstance(st, dict) else {"text": str(st)}
                when = first("date", "when", "year", "time", "period", src=st)
                lab = first("label", "title", "heading", "name", src=st)
                txt = first("text", "description", "detail", "details", "body", "content", "what", "event", "summary", src=st)
                if when and lab and not txt:
                    txt = lab
                elif when and lab:
                    txt = f"{lab} : {txt}"
                elif lab and not txt and not when:
                    lab, txt = "", lab
                steps.append({"label": str(when or lab or ""), "text": str(txt or "")})
            d["steps"] = steps
        if lay == "quote":
            d["quote"] = first("quote", "text", "body", "content", "citation") or ""
            d["author"] = first("author", "by", "attribution", "who", "source") or ""
        if lay == "table":
            if not d.get("rows"):
                d["rows"] = first("data", "table", "cells") or []
            hdr = first("headers", "header", "columns")
            if hdr and isinstance(hdr, list):
                d["rows"] = [hdr] + list(d.get("rows") or [])
            rows = d.get("rows") or []
            if rows and isinstance(rows[0], dict):
                head = list(rows[0].keys())
                d["rows"] = [head] + [[r.get(h, "") for h in head] for r in rows]
        if lay in ("stats", "table") and not d.get("note"):
            d["note"] = d.get("takeaway") or d.get("context") or ""
        if lay == "big_number":
            if not d.get("value"):
                d["value"] = first("number", "figure", "stat", "big") or ""
            if not d.get("label"):
                d["label"] = first("caption", "subtitle", "title") or ""
            if not d.get("context"):
                d["context"] = d.get("takeaway") or d.get("text") or d.get("description") or ""
        if lay == "stats":
            d["stats"] = [x if isinstance(x, dict) else {"value": str(x)}
                          for x in first("stats", "numbers", "items", "metrics", "kpis", "figures") or []]
            for x in d["stats"]:
                x.setdefault("value", x.get("number", x.get("figure", "")))
                x.setdefault("label", x.get("text") or x.get("description") or x.get("title") or "")
        if lay == "two_columns":
            cols = first("columns", "cols")
            if isinstance(cols, list) and len(cols) >= 2:
                d.setdefault("left", cols[0])
                d.setdefault("right", cols[1])
            for key in ("left", "right"):
                col = d.get(key)
                if isinstance(col, list):
                    col = {"bullets": col}
                elif isinstance(col, str):
                    col = {"text": col}
                if isinstance(col, dict):
                    col = dict(col)
                    if not col.get("heading") and col.get("title"):
                        col["heading"] = col["title"]
                    if not col.get("bullets") and not col.get("text"):
                        v = first("items", "points", "list", src=col)
                        if isinstance(v, list):
                            col["bullets"] = v
                    d[key] = col
        if lay == "gallery":
            ims = []
            for im in first("images", "photos", "pictures", "items") or []:
                if isinstance(im, dict):
                    im = dict(im)
                    im["path"] = first("path", "image", "photo", "src", src=im) or ""
                ims.append(im)
            d["images"] = ims
        if lay == "people":
            d["people"] = first("people", "members", "team", "items") or []
            for x in d["people"]:
                if isinstance(x, dict):
                    x.setdefault("role", x.get("title", ""))
                    x.setdefault("text", x.get("bio", x.get("description", "")))
        if lay == "chart" and isinstance(d.get("chart"), dict):
            c = d["chart"] = dict(d["chart"])
            if not c.get("categories"):
                c["categories"] = first("labels", "x", src=c) or []
            if isinstance(c.get("series"), list):    # [{name, values|data}]
                c["series"] = {str(s.get("name", f"Série {i + 1}")): s.get("values") or s.get("data") or []
                               for i, s in enumerate(c["series"]) if isinstance(s, dict)}
            elif not c.get("series") and isinstance(c.get("values") or c.get("data"), list):
                c["series"] = {str(c.get("name") or d.get("title") or "Valeurs"): c.get("values") or c.get("data")}
        return d

    # Each layout's fields (the docstring's); what a slide needs to show anything (its first group).
    FIELDS = {
        "cover": ("subtitle", "image"), "title": ("subtitle", "image"), "section": ("subtitle", "image"),
        "agenda": ("items",), "bullets": ("bullets", "image"), "two_columns": ("left", "right"),
        "big_number": ("value", "label", "context"), "stats": ("stats", "note"),
        "chart": ("chart", "takeaway", "source"), "table": ("rows", "note", "source"),
        "image_text": ("text", "bullets", "image", "caption"), "image_full": ("image", "caption"),
        "gallery": ("images",), "timeline": ("steps", "image"), "people": ("people",), "quote": ("quote", "author"),
        "code": ("code", "caption"), "closing": ("subtitle", "contact", "image"),
    }
    NEEDS = {
        "bullets": ("bullets",), "two_columns": ("left", "right"), "big_number": ("value",), "stats": ("stats",),
        "chart": ("chart",), "table": ("rows",), "image_text": ("text", "bullets"), "image_full": ("image",),
        "gallery": ("images",), "timeline": ("steps", "image"), "people": ("people",), "quote": ("quote",),
        "code": ("code",),
    }
    COMMON = {"layout", "title", "kicker", "notes", "credit"}

    def check(self, index, raw, d):
        """What the build's answer says about a slide: empty (its content under none of the names we know) and the
        fields it left out (neither ours for that layout nor read under another name)."""
        lay = d.get("layout", "bullets")
        out = {}
        needs = self.NEEDS.get(lay)
        if needs and not any(d.get(k) not in (None, "", [], {}) for k in needs):
            out["empty"] = " | ".join(needs)
        known = set(self.FIELDS.get(lay, ())) | self.COMMON
        shown = json.dumps([d.get(k) for k in sorted(known)], ensure_ascii=False)
        ignored = sorted(k for k, v in raw.items() if k not in known and not all(
            json.dumps(leaf, ensure_ascii=False).strip('"') in shown for leaf in leaves(v)))
        if ignored:
            out["ignored"] = ignored
        if out:
            self.problems.append({"slide": index, "layout": lay, **out})

    def build(self, slide):
        raw = slide if isinstance(slide, dict) else {}
        slide = self.drop_missing(self.normalize(raw))
        self.check(self.n + 1, raw, slide)
        lay = slide.get("layout", "bullets")
        fn = {"cover": self.cover, "title": self.cover, "section": self.section, "agenda": self.agenda,
              "bullets": self.bullets, "two_columns": self.two_columns, "big_number": self.big_number,
              "stats": self.stats, "chart": self.chart, "table": self.table, "image_full": self.image_full,
              "image_text": self.image_text, "gallery": self.gallery, "timeline": self.timeline,
              "people": self.people, "quote": self.quote, "code": self.code, "closing": self.closing,
              }.get(lay, self.bullets)
        self.treated = False
        s = fn(slide)
        notes = slide.get("notes") or ""
        credits = [slide.get("credit")] + [im.get("credit") for im in slide.get("images") or [] if isinstance(im, dict)]
        credits = [modified(c) if self.treated else c for c in credits if c]
        if credits:
            notes = (notes + "\n" if notes else "") + "Photos : " + " ; ".join(credits)
        if notes:
            s.notes_slide.notes_text_frame.text = notes

    def save(self, out):
        for sl in self.spec.get("slides", []):
            self.build(sl)
        self.prs.core_properties.title = self.dname
        self.prs.save(out)
        shutil.rmtree(self.tmp, ignore_errors=True)


def main(argv=None):
    styles = "\n".join(f" {k:<10} {v['hint']}" for k, v in STYLES.items())
    ap = argparse.ArgumentParser(description="Content-only JSON -> designed .pptx",
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
    d = Deck(spec, os.path.dirname(os.path.abspath(a.spec)))
    d.save(a.output)
    out = {"ok": True, "output": a.output, "style": d.style_name, "slides": d.n}
    if d.missing:
        out["missing_images"] = d.missing
    if d.problems:
        out["check"] = d.problems
        out["fix"] = ("a slide marked `empty` shows nothing: fix it in the spec (fields: deck.py --help) and build "
                      "again; `ignored` fields are not shown")
    if shutil.which("soffice") or shutil.which("libreoffice"):   # a preview only where LibreOffice is installed
        out["look"] = f"python3 {os.path.join(HERE, 'look.py')} {a.output} --outdir render"
    print(json.dumps(out, ensure_ascii=False))


if __name__ == "__main__":
    sys.exit(main())
