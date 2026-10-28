"""Builds the websites into site/dist/: bipagents.com (the app) and kerfystudio.com (the publisher, its legal texts).

    python3 site/build.py            # everything
    python3 site/build.py --serve    # then serves site/dist/<site> on http://127.0.0.1:8770 (bipagents) and :8771 (kerfy)

Standard library only (the legal texts' Markdown is converted here). Texts come from site/src/<site>/i18n/*.json, the
demo's scenarios from App/Resources/Demo/demo.json, the podcasts from the apps' own files: nothing is copied
twice in the repository.
"""
from __future__ import annotations

import html
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Dict, List

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parent
SRC = ROOT / "src"
DIST = ROOT / "dist"
LANGS = ["en", "fr", "de", "es"]
BIPAGENTS = "https://bipagents.com"
COLORS = {  # main, deep, tint variable (site.css): the apps' pale palette (2026-10-24, the « sober » direction)
    "daily": ("#F4B5A3", "#C9775F", "coral"), "wellness": ("#A9D9B8", "#5E9E73", "green"),
    "finance": ("#F2D48E", "#B08A2E", "gold"), "home": ("#9FD6DA", "#4E9AA0", "teal"),
    "work": ("#B3C4F5", "#6F86C9", "blue"), "learning": ("#CABCF3", "#8873C9", "violet"),
    "creative": ("#F2B5CE", "#C47396", "pink"), "tech": ("#C3C8D3", "#7F8696", "blue"),
}
CURRENT = ' aria-current="page"'
# The header's menu: each language, under it the countries it is sold in; the currency follows (the country's in
# site/config.json, else the default). (language, id in ?region=, country, its name in the menu, short label in the
# header). A language with one entry shows its name alone.
REGIONS = [
    ("en", "us", "US", "US", "EN-US"), ("en", "uk", "GB", "UK", "EN-UK"), ("en", "ca", "CA", "Canada", "EN-CA"),
    ("en", "eu", "", "Europe", "EN-EU"), ("en", "sg", "SG", "Singapore", "EN-SG"), ("en", "il", "IL", "Israel", "EN-IL"),
    ("fr", "eu", "", "France", "FR"), ("fr", "ca", "CA", "Canada", "FR-CA"),
    ("de", "eu", "", "Deutschland", "DE"), ("es", "eu", "", "España", "ES"),
]


def french_spaces(text: str) -> str:
    """French typography: a narrow no-break space before ? ! ; : and inside « », so they stay with their word."""
    text = re.sub(r" ([?!;:»])", "\u202f\\1", text)
    return text.replace("« ", "«\u202f")


def flatten(tree: Any, prefix: str = "") -> Dict[str, Any]:
    out: Dict[str, Any] = {}
    if isinstance(tree, dict):
        for key, value in tree.items():
            out.update(flatten(value, f"{prefix}{key}."))
    else:
        out[prefix[:-1]] = tree
    return out


def fill(template: str, values: Dict[str, Any], where: str) -> str:
    def one(match: re.Match) -> str:
        key = match.group(1)
        if key not in values:
            raise KeyError(f"{where}: no text for {{{{{key}}}}}")
        value = values[key]
        return value if isinstance(value, str) else json.dumps(value, ensure_ascii=False)
    return re.sub(r"\{\{([\w.]+)\}\}", one, template)


def localized(value: Any, lang: str) -> Any:
    """demo.json's { "fr": …, "en": … } values, in one language, all the way down."""
    if isinstance(value, dict) and value and set(value) <= set(LANGS):
        return value.get(lang) or value.get("en")
    if isinstance(value, dict):
        return {k: localized(v, lang) for k, v in value.items()}
    if isinstance(value, list):
        return [localized(v, lang) for v in value]
    return value


def copy(source: Path, target: Path) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, target)


PIXEL = REPO / "scripts" / "pixel" / "pixel.json"   # the Bips' one source (the apps' too: scripts/pixel/gen.py)


def bips_engine(assets: Path) -> None:
    """The Bips' engine for a site: site/src/shared/bips.js and, next to it, pixel-data.js (scripts/pixel/pixel.json as
    a module), so the site draws the Bips from the same numbers as the apps."""
    copy(SRC / "shared" / "bips.js", assets / "bips.js")
    data = json.dumps(json.loads(PIXEL.read_text(encoding="utf-8")), ensure_ascii=False, separators=(",", ":"))
    (assets / "pixel-data.js").write_text("// scripts/pixel/pixel.json (written by site/build.py)\nexport default "
                                          + data + ";\n", encoding="utf-8")


def node_draw(expression: str) -> str | None:
    """SVG markup drawn by the site's own engine under Node (`m` is bips.js), for the icon and link previews; None
    without Node."""
    node = shutil.which("node")
    if not node:
        return None
    engine = DIST / ".engine"
    if not (engine / "bips.js").exists() or (engine / "bips.js").read_bytes() != (SRC / "shared" / "bips.js").read_bytes():
        bips_engine(engine)
    script = (f"const m=await import({json.dumps((engine / 'bips.js').as_uri())});"
              f"process.stdout.write({expression});")
    out = subprocess.run([node, "--input-type=module", "-e", script], capture_output=True, text=True, timeout=30)
    return out.stdout if out.returncode == 0 and out.stdout.startswith("<svg") else None


