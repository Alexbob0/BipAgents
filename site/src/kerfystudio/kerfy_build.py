"""kerfystudio.com: the publisher's page, its support page and the legal texts of docs/legal/ (rendered here from
their Markdown), in French at the root and in English, German and Spanish under /en/, /de/, /es/. Called by
site/build.py."""
from __future__ import annotations

import html
import json
import re
import shutil
from pathlib import Path
from typing import Dict, List, Optional, Tuple

LANGS = ["fr", "en", "de", "es"]
SITE = "https://kerfystudio.com"
LEGAL = ("privacy", "terms", "offer", "acceptable-use")


def prefix(lang: str) -> str:
    return "" if lang == "fr" else f"/{lang}"


# -- Markdown (the subset docs/legal uses) --------------------------------------------------------------------------

def inline(text: str, links: Dict[str, str]) -> str:
    out = html.escape(text, quote=False)
    out = re.sub(r"`([^`]+)`", r"<code>\1</code>", out)
    def link(m: re.Match) -> str:
        label, target = m.group(1), links.get(m.group(2), m.group(2))
        external = target.startswith("http") and "kerfystudio.com" not in target
        return f'<a href="{html.escape(target)}"{EXTERNAL if external else ""}>{label}</a>'
    out = re.sub(r"\[([^\]]+)\]\(([^)\s]+)\)", link, out)
    # Bare addresses become links (not those already inside one).
    out = re.sub(r'(?<!href=")(?<!">)(https?://[^\s<)]+[^\s<).,;:])', r'<a href="\1" rel="noopener">\1</a>', out)
    out = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", out)
    out = re.sub(r"(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])", r"<em>\1</em>", out)
    # What the publisher still has to fill in, marked so it shows.
    out = re.sub(r"\[([^\]<>]+)\](?!\()", r'<mark class="todo">[\1]</mark>', out)
    return out


EXTERNAL = ' rel="noopener"'


def markdown(text: str, links: Dict[str, str]) -> str:
    lines = text.splitlines()
    out: List[str] = []
    i = 0
    paragraph: List[str] = []

    def flush() -> None:
        if paragraph:
            out.append(f"<p>{inline(' '.join(s.strip() for s in paragraph), links)}</p>")
            paragraph.clear()

    while i < len(lines):
        line = lines[i]
        stripped = line.strip()
        if not stripped:
            flush(); i += 1; continue
        heading = re.match(r"^(#{1,4})\s+(.*)$", stripped)
        if heading:
            flush()
            level = len(heading.group(1))
            title = heading.group(2)
            anchor = re.sub(r"[^\w]+", "-", title.lower()).strip("-")[:60]
            out.append(f'<h{level} id="{anchor}">{inline(title, links)}</h{level}>')
            i += 1; continue
        if re.match(r"^(-{3,}|\*{3,})$", stripped):
            flush(); out.append("<hr>"); i += 1; continue
        if stripped.startswith(">"):
            flush()
            quote = []
            while i < len(lines) and lines[i].strip().startswith(">"):
                quote.append(lines[i].strip()[1:].strip()); i += 1
            out.append(f"<blockquote><p>{inline(' '.join(quote), links)}</p></blockquote>")
            continue
        if stripped.startswith("|") and i + 1 < len(lines) and re.match(r"^\|?\s*:?-{3,}", lines[i + 1].strip()):
            flush()
            cells = lambda row: [c.strip() for c in row.strip().strip("|").split("|")]
            head = cells(lines[i]); i += 2
            rows = []
            while i < len(lines) and lines[i].strip().startswith("|"):
                rows.append(cells(lines[i])); i += 1
            out.append('<div class="table"><table><thead><tr>' + "".join(f"<th>{inline(c, links)}</th>" for c in head)
                       + "</tr></thead><tbody>" + "".join("<tr>" + "".join(f"<td>{inline(c, links)}</td>" for c in r) + "</tr>"
                                                          for r in rows) + "</tbody></table></div>")
            continue
        if re.match(r"^(\s*)([-*]|\d+\.)\s+", line):
            flush()
            block: List[str] = []
            while i < len(lines) and (re.match(r"^(\s*)([-*]|\d+\.)\s+", lines[i])
                                      or (lines[i].startswith("  ") and lines[i].strip() and block)):
                block.append(lines[i]); i += 1
            out.append(listing(block, links))
            continue
        paragraph.append(line)
        i += 1
    flush()
    return "\n".join(out)


