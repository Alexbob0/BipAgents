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


# -- bipagents.com ---------------------------------------------------------------------------------------------------

def bipagents(version: str) -> Path:
    src, out = SRC / "bipagents", DIST / "bipagents.com"
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
    copy(REPO / "docs" / "play" / "index.html", out / "play" / "index.html")

    demo = json.loads((REPO / "App" / "Resources" / "Demo" / "demo.json").read_text(encoding="utf-8"))
    # When each Bip speaks in the podcast (scripts/podcast-timeline.py).
    timeline = json.loads((src / "podcast-timeline.json").read_text(encoding="utf-8"))
    agents = {a["id"]: a for a in demo["agents"]}
    template = (src / "index.html").read_text(encoding="utf-8")
    texts = {lang: json.loads((src / "i18n" / f"{lang}.json").read_text(encoding="utf-8")) for lang in LANGS}
    paths = {lang: "/" if lang == "en" else f"/{lang}/" for lang in LANGS}
    for lang in LANGS:
        t = texts[lang]
        values = flatten({k: v for k, v in t.items() if k not in ("marquee", "lines", "liveDialog")})
        if lang == "fr":   # « ? », « : »… never alone at the start of a line
            values = {k: french_spaces(v) if isinstance(v, str) else v for k, v in values.items()}
        values.update({
            "lang": lang, "path": paths[lang], "home": paths[lang], "version": version,
            "legalprefix": "" if lang == "fr" else f"/{lang}",
            "hreflang": "\n".join([f'<link rel="alternate" hreflang="{l}" href="{BIPAGENTS}{paths[l]}">' for l in LANGS]
                                  + [f'<link rel="alternate" hreflang="x-default" href="{BIPAGENTS}/">']),
            "langmenu": "".join(f'<a href="{paths[l]}" lang="{l}" hreflang="{l}"{CURRENT if l == lang else ""}>'
                                f'{html.escape(texts[l]["lang"]["name"])}</a>' for l in LANGS),
            "marquee": "".join(f'<span>{html.escape(word)}<i style="background:{COLORS[CATEGORY_ORDER[i % 8]][0]}"></i></span>'
                               for i, word in enumerate(t["marquee"])),
            "wavebars": "<i></i>" * 48,
        })
        cards = []
        for category in CATEGORY_ORDER:
            main, deep, tone = COLORS[category]
            name, label = t["bips"]["cat"][category]["name"], t["bips"]["cat"][category]["label"]
            cards.append(
                f'<article class="bip-card" style="--main:{main};--deep:{deep};--tint:var(--{tone}-tint)">'
                f'<div data-bip="{category}" data-interactive aria-label="{html.escape(name)}"></div>'
                f'<h3>{html.escape(name)}</h3><span class="chip">{html.escape(label)}</span></article>')
        values["bipcards"] = "\n    ".join(cards)
        scenarios = {}
        for scenario, agent in (("book_table", "daily"), ("heating", "home")):
            s = localized(demo["scenarios"][scenario], lang)
            scenarios[scenario] = {"agent": localized(agents[agent]["name"], lang), "category": agents[agent]["category"],
                                   "prompt": s["prompt"], "steps": s["steps"]}
        data = {"lang": lang, "lines": t["lines"], "live": t["liveDialog"], "scenarios": scenarios,
                "podcast": timeline.get(lang, [[0, 0]]),
                "labels": {"once": t["control"]["once"], "session": t["control"]["session"], "deny": t["control"]["deny"]}}
        values["data"] = json.dumps(data, ensure_ascii=False).replace("</", "<\\/")
        page = fill(template, values, f"bipagents/{lang}")
        target = out / paths[lang].strip("/") / "index.html" if lang != "en" else out / "index.html"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(page, encoding="utf-8")
        og_image(t, lang, assets / f"og-{lang}.png")
    (out / "robots.txt").write_text(f"User-agent: *\nAllow: /\nSitemap: {BIPAGENTS}/sitemap.xml\n", encoding="utf-8")
    (out / "sitemap.xml").write_text('<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
                                     + "".join(f"  <url><loc>{BIPAGENTS}{paths[l]}</loc></url>\n" for l in LANGS)
                                     + "</urlset>\n", encoding="utf-8")
    return out


def main(argv: List[str]) -> int:
    version = time.strftime("%Y%m%d%H%M%S")
    built = [bipagents(version)]
    kerfy = SRC / "kerfystudio"
    if kerfy.exists():
        sys.path.insert(0, str(kerfy))
        from kerfy_build import build as kerfy_build   # the publisher's site (its own module)
        built.append(kerfy_build(ROOT, REPO, version))
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