def node_bip(category: str, mood: str = "idle", costume: str | None = None) -> str | None:
    """A still Bip (the apps' pixel design) as SVG; None without Node."""
    return node_draw(f"m.staticBip({json.dumps(category)},{json.dumps(mood)},{json.dumps(costume)},230)")


def favicon(category: str = "daily") -> str:
    """The app's icon « Lueur bleuet » (scripts/pixel/icon.py) as an SVG with rounded corners."""
    drawn = node_draw("m.iconSVG(26)")
    return drawn or ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 120">'
                     '<rect width="120" height="120" rx="26" fill="#25272D"/></svg>')


def app_icons(assets: Path) -> None:
    """The app's icon as pictures, from the one scripts/pixel/icon.py writes for iOS: apple-touch-icon (180, opaque:
    iOS rounds it), and 192/512 for browsers that want a PNG."""
    big = REPO / "App" / "Assets.xcassets" / "AppIcon.appiconset" / "icon-1024.png"
    if not big.exists():
        return
    for name, size in (("apple-touch-icon.png", 180), ("icon-192.png", 192), ("icon-512.png", 512)):
        if shutil.which("sips"):
            subprocess.run(["sips", "-Z", str(size), str(big), "--out", str(assets / name)], capture_output=True)
        else:
            copy(big, assets / name)


def browser() -> str | None:
    """A Chromium to render images headless (Chrome, else Brave, else Chromium); None without one."""
    for path in ("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                 "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
                 "/Applications/Chromium.app/Contents/MacOS/Chromium"):
        if Path(path).exists():
            return path
    return shutil.which("chromium") or shutil.which("google-chrome")


def render_png(page: Path, target: Path, width: int, height: int) -> bool:
    """An HTML file, drawn at width×height into a PNG (store images, link previews)."""
    engine = browser()
    if not engine:
        return False
    target.parent.mkdir(parents=True, exist_ok=True)
    done = subprocess.run([engine, "--headless=new", "--disable-gpu", "--hide-scrollbars", "--force-device-scale-factor=1",
                           "--default-background-color=00000000", f"--window-size={width},{height}",
                           f"--screenshot={target}", page.as_uri()], capture_output=True, timeout=120)
    return done.returncode == 0 and target.exists()


def og_image(t: Dict[str, Any], lang: str, target: Path) -> None:
    """The link preview (1200×630): the app's icon and name, the headline, and a row of Bips in their colours."""
    font = (REPO / "android" / "app" / "src" / "main" / "res" / "font" / "nunito.ttf").as_uri()
    icon = node_draw("m.iconSVG(26)") or ""
    row = (("daily", "idle"), ("wellness", "content"), ("work", "think"), ("home", "listen"), ("learning", "idle"))
    bips = "".join(f'<div class="b b{i}">{node_bip(c, mood) or ""}</div>' for i, (c, mood) in enumerate(row))
    page = target.with_suffix(".html")
    page.write_text(f"""<!doctype html><meta charset="utf-8"><style>
@font-face {{ font-family: N; src: url("{font}"); font-weight: 200 1000; }}
html, body {{ margin: 0; width: 1200px; height: 630px; overflow: hidden; }}
body {{ font-family: N, sans-serif; background: #F7F6F3; color: #1D1F24; position: relative; }}
.copy {{ position: absolute; left: 72px; top: 60px; width: 1060px; }}
.brand {{ display: flex; align-items: center; gap: 18px; font-size: 40px; font-weight: 800; letter-spacing: -.02em; }}
.brand b {{ font-weight: 1000; }}
.icon {{ width: 84px; height: 84px; border-radius: 20px; box-shadow: 0 10px 24px rgba(20,22,28,.18); }}
.icon svg {{ width: 100%; height: 100%; display: block; }}
h1 {{ font-size: 62px; line-height: 1.04; letter-spacing: -.045em; font-weight: 1000; margin: 30px 0 0; max-width: 1000px; }}
p {{ font-size: 26px; font-weight: 800; color: #555B68; margin: 20px 0 0; max-width: 620px; }}
.row {{ position: absolute; right: 48px; bottom: 22px; display: flex; align-items: flex-end; gap: 14px; }}
.b {{ width: 176px; height: 176px; }} .b svg {{ width: 100%; height: 100%; overflow: visible; display: block; }}
.b1, .b3 {{ width: 150px; height: 150px; }}
</style><div class="copy"><div class="brand"><span class="icon">{icon}</span><span>Bip<b>Agents</b></span></div>
<h1>{html.escape(t["hero"]["line1"])} {html.escape(t["hero"]["line2"])}</h1>
<p>{html.escape(t["hero"]["badge"])}</p></div><div class="row">{bips}</div>""", encoding="utf-8")
    render_png(page, target, 1200, 630)
    page.unlink(missing_ok=True)


# -- Config: the owner's one place (site/config.json) ----------------------------------------------------------------