def listing(block: List[str], links: Dict[str, str]) -> str:
    """Nested lists from indented `-` / `1.` items (continuation lines join their item)."""
    items: List[Tuple[int, bool, str]] = []
    for line in block:
        m = re.match(r"^(\s*)([-*]|\d+\.)\s+(.*)$", line)
        if m:
            items.append((len(m.group(1)), m.group(2)[0].isdigit(), m.group(3)))
        elif items:
            depth, ordered, text = items[-1]
            items[-1] = (depth, ordered, text + " " + line.strip())
    html_out: List[str] = []
    stack: List[Tuple[int, str]] = []
    for depth, ordered, text in items:
        tag = "ol" if ordered else "ul"
        while stack and stack[-1][0] > depth:
            html_out.append(f"</li></{stack.pop()[1]}>")
        if not stack or stack[-1][0] < depth:
            html_out.append(f"<{tag}>")
            stack.append((depth, tag))
        else:
            html_out.append("</li>")
        html_out.append(f"<li>{inline(text, links)}")
    while stack:
        html_out.append(f"</li></{stack.pop()[1]}>")
    return "".join(html_out)


# -- Pages ------------------------------------------------------------------------------------------------------------

def links_for() -> Dict[str, str]:
    found = {f"{name}.{lang}.md": f"{prefix(lang)}/{name}/" for name in LEGAL for lang in LANGS}
    found["mentions-legales.fr.md"] = "/mentions-legales/"
    return found


def shell(t: Dict, lang: str, title: str, body: str, path: str, version: str, alternates: Optional[Dict[str, str]],
          page_class: str = "") -> str:
    langs = alternates or {}
    alt = "\n".join(f'<link rel="alternate" hreflang="{l}" href="{SITE}{p}">' for l, p in langs.items())
    switch = "".join(f'<a href="{p}" lang="{l}"{CURRENT if l == lang else ""}>{l.upper()}</a>'
                     for l, p in langs.items()) if len(langs) > 1 else ""
    p = prefix(lang)
    return f"""<!doctype html>
<html lang="{lang}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<title>{html.escape(title)}</title>
<meta name="description" content="{html.escape(t['meta']['description'])}">
<link rel="canonical" href="{SITE}{path}">
{alt}
<link rel="icon" href="/assets/favicon.svg" type="image/svg+xml">
<link rel="preload" href="/assets/nunito.ttf" as="font" type="font/ttf" crossorigin>
<link rel="stylesheet" href="/assets/kerfy.css?v={version}">
</head>
<body class="{page_class}">
<header class="bar">
  <a class="mark" href="{p or '/'}" aria-label="Kerfy Studio"><span class="dot"></span>Kerfy<b>Studio</b></a>
  <nav>
    <a href="https://bipagents.com{'' if lang == 'en' else '/' + lang}/">BipAgents</a>
    <a href="{p}/support/">{t['nav']['support']}</a>
    <span class="switch">{switch}</span>
  </nav>
</header>
<main>
{body}
</main>
<footer class="foot">
  <div>
    <a class="mark" href="{p or '/'}"><span class="dot"></span>Kerfy<b>Studio</b></a>
    <p>{t['foot']['about']}</p>
  </div>
  <nav>
    <a href="{p}/privacy/">{t['foot']['privacy']}</a>
    <a href="{p}/terms/">{t['foot']['terms']}</a>
    <a href="{p}/offer/">{t['foot']['offer']}</a>
    <a href="{p}/acceptable-use/">{t['foot']['acceptableUse']}</a>
    <a href="/mentions-legales/">{t['foot']['notice']}</a>
    <a href="{p}/support/">{t['nav']['support']}</a>
  </nav>
  <p class="fine">© 2026 Kerfy Studio · {t['foot']['fine']}</p>
</footer>
</body>
</html>
"""


CURRENT = ' aria-current="page"'


def home_body(t: Dict, lang: str, bips: Dict[str, str]) -> str:
    h = t["home"]
    cluster = "".join(f'<span class="b b{i}">{bips[c]}</span>' for i, c in enumerate(("daily", "wellness", "finance", "home", "learning")))
    values = "".join(f'<div class="value"><span class="vn">0{i}</span><h3>{v["title"]}</h3><p>{v["text"]}</p></div>'
                     for i, v in enumerate(h["values"], 1))
    return f"""<section class="hero">
  <p class="kicker">{h['kicker']}</p>
  <h1>{h['title']}</h1>
  <p class="lede">{h['lede']}</p>
</section>
<a class="product" href="https://bipagents.com{'' if lang == 'en' else '/' + lang}/">
  <div class="cluster" aria-hidden="true">{cluster}</div>
  <div class="product-copy">
    <p class="kicker">{h['productKicker']}</p>
    <h2>BipAgents</h2>
    <p>{h['product']}</p>
    <span class="go">{h['productCta']} →</span>
  </div>
</a>
<section class="values">{values}</section>
<section class="contact">
  <h2>{h['contactTitle']}</h2>
  <p>{h['contactText']}</p>
  <a class="mail" href="mailto:support@kerfystudio.com">support@kerfystudio.com</a>
</section>"""


