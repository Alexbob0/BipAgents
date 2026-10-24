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
CATEGORY_ORDER = ["daily", "wellness", "finance", "home", "work", "learning", "creative", "tech"]
COLORS = {  # main, deep, tint variable (site.css)
    "daily": ("#FF7B5C", "#C9472A", "coral"), "wellness": ("#3CC37A", "#1D8A4F", "green"),
    "finance": ("#F6B826", "#9A6A00", "gold"), "home": ("#1FB2C1", "#0D7F8B", "teal"),
    "work": ("#4F7CFF", "#2B55D4", "blue"), "learning": ("#8E6CF5", "#6142D2", "violet"),
    "creative": ("#F2649E", "#C23774", "pink"), "tech": ("#6B7489", "#3F4659", "blue"),
}
CURRENT = ' aria-current="page"'
# The header's menu: each language with the countries it is sold in, the currency following (the country's in
# site/config.json, else the default). (language, id in ?region=, country, label, short label in the header)
REGIONS = [
    ("en", "us", "US", "English (US)", "EN-US"), ("en", "uk", "GB", "English (UK)", "EN-UK"),
    ("en", "ca", "CA", "English (Canada)", "EN-CA"), ("en", "sg", "SG", "English (Singapore)", "EN-SG"),
    ("en", "il", "IL", "English (Israel)", "EN-IL"), ("en", "eu", "", "English (Europe)", "EN-EU"),
    ("fr", "eu", "", "Français", "FR"), ("fr", "ca", "CA", "Français (Canada)", "FR-CA"),
    ("de", "eu", "", "Deutsch", "DE"), ("es", "eu", "", "Español", "ES"),
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


def node_bip(category: str, mood: str = "happy") -> str | None:
    """A still Bip drawn by the site's own code (site/src/shared/bips.js), for the icon; None without Node."""
    node = shutil.which("node")
    if not node:
        return None
    script = ("globalThis.matchMedia=()=>({matches:true});globalThis.addEventListener=()=>{};"
              "globalThis.requestAnimationFrame=()=>{};globalThis.IntersectionObserver=class{observe(){}};"
              f"const m=await import({json.dumps((SRC / 'shared' / 'bips.js').as_uri())});"
              f"process.stdout.write(m.staticBip({json.dumps(category)},{json.dumps(mood)}));")
    out = subprocess.run([node, "--input-type=module", "-e", script], capture_output=True, text=True, timeout=30)
    return out.stdout if out.returncode == 0 and out.stdout.startswith("<svg") else None


def favicon(category: str = "daily") -> str:
    drawn = node_bip(category) or ""
    inner = re.sub(r"^<svg[^>]*>|</svg>$", "", drawn)
    main = COLORS[category][0]
    return ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 120">'
            f'<rect width="120" height="120" rx="30" fill="#FFF3EE"/>'
            f'<g transform="translate(6 2) scale(.9)">{inner}</g></svg>' if inner else
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 120"><rect width="120" height="120" rx="30" fill="{main}"/></svg>')


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
    """The link preview (1200×630): the headline and five Bips."""
    font = (REPO / "android" / "app" / "src" / "main" / "res" / "font" / "nunito.ttf").as_uri()
    bips = "".join(f'<div class="b b{i}">{node_bip(c) or ""}</div>' for i, c in enumerate(("daily", "wellness", "finance", "home", "learning")))
    page = target.with_suffix(".html")
    page.write_text(f"""<!doctype html><meta charset="utf-8"><style>
@font-face {{ font-family: N; src: url("{font}"); font-weight: 200 1000; }}
html, body {{ margin: 0; width: 1200px; height: 630px; overflow: hidden; }}
body {{ font-family: N, sans-serif; background: linear-gradient(135deg, #FFE8E0, #FFF3D1 45%, #E2F5EA); color: #16181D; position: relative; }}
.copy {{ position: absolute; left: 72px; top: 70px; width: 640px; }}
.brand {{ font-size: 34px; font-weight: 800; letter-spacing: -.02em; }} .brand b {{ font-weight: 1000; }}
h1 {{ font-size: 76px; line-height: .98; letter-spacing: -.045em; font-weight: 1000; margin: 34px 0 0; }}
p {{ font-size: 26px; font-weight: 800; color: #4A5163; margin: 30px 0 0; }}
.b {{ position: absolute; width: 230px; height: 230px; }} .b svg {{ width: 100%; height: 100%; overflow: visible; }}
.b0 {{ left: 760px; top: 60px; }} .b1 {{ left: 940px; top: 150px; width: 200px; height: 200px; }} .b2 {{ left: 800px; top: 300px; }}
.b3 {{ left: 990px; top: 380px; width: 180px; height: 180px; }} .b4 {{ left: 690px; top: 430px; width: 160px; height: 160px; }}
</style><div class="copy"><div class="brand">Bip<b>Agents</b></div>
<h1>{html.escape(t["hero"]["line1"])}<br>{html.escape(t["hero"]["line2"])}<br>{html.escape(t["hero"]["line3"])}</h1>
<p>{html.escape(t["hero"]["badge"])}</p></div>{bips}""", encoding="utf-8")
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


def store_badges(links: Dict[str, str], soon: str, get: str) -> str:
    """The two store badges: links once the owner has set them, « coming soon » until then."""
    out = []
    for key, name, glyph in (("appStore", "App Store", APPLE), ("googlePlay", "Google Play", PLAY)):
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


def demo_page(demo: Dict[str, Any], name: str, lang: str) -> str:
    """One of the demo's pages (made with the page kit, like the agents' own pages), filled in one language."""
    page = demo["pages"][name]
    values = {k: localized(v, lang) for k, v in page.get("values", {}).items()}
    values["lang"] = lang
    return re.sub(r"\{\{(\w+)\}\}", lambda m: values.get(m.group(1), m.group(0)), page["template"])


# -- bipagents.com ---------------------------------------------------------------------------------------------------

DEMO_TABS = (("book_table", "daily"), ("walk", "daily"), ("runs_weeks", "coach"), ("heating", "home"))
# The pages: (name, template, path under the language's root). The home sells, and its prices hold the self-install
# too (« on my computer », #install); the demo and every question have their own pages, sharing the header and the
# footer (src/bipagents/partials/). /install/ only sends to the home's #install (old links keep working).
PAGES = (("home", "index.html", ""), ("demo", "demo.html", "demo/"), ("faq", "faq.html", "faq/"),
         ("install", "install.html", "install/"))
FAQ_HOME = (1, 3, 2, 6, 4, 5)   # what's included, AI credit, free week, stopping, privacy, Duo/Famille
FAQ_COUNT = 10


def bipagents(version: str) -> Path:
    src, out = SRC / "bipagents", DIST / "bipagents.com"
    cfg = config()
    if out.exists():
        shutil.rmtree(out)
    assets = out / "assets"
    assets.mkdir(parents=True)
    for name in ("site.css", "site.js"):
        copy(src / name, assets / name)
    copy(SRC / "shared" / "bips.js", assets / "bips.js")
    copy(REPO / "android" / "app" / "src" / "main" / "res" / "font" / "nunito.ttf", assets / "nunito.ttf")
    (assets / "favicon.svg").write_text(favicon(), encoding="utf-8")
    icon = REPO / "App" / "Assets.xcassets" / "AppIcon.appiconset"
    big = max(icon.glob("*.png"), key=lambda p: p.stat().st_size, default=None)
    if big and shutil.which("sips"):
        subprocess.run(["sips", "-Z", "180", str(big), "--out", str(assets / "apple-touch-icon.png")], capture_output=True)
    elif big:
        copy(big, assets / "apple-touch-icon.png")
    for shot in (src / "shots").glob("*"):
        copy(shot, assets / "shots" / shot.name)
    media = assets / "media"
    for lang in LANGS:
        copy(REPO / "App" / "Resources" / "Demo" / f"podcast-morning-{lang}.m4a", media / f"podcast-morning-{lang}.m4a")
    play = (REPO / "docs" / "play" / "index.html").read_text(encoding="utf-8")
    if 'rel="icon"' not in play:   # the playground, with the site's icon
        play = play.replace("<head>", '<head>\n<link rel="icon" href="/assets/favicon.svg" type="image/svg+xml">', 1)
    (out / "play").mkdir(parents=True, exist_ok=True)
    (out / "play" / "index.html").write_text(play, encoding="utf-8")

    demo = json.loads((REPO / "App" / "Resources" / "Demo" / "demo.json").read_text(encoding="utf-8"))
    map_file = walk_map(demo, assets / "showcase" / "walk-map.jpg")
    # When each Bip speaks in the podcast (scripts/podcast-timeline.py).
    timeline = json.loads((src / "podcast-timeline.json").read_text(encoding="utf-8"))
    agents = {a["id"]: a for a in demo["agents"]}
    partials = {p.stem: p.read_text(encoding="utf-8") for p in (src / "partials").glob("*.html")}
    templates = {name: re.sub(r"\{\{>(\w+)\}\}", lambda m: partials[m.group(1)], (src / file).read_text(encoding="utf-8"))
                 for name, file, _ in PAGES}
    texts = {lang: json.loads((src / "i18n" / f"{lang}.json").read_text(encoding="utf-8")) for lang in LANGS}
    paths = {lang: "/" if lang == "en" else f"/{lang}/" for lang in LANGS}
    prices, company = cfg["prices"], cfg["company"]
    # Built in the default currency (the one without countries); the script switches to the visitor's.
    currencies = prices["currencies"]
    default_currency = next(code for code, c in currencies.items() if not c.get("countries"))
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
        values = flatten({k: v for k, v in t.items() if k not in ("marquee", "lines", "liveDialog")})
        for where in ("web", "store"):
            for period in ("month", "year"):
                for plan in prices["plans"]:
                    values[f"p.{where}.{period}.{plan}"] = money(base[where][period][plan], lang)
        for plan in prices["plans"]:
            values[f"p.web.yearmonth.{plan}"] = money(round(base["web"]["year"][plan] / 12, 2), lang)
        # « from 6,49 € »: the lowest monthly price, which the script follows in the visitor's currency
        values["p.from"] = f'<span data-from>{money(base["web"]["month"][prices["plans"][0]], lang)}</span>'
        values["livefirst.you"] = html.escape(t["liveDialog"][0]["you"])
        values["livefirst.bip"] = html.escape(t["liveDialog"][0]["bip"])
        if lang == "fr":   # « ? », « : »… never alone at the start of a line
            values = {k: french_spaces(v) if isinstance(v, str) else v for k, v in values.items()}
        values["currencyoptions"] = "".join(
            f'<option value="{code}"{" selected" if code == default_currency else ""}>{html.escape(c["symbol"])} {code}</option>'
            for code, c in currencies.items())
        default_url = checkout_url(cfg, "famille", "year", lang)
        badges = store_badges(cfg["stores"]["bipagents"], t["store"]["soon"], t["store"]["get"])

        def faq(numbers: Any) -> str:
            return "\n    ".join(f'<details><summary>{values[f"faq.{n}.q"]}</summary><p>{values[f"faq.{n}.a"]}</p></details>'
                                 for n in numbers)
        values.update({
            "lang": lang, "home": paths[lang], "version": version,
            "demohref": f"{paths[lang]}demo/", "faqhref": f"{paths[lang]}faq/", "installhref": f"{paths[lang]}#install",
            "legalprefix": "" if lang == "fr" else f"/{lang}",
            "marquee": "".join(f'<span>{html.escape(word)}<i style="background:{COLORS[CATEGORY_ORDER[i % 8]][0]}"></i></span>'
                               for i, word in enumerate(t["marquee"])),
            "storebadges": badges,
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
        scenarios = {}
        for scenario, agent in DEMO_TABS:
            s = localized(demo["scenarios"][scenario], lang)
            scenarios[scenario] = {"agent": localized(agents[agent]["name"], lang), "category": agents[agent]["category"],
                                   "prompt": s["prompt"], "steps": s["steps"]}
        walk = {k: localized(v, lang) for k, v in demo["pages"]["walk"]["values"].items()}
        stops = re.findall(r'<li data-x="([\d.]+)" data-y="([\d.]+)"[^>]*data-color="(#\w+)">\{\{(\w+)\}\}</li>',
                           demo["pages"]["walk"]["template"])
        # Each page gets the data its script needs: the home its podcast, Live and prices, the demo its scenarios.
        data = {
            "home": {**common, "live": t["liveDialog"], "podcast": timeline.get(lang, [[0, 0]]),
                     "prices": {**{k: v for k, v in prices.items() if not k.startswith("_")},
                                "default": default_currency},
                     "checkout": {plan: {period: checkout_url(cfg, plan, period, lang) for period in ("month", "year")}
                                  for plan in prices["plans"]},
                     "pricing": t["pricing"]["js"]},
            "demo": {**common, "scenarios": scenarios,
                     "labels": {"once": t["control"]["once"], "session": t["control"]["session"], "deny": t["control"]["deny"],
                                "open": t["demo"]["open"]},
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
                # Each entry its language's page with ?region= (site.js keeps the currency and cleans the address).
                "langmenu": "".join(f'<a href="{here[l]}?region={rid}" lang="{l}" hreflang="{l}-{country or "150"}" '
                                    f'data-region="{rid}"{CURRENT if l == lang and rid == "eu" else ""}>'
                                    f'<span>{html.escape(label)}</span><small>{html.escape(currencies[r["currency"]]["symbol"])}</small></a>'
                                    for (l, rid, country, label, _), r in zip(REGIONS, regions)),
                "cur.demo": CURRENT if name == "demo" else "", "cur.faq": CURRENT if name == "faq" else "",
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
    Apple team) and BIP_PLAY_SHA256 (Google Play's app signing certificate) come from the environment."""
    team = os.environ.get("BIP_TEAM_ID", "TEAMID")
    play = os.environ.get("BIP_PLAY_SHA256", "PLAY:APP:SIGNING:SHA256")
    if team == "TEAMID" or play.startswith("PLAY:"):
        print("links: BIP_TEAM_ID / BIP_PLAY_SHA256 not set, placeholders written (set them before publishing)")
    known = out / ".well-known"
    known.mkdir(parents=True, exist_ok=True)
    (known / "apple-app-site-association").write_text(json.dumps({"applinks": {"details": [{
        "appIDs": [f"{team}.io.github.bipagents"],
        "components": [{"/": "/p", "#": "*", "comment": "a place of a Duo or a family, shared with this person"}]}]}},
        indent=2) + "\n", encoding="utf-8")
    (known / "assetlinks.json").write_text(json.dumps([{
        "relation": ["delegate_permission/common.handle_all_urls"],
        "target": {"namespace": "android_app", "package_name": "io.github.bipagents",
                   "sha256_cert_fingerprints": [play]}}], indent=2) + "\n", encoding="utf-8")
    # The page shown where the app isn't installed: the store links from site/config.json once the owner has them.
    page = (src / "links" / "p.html").read_text(encoding="utf-8")
    stores = config()["stores"]["bipagents"]
    if stores.get("appStore"):
        page = page.replace("https://apps.apple.com/app/bipagents/idAPPSTOREID", stores["appStore"])
    if stores.get("googlePlay"):
        page = page.replace("https://play.google.com/store/apps/details?id=io.github.bipagents", stores["googlePlay"])
    (out / "p").mkdir(parents=True, exist_ok=True)
    (out / "p" / "index.html").write_text(page, encoding="utf-8")


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