def config() -> Dict[str, Any]:
    """site/config.json, with the company details kept out of the public repository (the president's name, the
    phone) laid over it from internal/site-private.json when that file is there (the private checkout only)."""
    cfg = json.loads((ROOT / "config.json").read_text(encoding="utf-8"))
    private = ROOT.parent / "internal" / "site-private.json"
    if private.is_file():
        for key, value in json.loads(private.read_text(encoding="utf-8")).get("company", {}).items():
            if value:
                cfg["company"][key] = value
    return cfg


def money(value: float, lang: str, cents: bool = True) -> str:
    """6.49 → « 6,49 € » (fr, de, es) or « €6.49 » (en); 1000 without cents → « 1 000 € », « €1,000 »."""
    text = f"{value:,.2f}" if cents else f"{value:,.0f}"
    if lang == "en":
        return "€" + text
    return text.replace(",", " ").replace(".", ",") + " €"


def lang_menu(here: Dict[str, str], lang: str, texts: Dict[str, Any], regions: List[Dict[str, Any]],
              currencies: Dict[str, Any]) -> str:
    """The header's menu: a language, then its countries on one line (each its page with ?region=, which site.js turns
    into the currency and cleans from the address); a language sold in one place is its name alone."""
    out = []
    for l in LANGS:
        mine = [(r, row) for r, row in zip(regions, REGIONS) if r["lang"] == l]
        name = html.escape(texts[l]["lang"]["name"])
        current = CURRENT if l == lang else ""
        if len(mine) == 1:
            r = mine[0][0]
            out.append(f'<div class="lang-group"><a class="lang-name" href="{here[l]}?region={r["id"]}" lang="{l}" '
                       f'hreflang="{l}" data-region="{r["id"]}"{current}>{name}</a></div>')
            continue
        places = "".join(
            f'<a href="{here[l]}?region={r["id"]}" lang="{l}" hreflang="{l}-{r["country"] or "150"}" data-region="{r["id"]}"'
            f'{CURRENT if l == lang and r["id"] == "eu" else ""}>{html.escape(row[3])} '
            f'<small>{html.escape(currencies[r["currency"]]["symbol"])}</small></a>'
            for r, row in mine)
        out.append(f'<div class="lang-group"><a class="lang-name" href="{here[l]}" lang="{l}" hreflang="{l}">{name}</a>'
                   f'<div class="lang-places">{places}</div></div>')
    return "".join(out)


def todo(value: str, label: str) -> str:
    """A company detail, or a visible TODO mark while the owner hasn't given it."""
    return html.escape(value) if value else f'<mark class="todo">[{html.escape(label)}]</mark>'


def checkout_url(cfg: Dict[str, Any], plan: str, period: str, lang: str) -> str:
    c = cfg.get("checkout", {})
    url = ((c.get("links") or {}).get(plan) or {}).get(period) or c.get("url", "")
    return url.replace("{plan}", plan).replace("{period}", period).replace("{lang}", lang) if url else ""


APPLE = ('<svg viewBox="0 0 24 24" width="24" height="24"><path fill="currentColor" d="M16.4 12.6c0-2.6 2.1-3.8 2.2-3.9-1.2-1.8-3.1-2-3.7-2-1.6-.2-3.1.9-3.9.9-.8 0-2-.9-3.4-.9-1.7 0-3.3 1-4.2 2.6-1.8 3.1-.5 7.7 1.3 10.2.9 1.2 1.9 2.6 3.2 2.6 1.3-.1 1.8-.8 3.3-.8s2 .8 3.4.8c1.4 0 2.3-1.3 3.1-2.5 1-1.4 1.4-2.8 1.4-2.9 0 0-2.7-1-2.7-4.1zM13.9 4.9c.7-.9 1.2-2 1-3.2-1 0-2.3.7-3 1.6-.7.8-1.3 2-1.1 3.1 1.1.1 2.3-.6 3.1-1.5z"/></svg>')
PLAY = ('<svg viewBox="0 0 24 24" width="22" height="22"><path fill="#34A853" d="M3.6 1.8 13.4 12l-9.8 10.2c-.4-.2-.6-.7-.6-1.2V3c0-.5.2-1 .6-1.2z"/>'
        '<path fill="#FBBC04" d="m16.8 8.6-3.4 3.4 3.4 3.4 3.8-2.2c1.1-.6 1.1-1.8 0-2.4z"/>'
        '<path fill="#4285F4" d="M3.6 22.2 13.4 12l3.4 3.4L5 22.2c-.5.3-1 .3-1.4 0z"/>'
        '<path fill="#EA4335" d="M3.6 1.8c.4-.3.9-.3 1.4 0l11.8 6.8-3.4 3.4z"/></svg>')


def store_badges(links: Dict[str, str], soon: str, get: str, apple: str = "App Store") -> str:
    """The two store badges: links once the owner has set them, « coming soon » until then (apple: the Apple badge's
    name, « TestFlight » for the beta)."""
    out = []
    for key, name, glyph in (("appStore", apple, APPLE), ("googlePlay", "Google Play", PLAY)):
        url = links.get(key, "")
        inner = (f'<span class="store-glyph" aria-hidden="true">{glyph}</span><span class="store-text">'
                 f'<span class="store-kicker">{html.escape(get if url else soon)}</span>'
                 f'<span class="store-name">{name}</span></span>')
        out.append(f'<a class="store" href="{html.escape(url)}" rel="noopener">{inner}</a>' if url
                   else f'<span class="store store-soon">{inner}</span>')
    return "".join(out)


