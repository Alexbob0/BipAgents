"""kerfystudio.com: the publisher's page, its support page and the legal texts of docs/legal/ (rendered here from
their Markdown), in French at the root and in English, German and Spanish under /en/, /de/, /es/. Called by
site/build.py."""
from __future__ import annotations

import base64
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
{'<meta name="robots" content="noindex">' if page_class == "doc" else ""}
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
    <a class="hide-s" href="{p or ''}/#apps">{t['nav']['apps']}</a>
    <a class="hide-s" href="{p or ''}/#contact">{t['nav']['contact']}</a>
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
{'<script type="module" src="/assets/kerfy.js?v=' + version + '"></script>' if page_class == "home" else ""}
</body>
</html>
"""


CURRENT = ' aria-current="page"'


def esc(text: str) -> str:
    return html.escape(text, quote=True)


def company_value(company: Dict[str, str], key: str, label: str) -> str:
    value = company.get(key, "")
    return esc(value) if value else f'<mark class="todo">[{esc(label)}]</mark>'


# The legal texts' placeholders the company's details fill in (site/config.json), in the four languages. Placeholders
# may wrap over two lines in the Markdown, hence the spaces matched loosely.
LEGAL_FILL = {
    "capital": ("capital", "Kapital"),
    "siren": ("SIREN",),
    "address": ("adresse du siège", "registered office address", "Adresse des Sitzes", "dirección del domicilio social"),
    "rcsCity": ("ville du greffe", "registry city", "Ort des Handelsregisters", "ciudad del registro"),
    "phone": ("numéro de téléphone", "phone number", "Telefonnummer", "número de teléfono"),
    "president": ("Prénom Nom", "First name Last name", "Vorname Nachname", "Vorname Name", "Nombre Apellido", "NOM DU PRÉSIDENT"),
    "vat": ("numéro de TVA intracommunautaire ; supprimer cette ligne si aucun numéro n'est attribué",),
    "repo": ("URL du dépôt", "repository URL", "URL des Repositorys", "URL del repositorio"),
}


# The phone only in the legal texts (the law asks for it there), never as plain text: a button shows it, so crawlers
# reading the HTML don't harvest it (owner's choice, 2026-10-24). The texts get this token, the page its button.
PHONE_TOKEN = "@@KERFY-PHONE@@"
PHONE_LABEL = {"fr": "afficher le numéro", "en": "show the number", "de": "Nummer anzeigen", "es": "mostrar el número"}
PHONE_SCRIPT = ("<script>document.addEventListener('click',e=>{const b=e.target.closest('.phone-show');if(!b)return;"
                "const s=b.parentElement,n=atob(s.dataset.n).split('').reverse().join(''),a=document.createElement('a');"
                "a.href='tel:'+n.replace(/[^+0-9]/g,'').replace(/^0/,'+33');a.textContent=n;s.replaceChildren(a);});</script>")


def phone_html(cfg: Dict, lang: str) -> str:
    phone = cfg["company"].get("phone", "")
    if not phone:
        return '<mark class="todo">[téléphone]</mark>'
    encoded = base64.b64encode(phone[::-1].encode()).decode()
    return (f'<span class="phone" data-n="{encoded}"><button type="button" class="phone-show">'
            f'{esc(PHONE_LABEL.get(lang, PHONE_LABEL["en"]))}</button></span>')


def fill_company(text: str, cfg: Dict) -> str:
    values = dict(cfg["company"], repo=cfg.get("repo", ""))
    if values.get("phone"):
        values["phone"] = PHONE_TOKEN
    for key, labels in LEGAL_FILL.items():
        if not values.get(key):
            continue
        for label in labels:
            pattern = r"\[" + r"\s+".join(re.escape(word) for word in label.split()) + r"\]"
            text = re.sub(pattern, lambda _m: values[key], text)
    return text


TABLEE = {"paper": "#FBF8F3", "sable": "#ECE4D3", "ink": "#15171A", "pain": "#D4A574", "painDeep": "#B8873A",
          "herbe": "#8FA876", "herbeDeep": "#5A7A48", "terre": "#B8492A", "terreDeep": "#8A3315", "icon": "#F2B242"}

# Tablée's icon (a steaming pot), drawn again in SVG after the app's own icon.
TABLEE_ICON = ('<svg viewBox="0 0 1024 1024" aria-hidden="true"><rect width="1024" height="1024" rx="228" fill="#F2B242"/>'
               '<path d="M408 80q40 40 0 85t0 85M614 80q-40 40 0 85t0 85" fill="none" stroke="#2D2621" stroke-width="38" stroke-linecap="round"/>'
               '<rect x="482" y="318" width="60" height="50" rx="10" fill="#2D2621"/>'
               '<path d="M226 438h572l-40 420a44 44 0 0 1-44 42H310a44 44 0 0 1-44-42z" fill="#B84B2E"/>'
               '<rect x="144" y="472" width="96" height="50" rx="22" fill="#B84B2E"/><rect x="784" y="472" width="96" height="50" rx="22" fill="#B84B2E"/>'
               '<rect x="185" y="370" width="654" height="70" rx="28" fill="#2D2621"/></svg>')


def home_body(t: Dict, lang: str, cfg: Dict) -> str:
    from build import store_badges
    h, company, contact = t["home"], cfg["company"], cfg["contact"]
    bip_site = f"https://bipagents.com{'' if lang == 'en' else '/' + lang}/"
    b, tb, r = h["bip"], h["tablee"], h["rows"]
    pts = lambda items: "".join(f"<li>{esc(p)}</li>" for p in items)
    bip_badges = store_badges(cfg["stores"]["bipagents"], t["store"]["soon"], t["store"]["get"])
    tablee_badges = store_badges(cfg["stores"]["tablee"], t["store"]["soon"], t["store"]["get"])
    values = "".join(f'<div class="value reveal"><span class="vn">0{i}</span><h3>{esc(v["title"])}</h3><p>{esc(v["text"])}</p></div>'
                     for i, v in enumerate(h["values"], 1))
    rcs = (f'RCS {company_value(company, "rcsCity", "ville du greffe")} {company_value(company, "siren", "SIREN")}')
    rows = [(r["name"], esc(company["name"])), (r["form"], esc(r["formValue"])),
            (r["capital"], company_value(company, "capital", "capital") + (" €" if company.get("capital") else "")),
            (r["address"], company_value(company, "address", "adresse du siège")), (r["rcs"], rcs),
            (r["vat"], company_value(company, "vat", "TVA")), (r["president"], company_value(company, "president", "Prénom Nom")),
            (r["contact"], f'<a href="mailto:{contact["general"]}">{contact["general"]}</a>'),
            (r["support"], f'<a href="mailto:{contact["support"]}">{contact["support"]}</a>')]
    table = "".join(f"<div><dt>{esc(k)}</dt><dd>{v}</dd></div>" for k, v in rows)
    m = lambda pair: f'<p class="t-meal"><small>{esc(pair[0])}</small><b>{esc(pair[1])}</b></p>'
    return f"""<section class="hero">
  <div class="hero-copy reveal">
    <p class="kicker">{esc(h['kicker'])}</p>
    <h1>{'<br>'.join(esc(line) for line in h['title'].split(chr(10)))}</h1>
    <p class="lede">{esc(h['lede'])}</p>
  </div>
  <div class="hero-apps reveal" aria-hidden="true">
    <span class="app-tile tile-bip" data-tilt><span class="tile-bip-in" data-bip="daily"></span></span>
    <span class="app-tile tile-tablee" data-tilt>{TABLEE_ICON}</span>
  </div>