def support_body(t: Dict, lang: str) -> str:
    s = t["support"]
    blocks = "".join(f'<section class="card"><h2>{b["title"]}</h2>{"".join(f"<p>{p}</p>" for p in b["text"])}</section>'
                     for b in s["blocks"])
    return f"""<section class="page-head">
  <p class="kicker">{s['kicker']}</p>
  <h1>{s['title']}</h1>
  <p class="lede">{s['lede']}</p>
  <a class="mail big" href="mailto:support@kerfystudio.com">support@kerfystudio.com</a>
</section>
<div class="cards">{blocks}</div>
<section class="card who"><h2>{s['whoTitle']}</h2><p>{s['who']}</p></section>"""


def build(root: Path, repo: Path, version: str) -> Path:
    import sys
    sys.path.insert(0, str(root))
    from build import copy, favicon, node_bip   # the shared helpers of site/build.py

    src, out = root / "src" / "kerfystudio", root / "dist" / "kerfystudio.com"
    if out.exists():
        shutil.rmtree(out)
    assets = out / "assets"
    assets.mkdir(parents=True)
    copy(src / "kerfy.css", assets / "kerfy.css")
    copy(repo / "android" / "app" / "src" / "main" / "res" / "font" / "nunito.ttf", assets / "nunito.ttf")
    (assets / "favicon.svg").write_text(favicon("daily"), encoding="utf-8")
    bips = {c: node_bip(c) or "" for c in ("daily", "wellness", "finance", "home", "learning")}
    texts = {lang: json.loads((src / "i18n" / f"{lang}.json").read_text(encoding="utf-8")) for lang in LANGS}
    links = links_for()

    def write(path: str, page: str) -> None:
        target = out / path.strip("/") / "index.html" if path.strip("/") else out / "index.html"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(page, encoding="utf-8")

    homes = {l: (prefix(l) or "") + "/" for l in LANGS}
    supports = {l: f"{prefix(l)}/support/" for l in LANGS}
    for lang in LANGS:
        t = texts[lang]
        write(homes[lang], shell(t, lang, t["meta"]["title"], home_body(t, lang, bips), homes[lang], version, homes, "home"))
        write(supports[lang], shell(t, lang, f"{t['support']['title']} · Kerfy Studio", support_body(t, lang),
                                    supports[lang], version, supports))
        for name in LEGAL:
            source = repo / "docs" / "legal" / f"{name}.{lang}.md"
            text = source.read_text(encoding="utf-8")
            text = "\n".join(text.splitlines()[1:]) if text.startswith("[") else text   # its language line: the header has one
            title = re.search(r"^#\s+(.*)$", text, re.M).group(1)
            pages = {l: f"{prefix(l)}/{name}/" for l in LANGS}
            write(pages[lang], shell(t, lang, f"{title} · Kerfy Studio", f'<article class="legal">{markdown(text, links)}</article>',
                                     pages[lang], version, pages, "doc"))
    notice = (repo / "docs" / "legal" / "mentions-legales.fr.md").read_text(encoding="utf-8")
    write("/mentions-legales/", shell(texts["fr"], "fr", "Mentions légales · Kerfy Studio",
                                      f'<article class="legal">{markdown(notice, links)}</article>', "/mentions-legales/",
                                      version, {"fr": "/mentions-legales/"}, "doc"))
    well_known = out / ".well-known"
    well_known.mkdir()
    (well_known / "security.txt").write_text(
        "Contact: mailto:support@kerfystudio.com\nExpires: 2027-10-01T00:00:00.000Z\nPreferred-Languages: fr, en\n"
        f"Canonical: {SITE}/.well-known/security.txt\nPolicy: https://github.com/Alexbob0/BipAgents/blob/main/SECURITY.md\n",
        encoding="utf-8")
    (out / "robots.txt").write_text(f"User-agent: *\nAllow: /\n", encoding="utf-8")
    return out