def walk_map(demo: Dict[str, Any], target: Path) -> str:
    """The map of the demo's « Sunday walk » page (an OpenStreetMap drawing the app embeds), as a small JPEG when sips
    is there, else the PNG itself. Returns the file's name."""
    import base64
    found = re.search(r"data:image/png;base64,([A-Za-z0-9+/=]+)", demo["pages"]["walk"]["template"])
    if not found:
        return ""
    target.parent.mkdir(parents=True, exist_ok=True)
    png = target.with_suffix(".png")
    png.write_bytes(base64.b64decode(found.group(1)))
    if shutil.which("sips"):
        done = subprocess.run(["sips", "-s", "format", "jpeg", "-s", "formatOptions", "60", str(png), "--out", str(target)],
                              capture_output=True)
        if done.returncode == 0 and target.exists():
            png.unlink()
            return target.name
    return png.name


def merged(base: Dict[str, Any], extra: Dict[str, Any]) -> Dict[str, Any]:
    """`extra` laid over `base`, dict by dict (a page's own texts file next to the site's)."""
    out = dict(base)
    for key, value in extra.items():
        out[key] = merged(out[key], value) if isinstance(value, dict) and isinstance(out.get(key), dict) else value
    return out


def migrate_prompt(lang: str) -> str:
    """The message to paste into Grok Bot (docs/migrate/grok-bot.<lang>.md's text block, also served by the bridge)."""
    doc = (REPO / "docs" / "migrate" / f"grok-bot.{lang}.md").read_text(encoding="utf-8")
    found = re.search(r"```text\n(.*?)\n```", doc, re.S)
    if not found:
        raise ValueError(f"docs/migrate/grok-bot.{lang}.md: no ```text block")
    return found.group(1)


def demo_page(demo: Dict[str, Any], name: str, lang: str) -> str:
    """One of the demo's pages (made with the page kit, like the agents' own pages), filled in one language."""
    page = demo["pages"][name]
    values = {k: localized(v, lang) for k, v in page.get("values", {}).items()}
    values["lang"] = lang
    return re.sub(r"\{\{(\w+)\}\}", lambda m: values.get(m.group(1), m.group(0)), page["template"])


def small_map(source: Path) -> str:
    """A smaller copy of the walk's map for the home's first screen (≈ 440 px wide), when sips is there; else the map
    itself. Returns the file's name."""
    target = source.with_name("walk-map-small.jpg")
    if source.exists() and shutil.which("sips"):
        done = subprocess.run(["sips", "-Z", "440", "-s", "format", "jpeg", "-s", "formatOptions", "62", str(source),
                               "--out", str(target)], capture_output=True)
        if done.returncode == 0 and target.exists():
            return target.name
    return source.name