</section>

<section class="apps" id="apps">
  <p class="kicker reveal">{esc(h['appsKicker'])}</p>
  <h2 class="reveal">{esc(h['appsTitle'])}</h2>

  <article class="showcase show-bip reveal">
    <div class="show-copy">
      <p class="tag">{esc(b['tag'])}</p>
      <h3>BipAgents</h3>
      <p class="pitch">{esc(b['pitch'])}</p>
      <ul class="points">{pts(b['points'])}</ul>
      <div class="show-act"><a class="go" href="{bip_site}">{esc(b['cta'])} <span aria-hidden="true">→</span></a></div>
      <div class="badges">{bip_badges}</div>
    </div>
    <div class="show-art bip-art" aria-hidden="true">
      <div class="bip-stage">
        <span data-bip="daily" data-interactive class="sb sb0"></span><span data-bip="wellness" data-interactive class="sb sb1"></span>
        <span data-bip="finance" data-interactive class="sb sb2"></span><span data-bip="home" data-interactive class="sb sb3"></span>
        <span data-bip="learning" data-interactive class="sb sb4"></span>
      </div>
    </div>
  </article>

  <article class="showcase show-tablee reveal">
    <div class="show-copy">
      <p class="tag">{esc(tb['tag'])}</p>
      <h3>Tablée</h3>
      <p class="pitch">{esc(tb['pitch'])}</p>
      <ul class="points">{pts(tb['points'])}</ul>
      <div class="show-act"><a class="go" href="https://tablee.net" rel="noopener">{esc(tb['cta'])} <span aria-hidden="true">→</span></a></div>
      <div class="badges">{tablee_badges}</div>
    </div>
    <div class="show-art tablee-art" aria-hidden="true">
      <div class="t-phone" data-tilt>
        <div class="t-head"><span class="t-icon">{TABLEE_ICON}</span><b>{esc(tb['mockTitle'])}</b></div>
        <p class="t-family">👨‍👩‍👧‍👦 {esc(tb['family'])}</p>
        {m(tb['m1'])}{m(tb['m2'])}{m(tb['m3'])}
        <div class="t-list"><p class="t-list-h">🛒 {esc(tb['list'])}</p>
          <p class="t-item done"><i></i>{esc(tb['i1'])}</p><p class="t-item"><i></i>{esc(tb['i2'])}</p><p class="t-item"><i></i>{esc(tb['i3'])}</p></div>
      </div>
    </div>
  </article>