def hero_show(demo: Dict[str, Any], t: Dict[str, Any], lang: str, map_file: str, demo_href: str) -> str:
    """The home's first screen, on the right: what one asks and what the agent answers with, taken from the app's own
    demo (demo.json): the walk's map, the booking (its 3 finds, then the approval), the subscriptions to cancel. A loose
    stack on a computer, one column on a phone; each exchange fades in once, one after another (none with reduced
    motion). A tap on one opens the demo page on that same scenario (/demo/#walk: site.js picks the tab from the address)."""
    tidy = french_spaces if lang == "fr" else (lambda s: s)
    e = lambda s: html.escape(tidy(s), quote=False)
    agents = {a["id"]: a for a in demo["agents"]}
    scen = {k: localized(v, lang) for k, v in demo["scenarios"].items()}

    def head(agent: str) -> str:
        a = agents[agent]
        return (f'<p class="ex-from"><span class="ex-av" data-bip-static="{a["category"]}"></span>'
                f'<b>{e(localized(a["name"], lang))}</b><span class="ai">{e(t["ai"])}</span>'
                f'<span class="ex-arrow" aria-hidden="true">→</span></p>')

    def one(i: int, agent: str, tone: str, ask: str, card: str, kind: str, scenario: str) -> str:
        # The whole exchange is the link: the ask's link stretched over it (the runs card's own link stays on top).
        go = (f'<a class="ex-go" href="{demo_href}#{scenario}">{e(ask)}'
              f'<span class="vh"> · {e(t["show"]["go"])}</span></a>')
        return (f'<li class="ex ex--{kind}" style="--i:{i};--tint:var(--{tone}-tint)">'
                f'<p class="ex-ask">{go}</p><div class="ex-card">{head(agent)}{card}</div></li>')

    # The booking: the agent found 3 places (the three as chips, the second picked, as in the demo's own run), then
    # asks before sending: the approval with its detail (« Pasta Mia · 20 h 30 · 2 personnes · vendredi soir »).
    book = scen["book_table"]
    asked = next(s for s in book["steps"] if "ask" in s)
    choices = asked["ask"]["choices"]
    answer = choices[1]
    found = re.split(r"(?<=[.?!])\s", asked["ask"]["question"])[0]
    approve = next(s for s in asked["then"] if "approve" in s)["approve"]
    where = answer.split(" · ★")[0]
    detail = " · ".join([where] + approve["command"].split("\n")[1:])
    c = t["control"]
    chips = "".join(f'<li{" class=on" if x == answer else ""}>{e(x)}</li>' for x in choices)
    approval = (f'<p class="ex-line">{e(found)}</p><ul class="ex-chips" aria-hidden="true">{chips}</ul>'
                f'<div class="ex-ok"><p class="ex-say">{e(approve["description"])}</p>'
                f'<p class="ex-detail">{e(detail)}</p>'
                f'<p class="ex-btns" aria-hidden="true"><span class="on">{e(c["once"])}</span><span>{e(c["deny"])}</span></p></div>')
    # The walk: the map with its numbered stops (the main picture).
    walk = {k: localized(v, lang) for k, v in demo["pages"]["walk"]["values"].items()}
    stops = re.findall(r'<li data-x="([\d.]+)" data-y="([\d.]+)"[^>]*data-color="(#\w+)">\{\{(\w+)\}\}</li>',
                       demo["pages"]["walk"]["template"])
    pins = "".join(f'<i style="left:{x}%;top:{y}%;--c:{col}">{n}</i>' for n, (x, y, col, _) in enumerate(stops, 1))
    names = " · ".join(e(walk[k]) for *_, k in stops)
    walk_card = (f'<p class="ex-title">🌳 {e(walk["title"])}</p>'
                 f'<div class="ex-map"><img src="/assets/showcase/{map_file}" alt="{e(walk["alt"])} · {names}" '
                 f'width="440" height="293" decoding="async">{pins}</div>')
    # The subscriptions: the two the agent found asleep, read from its question (« CinéPlus (13,99 €/mois, pas ouvert
    # depuis juillet) »), what cancelling both saves a month, and the choice.
    subs_q = next(s for s in scen["subscriptions"]["steps"] if "ask" in s)["ask"]["question"]
    subs = re.findall(r"([A-Z][\w\u00C0-\u024F]*(?: [A-Z][\w\u00C0-\u024F]*)*) \(([^()]*?\d[.,]\d\d[^,()]*), ([^()]+)\)", subs_q)
    if len(subs) != 2:
        raise ValueError(f"demo.json subscriptions ({lang}): expected two « Name (price, since) », got {subs}")
    euros = sum(float(re.search(r"\d+[.,]\d+", price).group().replace(",", ".")) for _, price, _ in subs)
    rows = "".join(f'<li><b>{e(name)}</b><span>{e(price)}</span><small>{e(since)}</small></li>' for name, price, since in subs)
    subs_card = (f'<ul class="ex-subs">{rows}</ul>'
                 f'<p class="ex-save">{e(t["show"]["saved"].replace("{amount}", money(round(euros), lang, cents=False)))}</p>'
                 f'<p class="ex-btns" aria-hidden="true"><span class="on">{e(t["show"]["cancelBoth"])}</span>'
                 f'<span>{e(t["show"]["choose"])}</span></p>')
    # Three exchanges: the walk's map on the left (the main picture), the booking and the subscriptions on the right.
    left = one(0, "daily", "coral", scen["walk"]["prompt"], walk_card, "map", "walk")
    right = (one(1, "daily", "coral", book["prompt"], approval, "approve", "book_table")
             + one(2, "budget", "gold", scen["subscriptions"]["prompt"], subs_card, "subs", "subscriptions"))
    return f'<ul class="ex-col">{left}</ul><ul class="ex-col">{right}</ul>'


# -- bipagents.com ---------------------------------------------------------------------------------------------------

# The demo page's tabs (scenario, agent); /demo/#<scenario> opens on one (the home's first-screen cards link there).
DEMO_TABS = (("book_table", "daily"), ("walk", "daily"), ("runs_weeks", "coach"), ("heating", "home"),
             ("bins", "home"), ("subscriptions", "budget"))
# The pages: (name, template, path under the language's root). The home sells, and its prices hold the self-install
# too (« on my computer », #install); the demo and every question have their own pages, sharing the header and the
# footer (src/bipagents/partials/). /install/ only sends to the home's #install (old links keep working).
PAGES = (("home", "index.html", ""), ("demo", "demo.html", "demo/"), ("faq", "faq.html", "faq/"),
         ("install", "install.html", "install/"), ("beta", "beta.html", "beta/"),
         ("grokbot", "grok-bot.html", "grok-bot/"))
FAQ_HOME = (1, 3, 2, 6, 4, 5)   # what's included, AI credit, free week, stopping, privacy, Duo/Famille
FAQ_COUNT = 11


def bipagents(version: str) -> Path:
    src, out = SRC / "bipagents", DIST / "bipagents.com"
    cfg = config()
    if out.exists():
        shutil.rmtree(out)
    assets = out / "assets"
    assets.mkdir(parents=True)
    for name in ("site.css", "site.js"):
        copy(src / name, assets / name)
    bips_engine(assets)
    copy(REPO / "android" / "app" / "src" / "main" / "res" / "font" / "nunito.ttf", assets / "nunito.ttf")
    (assets / "favicon.svg").write_text(favicon(), encoding="utf-8")
    app_icons(assets)
    for shot in (src / "shots").glob("*"):
        copy(shot, assets / "shots" / shot.name)
    media = assets / "media"
    for lang in LANGS:
        copy(REPO / "App" / "Resources" / "Demo" / f"podcast-morning-{lang}.m4a", media / f"podcast-morning-{lang}.m4a")
    # /play/: the Bips to play with (src/bipagents/play.html, the same engine as every page).
    (out / "play").mkdir(parents=True, exist_ok=True)
    (out / "play" / "index.html").write_text((src / "play.html").read_text(encoding="utf-8").replace("{{version}}", version),
                                             encoding="utf-8")

    demo = json.loads((REPO / "App" / "Resources" / "Demo" / "demo.json").read_text(encoding="utf-8"))
    map_file = walk_map(demo, assets / "showcase" / "walk-map.jpg")
    map_small = small_map(assets / "showcase" / map_file) if map_file else ""
    agents = {a["id"]: a for a in demo["agents"]}
    partials = {p.stem: p.read_text(encoding="utf-8") for p in (src / "partials").glob("*.html")}
    templates = {name: re.sub(r"\{\{>(\w+)\}\}", lambda m: partials[m.group(1)], (src / file).read_text(encoding="utf-8"))
                 for name, file, _ in PAGES}
    texts = {lang: json.loads((src / "i18n" / f"{lang}.json").read_text(encoding="utf-8")) for lang in LANGS}
    # Pages with texts of their own (i18n/<page>.<lang>.json), kept apart from the home's.
    for lang in LANGS:
        for extra in sorted((src / "i18n").glob(f"*.{lang}.json")):
            texts[lang] = merged(texts[lang], json.loads(extra.read_text(encoding="utf-8")))
    paths = {lang: "/" if lang == "en" else f"/{lang}/" for lang in LANGS}
    prices, company = cfg["prices"], cfg["company"]
    # Built in the default currency (the one without countries); the script switches to the visitor's.
    currencies = prices["currencies"]
    default_currency = next(code for code, c in currencies.items() if not c.get("countries"))
    base = currencies[default_currency]
    # No price « in the app » while the app isn't in the stores: no reference price for something not on sale.
    stores_live = any(cfg["stores"]["bipagents"].get(k) for k in ("appStore", "googlePlay"))
    if not stores_live:
        currencies = {code: {**c, "store": None} for code, c in currencies.items()}
        prices = {**prices, "currencies": currencies}
        base = currencies[default_currency]
    regions = [{"id": rid, "lang": l, "country": country, "short": short,
                "currency": next((code for code, c in currencies.items() if country and country in c.get("countries", [])),
                                 default_currency)}
               for l, rid, country, _, short in REGIONS]
    (assets / "pages").mkdir()
    for lang in LANGS:
        t = texts[lang]
        # A page the demo's agent makes (the page kit's real output), shown in a frame in the chat.
        (assets / "pages" / f"runs-{lang}.html").write_text(demo_page(demo, "runs", lang), encoding="utf-8")
        values = flatten({k: v for k, v in t.items() if k != "lines"})
        for where in ("web", "store"):
            for period in ("month", "year"):
                for plan in prices["plans"]:
                    values[f"p.{where}.{period}.{plan}"] = money(base[where][period][plan], lang) if base.get(where) else ""
        if not stores_live:
            values["pricing.inApp"] = ""
        for plan in prices["plans"]:
            values[f"p.web.yearmonth.{plan}"] = money(round(base["web"]["year"][plan] / 12, 2), lang)
        # « from 6,49 € »: the lowest monthly price, which the script follows in the visitor's currency
        values["p.from"] = f'<span data-from>{money(base["web"]["month"][prices["plans"][0]], lang)}</span>'
        if lang == "fr":   # « ? », « : »… never alone at the start of a line
            values = {k: french_spaces(v) if isinstance(v, str) else v for k, v in values.items()}
        values["currencyoptions"] = "".join(
            f'<option value="{code}"{" selected" if code == default_currency else ""}>{html.escape(c["symbol"])} {code}</option>'
            for code, c in currencies.items())
        values["heroshow"] = hero_show(demo, t, lang, map_small, f"{paths[lang]}demo/")
        default_url = checkout_url(cfg, "famille", "year", lang)
        badges = store_badges(cfg["stores"]["bipagents"], t["store"]["soon"], t["store"]["get"])
        # The public beta (site/config.json « beta »): TestFlight and Google Play's testing links once they are set.
        beta = cfg.get("beta", {})
        cap = beta.get("cap") or ""
        cap = (cap.get(lang) or "") if isinstance(cap, dict) else cap

        def faq(numbers: Any) -> str:
            return "\n    ".join(f'<details><summary>{values[f"faq.{n}.q"]}</summary><p>{values[f"faq.{n}.a"]}</p></details>'
                                 for n in numbers)
        values.update({
            "lang": lang, "home": paths[lang], "version": version,
            "demohref": f"{paths[lang]}demo/", "faqhref": f"{paths[lang]}faq/", "installhref": f"{paths[lang]}#install",
            "legalprefix": "" if lang == "fr" else f"/{lang}",
            "storebadges": badges,
            "betahref": f"{paths[lang]}beta/",
            "grokbothref": f"{paths[lang]}grok-bot/",
            "grokbotprompt": html.escape(migrate_prompt(lang), quote=False),
            "betabadges": store_badges({"appStore": beta.get("testflight", ""), "googlePlay": beta.get("play", "")},
                                       t["beta"]["storeSoon"], t["beta"]["storeGet"], "TestFlight"),
            "betasoon": "" if beta.get("testflight") or beta.get("play") else f'<p class="beta-soon">{values["beta.soon"]}</p>',
            "betacap": f'<p class="beta-cap">{html.escape(values["beta.cap"].replace("{cap}", cap))}</p>' if cap else "",
            # The home keeps the questions that settle a purchase; /faq/ has them all.
            "faqhome": faq(FAQ_HOME), "faqall": faq(range(1, FAQ_COUNT + 1)),
            "buy": (f'<a class="pill pill-buy" id="buy" href="{html.escape(default_url)}" rel="noopener">'
                    f'<span class="buy-label">{html.escape(t["pricing"]["cta"])}</span><span aria-hidden="true">→</span></a>'
                    if default_url else
                    f'<p class="soon-web">{t["pricing"]["soon"]}</p><div class="cta-row center">{badges}</div>'),
            "checkoutstate": "on" if default_url else "off",
            "company": (f'{html.escape(company["name"])}, {html.escape(company["form"])} · {html.escape(t["foot"]["capital"])} '
                        f'{todo(money(float(company["capital"]), lang, cents=False) if company["capital"] else "", "capital")} · RCS '
                        f'{todo(company["rcsCity"], "RCS")} {todo(company["siren"], "SIREN")} · '
                        f'{todo(company["address"], t["foot"]["address"])}'),
            "contact": cfg["contact"]["general"],
        })
        common = {"lang": lang, "lines": t["lines"], "regions": regions, "currency": default_currency,
                  "currencies": list(currencies)}
        tool_labels = json.loads((SRC / "bipagents" / "tool-labels.json").read_text())
        scenarios = {}
        for scenario, agent in DEMO_TABS:
            s = localized(demo["scenarios"][scenario], lang)
            scenarios[scenario] = {"agent": localized(agents[agent]["name"], lang), "category": agents[agent]["category"],
                                   "prompt": s["prompt"], "steps": s["steps"], "elapsed": s.get("elapsed", 0)}
        walk = {k: localized(v, lang) for k, v in demo["pages"]["walk"]["values"].items()}
        stops = re.findall(r'<li data-x="([\d.]+)" data-y="([\d.]+)"[^>]*data-color="(#\w+)">\{\{(\w+)\}\}</li>',
                           demo["pages"]["walk"]["template"])
        # Each page gets the data its script needs: the home its prices, the demo its scenarios.
        data = {
            "home": {**common, "prices": {**{k: v for k, v in prices.items() if not k.startswith("_")},
                                "default": default_currency},
                     "checkout": {plan: {period: checkout_url(cfg, plan, period, lang) for period in ("month", "year")}
                                  for plan in prices["plans"]},
                     "pricing": t["pricing"]["js"]},
            "demo": {**common, "scenarios": scenarios,
                     "labels": {"once": t["control"]["once"], "session": t["control"]["session"], "deny": t["control"]["deny"],
                                "open": t["demo"]["open"]},
                     # tool rows and the working line in plain words, as the apps word them (scripts/tool-labels.py)
                     "tools": {"list": tool_labels["kinds"], "words": tool_labels["text"][lang]["kinds"],
                               "extra": tool_labels["text"][lang]["extra"],
                               "thinking": tool_labels["text"][lang]["thinking"]},
                     "pages": {"runs": {"src": f"/assets/pages/runs-{lang}.html",
                                        "title": localized(demo["pages"]["runs"]["values"]["title"], lang)},
                               "walk": {"src": f"/assets/showcase/{map_file}", "title": walk["title"], "subtitle": walk["subtitle"],
                                        "alt": walk["alt"], "tip": walk["tip"], "source": walk["source"],
                                        "stops": [{"x": float(x), "y": float(y), "color": c, "name": walk[k]}
                                                  for x, y, c, k in stops]}}},
        }
        for name, _, sub in PAGES:
            here = {l: paths[l] + sub for l in LANGS}
            page_values = dict(values)
            page_values.update({
                "pagename": name, "path": here[lang],
                "pagetitle": values["meta.title"] if name == "home" else values[f"page.{name}.title"],
                "pagedesc": values["meta.description"] if name == "home" else values[f"page.{name}.description"],
                "hreflang": "\n".join([f'<link rel="alternate" hreflang="{l}" href="{BIPAGENTS}{here[l]}">' for l in LANGS]
                                      + [f'<link rel="alternate" hreflang="x-default" href="{BIPAGENTS}{here["en"]}">']),
                "langmenu": lang_menu(here, lang, texts, regions, currencies),
                "cur.demo": CURRENT if name == "demo" else "", "cur.faq": CURRENT if name == "faq" else "",
                "cur.beta": CURRENT if name == "beta" else "",
                "data": json.dumps(data.get(name, common), ensure_ascii=False).replace("</", "<\\/"),
            })
            # Twice: the texts themselves may carry {{…}} (the prices).
            page = fill(fill(templates[name], page_values, f"bipagents/{lang}/{name}"), page_values,
                        f"bipagents/{lang}/{name} (texts)")
            target = out / here[lang].strip("/") / "index.html"
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(page, encoding="utf-8")
        og_image(t, lang, assets / f"og-{lang}.png")
    links(src, out)
    (out / "robots.txt").write_text(f"User-agent: *\nAllow: /\nSitemap: {BIPAGENTS}/sitemap.xml\n", encoding="utf-8")
    (out / "sitemap.xml").write_text('<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
                                     + "".join(f"  <url><loc>{BIPAGENTS}{paths[l]}{sub}</loc></url>\n"
                                               for name, _, sub in PAGES if name != "install" for l in LANGS)
                                     + "</urlset>\n", encoding="utf-8")
    return out