</section>

<section class="values-wrap" id="studio">
  <h2 class="reveal">{esc(h['valuesTitle'])}</h2>
  <div class="values">{values}</div>
</section>

<section class="company reveal" id="company">
  <div>
    <h2>{esc(h['companyTitle'])}</h2>
    <p>{esc(h['companyText'])}</p>
  </div>
  <dl>{table}</dl>
</section>

<section class="contact reveal" id="contact">
  <h2>{esc(h['contactTitle'])}</h2>
  <p>{esc(h['contactText'])}</p>
  <a class="mail" href="mailto:{contact['general']}">{contact['general']}</a>
  <p class="support-line">{esc(h['supportLine'])} <a href="{prefix(lang)}/support/">{esc(t['nav']['support'])} →</a></p>
</section>"""


def support_body(t: Dict, lang: str, cfg: Dict) -> str:
    s, company, contact = t["support"], cfg["company"], cfg["contact"]
    blocks = "".join(f'<section class="card"><h2>{b["title"]}</h2>{"".join(f"<p>{p}</p>" for p in b["text"])}</section>'
                     for b in s["blocks"])
    who = (f'{esc(company["name"])} ({esc(company["form"])}) · {company_value(company, "address", "adresse du siège")} · '
           f'RCS {company_value(company, "rcsCity", "ville du greffe")} {company_value(company, "siren", "SIREN")} · '
           f'<a href="mailto:{contact["support"]}">{contact["support"]}</a> · '
           f'<a href="mailto:{contact["general"]}">{contact["general"]}</a>')
    return f"""<section class="page-head">
  <p class="kicker">{s['kicker']}</p>
  <h1>{s['title']}</h1>
  <p class="lede">{s['lede']}</p>
  <a class="mail big" href="mailto:{contact['support']}">{contact['support']}</a>
</section>
<div class="cards">{blocks}</div>
<section class="card who"><h2>{s['whoTitle']}</h2><p>{who}</p></section>"""


def build(root: Path, repo: Path, version: str) -> Path:
    import sys
    sys.path.insert(0, str(root))
    from build import config, copy, favicon   # the shared helpers of site/build.py
    cfg = config()

    src, out = root / "src" / "kerfystudio", root / "dist" / "kerfystudio.com"
    if out.exists():
        shutil.rmtree(out)
    assets = out / "assets"
    assets.mkdir(parents=True)
    for name in ("kerfy.css", "kerfy.js"):
        copy(src / name, assets / name)
    copy(root / "src" / "shared" / "bips.js", assets / "bips.js")
    copy(repo / "android" / "app" / "src" / "main" / "res" / "font" / "nunito.ttf", assets / "nunito.ttf")
    (assets / "favicon.svg").write_text(favicon("daily"), encoding="utf-8")
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
        write(homes[lang], shell(t, lang, t["meta"]["title"], home_body(t, lang, cfg), homes[lang], version, homes, "home"))
        write(supports[lang], shell(t, lang, f"{t['support']['title']} · Kerfy Studio", support_body(t, lang, cfg),
                                    supports[lang], version, supports))
        for name in LEGAL:
            source = repo / "docs" / "legal" / f"{name}.{lang}.md"
            text = source.read_text(encoding="utf-8")
            text = "\n".join(text.splitlines()[1:]) if text.startswith("[") else text   # its language line: the header has one
            text = fill_company(text, cfg)
            title = re.search(r"^#\s+(.*)$", text, re.M).group(1)
            pages = {l: f"{prefix(l)}/{name}/" for l in LANGS}
            body = markdown(text, links).replace(PHONE_TOKEN, phone_html(cfg, lang))
            write(pages[lang], shell(t, lang, f"{title} · Kerfy Studio", f'<article class="legal">{body}</article>'
                                     + (PHONE_SCRIPT if "phone-show" in body else ""), pages[lang], version, pages, "doc"))
    notice = fill_company((repo / "docs" / "legal" / "mentions-legales.fr.md").read_text(encoding="utf-8"), cfg)
    write("/mentions-legales/", shell(texts["fr"], "fr", "Mentions légales · Kerfy Studio",
                                      f'<article class="legal">{markdown(notice, links).replace(PHONE_TOKEN, phone_html(cfg, "fr"))}'
                                      f'</article>{PHONE_SCRIPT}', "/mentions-legales/",
                                      version, {"fr": "/mentions-legales/"}, "doc"))
    well_known = out / ".well-known"
    well_known.mkdir()
    (well_known / "security.txt").write_text(
        f"Contact: mailto:{cfg['contact']['support']}\nExpires: 2027-10-01T00:00:00.000Z\nPreferred-Languages: fr, en\n"
        f"Canonical: {SITE}/.well-known/security.txt\nPolicy: https://github.com/Alexbob0/BipAgents/blob/main/SECURITY.md\n",
        encoding="utf-8")
    (out / "robots.txt").write_text(f"User-agent: *\nAllow: /\n", encoding="utf-8")
    return out