def links(src: Path, out: Path) -> None:
    """The links that open the app: https://bipagents.com/p#… (a shared place), with the files that let iOS and Android
    open them in the app (universal links, App Links), and the page shown where it isn't installed. BIP_TEAM_ID (the
    Apple team) and BIP_PLAY_SHA256 (Google Play's app signing certificate) come from the environment, else from
    site/config.json (stores.bipagents: appleTeamId, playSigningSha256 — neither is secret)."""
    stores = config()["stores"]["bipagents"]
    team = os.environ.get("BIP_TEAM_ID") or stores.get("appleTeamId") or "TEAMID"
    play = os.environ.get("BIP_PLAY_SHA256") or stores.get("playSigningSha256") or "PLAY:APP:SIGNING:SHA256"
    if team == "TEAMID" or play.startswith("PLAY:"):
        print("links: BIP_TEAM_ID / BIP_PLAY_SHA256 not set, placeholders written (set them before publishing)")
    known = out / ".well-known"
    known.mkdir(parents=True, exist_ok=True)
    (known / "apple-app-site-association").write_text(json.dumps({"applinks": {"details": [{
        "appIDs": [f"{team}.com.kerfystudio.bipagents"],
        "components": [{"/": "/p", "#": "*", "comment": "a place of a Duo or a family, shared with this person"},
                       {"/": "/v", "#": "*", "comment": "a conversation shown to this person read-only"},
                       {"/": "/m/*", "comment": "an agent shared as a model (« Partager comme modèle »)"}]}]}},
        indent=2) + "\n", encoding="utf-8")
    (known / "assetlinks.json").write_text(json.dumps([{
        "relation": ["delegate_permission/common.handle_all_urls"],
        "target": {"namespace": "android_app", "package_name": "com.kerfystudio.bipagents",
                   "sha256_cert_fingerprints": [play]}}], indent=2) + "\n", encoding="utf-8")
    # The page shown where the app isn't installed: the store links from site/config.json once the owner has them.
    # /p: a place shared with this person; /v: a conversation shown to them read-only (« Partager en lecture »);
    # /m/<id>: an agent shared as a model (.htaccess sends every /m/<id> to /m/index.html).
    for name in ("p", "v", "m"):
        page = (src / "links" / f"{name}.html").read_text(encoding="utf-8")
        if stores.get("appStore"):
            page = page.replace("https://apps.apple.com/app/bipagents/idAPPSTOREID", stores["appStore"])
        if stores.get("googlePlay"):
            page = page.replace("https://play.google.com/store/apps/details?id=com.kerfystudio.bipagents", stores["googlePlay"])
        (out / name).mkdir(parents=True, exist_ok=True)
        (out / name / "index.html").write_text(page, encoding="utf-8")


def main(argv: List[str]) -> int:
    version = time.strftime("%Y%m%d%H%M%S")
    built = [bipagents(version)]
    kerfy = SRC / "kerfystudio"
    if kerfy.exists():
        sys.path.insert(0, str(kerfy))
        from kerfy_build import build as kerfy_build   # the publisher's site (its own module)
        built.append(kerfy_build(ROOT, REPO, version))
    sys.path.insert(0, str(ROOT))
    import hosting   # OVH's Apache: .htaccess (CSP with the inline scripts' hashes), 404 page, security.txt
    for path in built:
        hosting.finish(path, path.name, config())
    for path in built:
        print(f"built {path.relative_to(REPO)}")
    if "--serve" in argv:
        import functools
        import http.server
        import threading
        servers = []
        class Fresh(http.server.SimpleHTTPRequestHandler):
            def end_headers(self) -> None:
                self.send_header("Cache-Control", "no-store")   # a rebuild shows at once
                super().end_headers()

            def log_message(self, *args: Any) -> None:
                pass

        for port, path in zip((8770, 8771), built):
            handler = functools.partial(Fresh, directory=str(path))
            server = http.server.ThreadingHTTPServer(("127.0.0.1", port), handler)
            threading.Thread(target=server.serve_forever, daemon=True).start()
            servers.append(server)
            print(f"serving {path.name} on http://127.0.0.1:{port}")
        try:
            threading.Event().wait()
        except KeyboardInterrupt:
            pass
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
