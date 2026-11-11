//! Pages the agents make for the person: a dashboard on their blood tests, a tracker, a chart, a quiz, a small tool —
//! any subject, one prompt. The agent writes one HTML file (its CSS, script and data inline); the bridge keeps it in its
//! own folder and serves it to the app like any `MEDIA:` file under `/bipagents/pages`. An agent can be talked into
//! writing anything (a web page or a mail it read), so the page is locked before it is kept: a Content-Security-Policy
//! first in its head (no network, no forms, no frames: it runs offline; only the maps' `bipmap:` door, which the apps
//! answer themselves with fixed map files), Chart.js from the bridge put inline instead of a
//! CDN's, and the apps show it in a view with no way out (nothing loaded from the network, no navigation, no bridge to the
//! app). The agent's own source is kept beside it (`<id>.source`, never served) for `read_page`.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use crate::config::Config;
use crate::tools_knowledge::{self as tk, html::escape};
use crate::{py, Dict};

/// How the app asks for them (`GET /v1/media?path=…`).
pub const PREFIX: &str = "/bipagents/pages";
/// The agent's HTML; Chart.js adds 200 KB when the page uses it.
pub const MAX_BYTES: usize = 1_000_000;
/// Per agent.
pub const MAX_PAGES: usize = 100;
/// One door for the interactive maps (map_tiles.rs, 2026-10-23): `bipmap:` (the iPhone's web view serves it) and
/// `https://bipmap.invalid` (Android's web view answers it before any network: `.invalid` never resolves anywhere
/// else), for the map library the apps ship and the files the bridge keeps for it; MapLibre's worker is a blob. Outside
/// the apps neither answers and the page keeps the map's picture. No WebRTC (security audit of 11/11, IO-1 / AN-2): a
/// peer connection to a STUN/TURN server is no fetch, `connect-src` doesn't cover it; `webrtc 'block'` stops it in
/// Chromium (Android's web view), WebKit ignores it (the iPhone takes the constructors away itself).
pub const POLICY: &str =
    "default-src 'none'; script-src 'unsafe-inline' bipmap: https://bipmap.invalid; style-src 'unsafe-inline'; \
img-src data: blob:; font-src data:; media-src data: blob:; connect-src bipmap: https://bipmap.invalid; \
form-action 'none'; base-uri 'none'; frame-src 'none'; worker-src blob:; child-src blob:; object-src 'none'; \
webrtc 'block'";
/// No DNS look-up ahead of time for the names a page mentions (`<link rel=dns-prefetch>`, links): a name can carry data
/// and no CSP directive covers it. Sent as a header with the pages served over HTTP, and as a `<meta>` in the page
/// (the apps load it from a string, without headers).
pub const DNS_PREFETCH: (&str, &str) = ("x-dns-prefetch-control", "off");

/// What goes first in every page's head.
pub static HEAD: LazyLock<String> = LazyLock::new(|| {
    format!(
        "<meta http-equiv=\"Content-Security-Policy\" content=\"{POLICY}\"><meta http-equiv=\"x-dns-prefetch-control\" \
content=\"off\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"color-scheme\" content=\"light dark\">"
    )
});
/// A page id (Python's `re.match` with `$`: a final newline passes too).
pub static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9-]{0,59}\n?$").expect("valid regex"));
static DOCTYPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*(<!doctype[^>]*>)?").expect("valid regex"));
static CHART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)<script\b[^>]*\bsrc\s*=\s*["']?[^"'\s>]*chart(?:\.umd)?(?:\.min)?\.js[^"'\s>]*["']?[^>]*>\s*</script\s*>"#)
        .expect("valid regex")
});
static OUTSIDE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)<(script|link|img|iframe|audio|video|source)\b[^>]*\b(?:src|href)\s*=\s*["']?((?:https?:)?//[^"'\s>]+)"#,
    )
    .expect("valid regex")
});
static TITLE_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<title\b[^>]*>\s*[^<\s]").expect("valid regex"));
static DOCUMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<(html|head|body)\b").expect("valid regex"));

/// Python's `ValueError` here: a message for the agent.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct ValueError(pub String);

/// Chart.js, inlined in the pages that use it (`static/vendor/chart.umd.min.js`, embedded).
pub fn chart_js() -> &'static [u8] {
    crate::assets::file(&crate::assets::STATIC, "vendor/chart.umd.min.js").unwrap_or(b"")
}

/// The page kit (`static/pagekit/`, embedded): the app's look (light and dark, the agent's colour, cards, big numbers,
/// chips, tables that sort, progress bars, timelines, diagrams, sliders and switches) and its small script (`Bip.bars`,
/// `Bip.line`, `Bip.donut`, `Bip.spark`, `Bip.diagram`, `Bip.whatIf`, the app's colours for Chart.js), so that even a
/// small model makes a polished page with little HTML. Put inline in every page: it still opens offline, alone, years
/// later (the Carnet's Documents, an Obsidian export). All its CSS is in `:where()`: the agent's own CSS wins.
pub fn kit_css() -> &'static str {
    crate::assets::text(&crate::assets::STATIC, "pagekit/kit.css").unwrap_or("")
}

pub fn kit_js() -> &'static str {
    crate::assets::text(&crate::assets::STATIC, "pagekit/kit.js").unwrap_or("")
}

/// The kit as it goes in a page's head, in the colours of the agent's kind (`daily`, `wellness`…; else the app's).
pub fn kit(tint: Option<&str>) -> String {
    let tint = tint.filter(|t| crate::souls::CATEGORIES.contains(t)).unwrap_or("");
    format!(
        "<style id=\"bip-kit\">{}</style><script id=\"bip-kit-js\">{}</script>",
        kit_css().replace("</style", "<\\/style"),
        kit_js().replace("__BIP_TINT__", tint).replace("</script", "<\\/script")
    )
}

pub fn root(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("agent-pages")
}

pub fn slug(title: &str) -> String {
    let text = tk::dashes(&tk::ascii_fold(&tk::spaced_punctuation(title)).to_lowercase());
    let text = tk::head(text.trim_matches('-'), 40).trim_matches('-').to_string();
    if text.is_empty() {
        "page".into()
    } else {
        text
    }
}

pub fn media_path(agent: &str, page: &str) -> String {
    format!("{PREFIX}/{agent}/{page}.html")
}

/// The page as kept: the policy first in its head (before anything that could run), a whole document even when the
/// agent wrote a fragment, the kit then Chart.js inline. With what it asks from the network (it will not load: the
/// agent hears).
pub fn lock(source: &str, title: &str) -> (String, Vec<String>) {
    lock_with(source, title, None)
}

/// [`lock`] in the colours of the agent's kind.
pub fn lock_with(source: &str, title: &str, tint: Option<&str>) -> (String, Vec<String>) {
    let without_chart = CHART.replace_all(source, "");
    let mut outside: Vec<String> = OUTSIDE.captures_iter(&without_chart).map(|c| c[2].to_string()).collect();
    outside.sort();
    outside.dedup();
    let mut body = source.to_string();
    if CHART.is_match(&body) {
        let library =
            tk::universal_newlines(String::from_utf8_lossy(chart_js()).into_owned()).replace("</script", "<\\/script");
        // the app's colours for the charts, before the page's own script draws them
        let inline = format!("<script>{library}\n;window.Bip&&Bip._chart&&Bip._chart();</script>");
        body = CHART.replacen(&body, 1, regex::NoExpand(&inline)).into_owned();
        body = CHART.replace_all(&body, "").into_owned();
    }
    // its title in the page, as written (the apps read it there: a page without one showed its id's words, « L
    // enquete » for « L'enquête »)
    let named = if TITLE_TAG.is_match(&body) || title.trim().is_empty() {
        String::new()
    } else {
        format!("<title>{}</title>", escape(title))
    };
    if !DOCUMENT.is_match(&body) {
        body = format!("<body>{body}</body>");
    }
    let found = DOCTYPE.captures(&body).expect("always matches");
    let doctype = found.get(1).map(|m| m.as_str().to_string()).unwrap_or_else(|| "<!doctype html>".into());
    let rest = &body[found.get(0).map(|m| m.end()).unwrap_or(0)..];
    // A <meta> before <html> opens the head: the policy is in force before the agent's first tag.
    (format!("{doctype}\n{}{}{named}\n{rest}", *HEAD, kit(tint)), outside)
}

static MAP_SRC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bsrc\s*=\s*(["'])map:([A-Za-z0-9-]{1,40})(["'])"#).expect("valid regex"));

/// The agent's maps (`src="map:<id>"`, maps.rs) put inline as data: URIs: the page still opens offline, alone.
pub fn inline_maps(config: &Config, agent: &str, html: &str) -> String {
    use base64::Engine;
    MAP_SRC
        .replace_all(html, |c: &regex::Captures| match crate::maps::file(config, agent, &c[2]) {
            Some(path) => match std::fs::read(path) {
                Ok(data) => {
                    format!(
                        "src={}data:image/png;base64,{}{}",
                        &c[1],
                        base64::engine::general_purpose::STANDARD.encode(data),
                        &c[3]
                    )
                }
                Err(_) => c[0].to_string(),
            },
            None => c[0].to_string(),
        })
        .into_owned()
}

static MAP_CARD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<figure\b[^>]*\bclass\s*=\s*["'][^"']*\bmap-card\b[^"']*["'][^>]*>.*?</figure\s*>"#)
        .expect("valid regex")
});
static MAP_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:\bsrc\s*=\s*["']map:|\bdata-map\s*=\s*["'])([A-Za-z0-9-]{1,40})["']"#).expect("valid regex")
});
/// A map drawn by the page itself: its places would be the model's guesses (2026-10-31, Lisbon's monuments in the sea).
static OWN_MAP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bmaplibregl\b|leaflet(?:\.js|\.css|@|/dist)|\bL\.(?:map|marker|tileLayer)\s*\(|bipmap:")
        .expect("valid regex")
});

pub const OWN_MAP_REFUSED: &str = "No page: it draws a map of its own (or a map card map_image did not make), with \
places guessed. Call map_image with the places' names or addresses (and area: the town), then put the card it gives \
in the page as it is.";

/// The maps of a page as `map_image` made them: each map card (`<figure class="map-card">` with `map:<id>`) rebuilt
/// from the map's kept data, whatever the agent wrote in it (positions come from the geocoder only); a card of an
/// older map without data stays as written. Err: a map the page draws itself, or a card of no known map.
pub fn expand_maps(config: &Config, agent: &str, html: &str) -> Result<String, ValueError> {
    let outside_cards = MAP_CARD.replace_all(html, "");
    if OWN_MAP.is_match(&outside_cards) {
        return Err(ValueError(OWN_MAP_REFUSED.into()));
    }
    let mut refused = false;
    let expanded = MAP_CARD.replace_all(html, |c: &regex::Captures| {
        let card = &c[0];
        let id = MAP_ID.captures(card).map(|m| m[1].to_string()).unwrap_or_default();
        match crate::maps::kept(config, agent, &id) {
            Some(map) => crate::maps::figure_of(&map),
            None if !id.is_empty() && crate::maps::file(config, agent, &id).is_some() && !OWN_MAP.is_match(card) => {
                card.to_string()
            }
            None => {
                refused = true;
                card.to_string()
            }
        }
    });
    if refused {
        return Err(ValueError(OWN_MAP_REFUSED.into()));
    }
    Ok(expanded.into_owned())
}

fn meta_path(folder: &Path, page: &str) -> PathBuf {
    folder.join(format!("{page}.json"))
}

fn read_meta(path: &Path) -> Dict {
    match tk::read_text(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(Value::Object(map)) => map,
        _ => Dict::new(),
    }
}

/// `page.strip().lower().removesuffix(".html")`.
fn normalize(page: &str) -> String {
    let page = tk::strip(page).to_lowercase();
    page.strip_suffix(".html").map(str::to_string).unwrap_or(page)
}

fn unknown(page: &str) -> anyhow::Error {
    ValueError(format!("unknown page id: {page} (see list_pages)")).into()
}

/// Keeps a new page, or a new version of `page` (same path: the person's link shows the new one).
pub fn save(config: &Config, agent: &str, html: &str, title: &str, page: Option<&str>) -> anyhow::Result<Dict> {
    save_marked(config, agent, html, title, page, None)
}

/// What a page made in a shared conversation carries (shared_pages.rs): `group`, seen only by that conversation's
/// people (`GROUP_META` in its head: the apps offer no « Partager » for it); `asked_by`, made at a relative's request
/// (a line at its top says so, in the agent's language `lang`). Both kept across its versions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mark {
    pub group: bool,
    pub asked_by: Option<String>,
    pub lang: String,
}

/// The mark of a page seen only by a shared conversation's people (the apps read it in the HTML).
pub const GROUP_META: &str = "<meta name=\"bipagents-group\" content=\"1\">";
/// The class of the line saying whose request the page was made at.
pub const ASKED_CLASS: &str = "bip-asked";

static BODY_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<body\b[^>]*>").expect("valid regex"));

/// « Faite à la demande de Marie » in the agent's language.
pub fn asked_line(name: &str, lang: &str) -> String {
    let name = escape(name);
    match lang.split(['-', '_']).next().unwrap_or("") {
        "fr" => format!("Faite à la demande de {name}"),
        "de" => format!("Auf Wunsch von {name} erstellt"),
        "es" => format!("Hecha a petición de {name}"),
        "nl" => format!("Gemaakt op verzoek van {name}"),
        _ => format!("Made at {name}\u{2019}s request"),
    }
}

/// The locked page with its mark: the group's meta right after the policy, the request's line first in its body
/// (styled inline and `!important`, so the page's own CSS doesn't hide it by accident).
pub fn apply_mark(locked: &str, mark: &Mark) -> String {
    let mut out = locked.to_string();
    if mark.group && !out.contains(GROUP_META) {
        out = out.replacen(HEAD.as_str(), &format!("{}{GROUP_META}", *HEAD), 1);
    }
    if let Some(name) = mark.asked_by.as_deref().filter(|n| !n.trim().is_empty()) {
        let line = format!(
            "<div class=\"{ASKED_CLASS}\" role=\"note\" style=\"display:block!important;visibility:visible!important;\
opacity:1!important;position:static!important;margin:10px 12px 0!important;padding:6px 12px!important;\
border-radius:999px!important;background:rgba(127,127,127,.16)!important;color:inherit!important;\
font:600 12.5px/1.35 -apple-system,system-ui,sans-serif!important;width:fit-content!important\">{}</div>",
            asked_line(name, &mark.lang)
        );
        out = match BODY_TAG.find(&out) {
            Some(tag) => format!("{}{line}{}", &out[..tag.end()], &out[tag.end()..]),
            None => format!("{out}{line}"),
        };
    }
    out
}

/// [`save`], with the mark of a page made in a shared conversation (None: the page keeps the one it had).
pub fn save_marked(
    config: &Config,
    agent: &str,
    html: &str,
    title: &str,
    page: Option<&str>,
    mark: Option<&Mark>,
) -> anyhow::Result<Dict> {
    let html = tk::strip(html);
    if html.is_empty() {
        return Err(ValueError("the page is empty: give its whole HTML".into()).into());
    }
    let size = html.len();
    if size > MAX_BYTES {
        return Err(ValueError(format!(
            "the page is too big ({} KB, at most {} KB): keep the data to what it shows, no images in base64",
            size / 1000,
            MAX_BYTES / 1000
        ))
        .into());
    }
    let mut title = tk::head(&tk::squeeze(title), 80).to_string();
    let folder = root(config).join(agent);
    let mut page = match page {
        Some(p) => {
            let p = normalize(p);
            if !ID.is_match(&p) {
                return Err(unknown(&p));
            }
            Some(p)
        }
        None => None,
    };
    let known = match &page {
        Some(p) if !p.is_empty() => read_meta(&meta_path(&folder, p)),
        _ => Dict::new(),
    };
    if known.is_empty() {
        if list_pages(config, agent).len() >= MAX_PAGES {
            return Err(ValueError(format!(
                "this agent already has {MAX_PAGES} pages: update one or ask the person which to delete"
            ))
            .into());
        }
        if page.is_none() {
            let base = slug(if title.is_empty() { "page" } else { &title });
            let (mut candidate, mut n) = (base.clone(), 2);
            while meta_path(&folder, &candidate).exists() {
                candidate = format!("{base}-{n}");
                n += 1;
            }
            page = Some(candidate);
        }
    }
    let page = page.unwrap_or_default();
    if title.is_empty() {
        title = tk::str_or_empty(known.get("title"));
    }
    if title.is_empty() {
        title = tk::capitalize(&page.replace('-', " "));
    }
    let tint = config.agent(Some(agent)).and_then(|a| a.category.clone());
    let with_maps = expand_maps(config, agent, html)?;
    let (locked, outside) = lock_with(&with_maps, &title, tint.as_deref());
    let locked = inline_maps(config, agent, &locked);
    // the mark it had stays; a new one adds to it
    let kept = Mark {
        group: known.get("group").map(py::truthy).unwrap_or(false) || mark.is_some_and(|m| m.group),
        asked_by: mark
            .and_then(|m| m.asked_by.clone())
            .or_else(|| Some(tk::str_or_empty(known.get("asked_by"))).filter(|n| !n.is_empty())),
        lang: mark.map(|m| m.lang.clone()).unwrap_or_else(|| crate::names::agent_language(config, agent)),
    };
    let locked = apply_mark(&locked, &kept);
    std::fs::create_dir_all(&folder)?;
    let now = tk::now_iso_seconds();
    let version = match known.get("version") {
        Some(v) if py::truthy(v) => py::py_int(v).map_err(ValueError)?,
        _ => 0,
    } + 1;
    let created = match known.get("created") {
        Some(v) if py::truthy(v) => v.clone(),
        _ => Value::String(now.clone()),
    };
    let mut meta = Dict::new();
    meta.insert("title".into(), json!(title));
    meta.insert("created".into(), created);
    meta.insert("updated".into(), json!(now));
    meta.insert("version".into(), json!(version));
    meta.insert("bytes".into(), json!(locked.len()));
    // its place in the Carnet stays across versions (set_link)
    for key in ["notebook", "private"] {
        if let Some(v) = known.get(key) {
            meta.insert(key.into(), v.clone());
        }
    }
    if kept.group {
        meta.insert("group".into(), json!(true));
    }
    if let Some(name) = &kept.asked_by {
        meta.insert("asked_by".into(), json!(name));
    }
    let record = py::dumps(&Value::Object(meta.clone())) + "\n";
    for (name, text) in [
        (format!("{page}.source"), html.to_string()),
        (format!("{page}.html"), locked),
        (format!("{page}.json"), record),
    ] {
        tk::replace_with(&folder.join(format!(".{name}.part")), &folder.join(&name), text.as_bytes())?;
    }
    let mut out = Dict::new();
    out.insert("page".into(), json!(page));
    out.insert("path".into(), json!(media_path(agent, &page)));
    out.insert("outside".into(), json!(outside));
    out.extend(meta);
    Ok(out)
}

/// A page's place in the Carnet: linked to a Carnet page (`link`: its stable id and part, « main » or « health »),
/// and kept in the private health part (`private`: made by the agent the person chose for their health, or linked to
/// that part; once private, always). Nothing else of the page changes.
pub fn set_link(
    config: &Config,
    agent: &str,
    page: &str,
    link: Option<(&str, &str)>,
    private: bool,
) -> anyhow::Result<()> {
    let page = normalize(page);
    if !ID.is_match(&page) {
        return Err(unknown(&page));
    }
    let folder = root(config).join(agent);
    let path = meta_path(&folder, &page);
    let mut meta = read_meta(&path);
    if meta.is_empty() {
        return Err(unknown(&page));
    }
    if let Some((id, part)) = link {
        meta.insert("notebook".into(), json!({"id": id, "part": part}));
    }
    if private || link.is_some_and(|(_, part)| part == "health") {
        meta.insert("private".into(), json!(true));
    }
    let record = py::dumps(&Value::Object(meta)) + "\n";
    tk::replace_with(&folder.join(format!(".{page}.json.part")), &path, record.as_bytes())?;
    Ok(())
}

/// Whether a page belongs to the private health part (`private` in its record).
pub fn private(meta: &Dict) -> bool {
    meta.get("private").map(py::truthy).unwrap_or(false)
}

/// The Carnet page a page is linked to: (its id, its part), if any.
pub fn link(meta: &Dict) -> Option<(String, String)> {
    let n = meta.get("notebook")?;
    let id = tk::str_or_empty(n.get("id"));
    (!id.is_empty()).then(|| (id, tk::str_or_empty(n.get("part"))))
}

/// A `MEDIA:` path that names a page of this bridge (`/bipagents/pages/<agent>/<page>.html`) not kept here: a name
/// the model made up (9/11, routine reports on 63a8dec4 and after: `placeholder.html`, `x.html`… opened on a 404)
/// or a page deleted since. False for any other path (not the bridge's to say).
pub fn missing(config: &Config, path: &str) -> bool {
    let Some(rest) = path.strip_prefix(PREFIX).and_then(|r| r.strip_prefix('/')) else { return false };
    let Some((agent, file)) = rest.split_once('/') else { return true };
    let Some(page) = file.strip_suffix(".html") else { return false };
    let safe = |part: &str| !part.is_empty() && part != "." && part != ".." && !part.contains(['/', '\\', '\0']);
    !(safe(agent) && ID.is_match(page) && !page.ends_with('\n') && root(config).join(agent).join(file).is_file())
}

/// The agent's pages, the last changed first.
pub fn list_pages(config: &Config, agent: &str) -> Vec<Dict> {
    let folder = root(config).join(agent);
    let mut found = Vec::new();
    if folder.is_dir() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(page) = name.strip_suffix(".json").filter(|p| !p.is_empty()) else { continue };
            let meta = read_meta(&entry.path());
            if !meta.is_empty() && ID.is_match(page) && folder.join(format!("{page}.html")).is_file() {
                let mut item = Dict::new();
                item.insert("page".into(), json!(page));
                item.insert("path".into(), json!(media_path(agent, page)));
                for (k, v) in meta {
                    item.insert(k, v);
                }
                found.push(item);
            }
        }
    }
    // sorted(..., reverse=True) keeps equal keys in their order, as a stable sort on the reversed key does
    found.sort_by_key(|p| std::cmp::Reverse(tk::str_or_empty(p.get("updated"))));
    found
}

/// The maps (`m-<hex>`, maps.rs) a page of `agent` shows, by its `MEDIA:` path: its map cards' ids (`data-map`, as
/// `expand_maps` rebuilt them). Empty for anything else or a page not kept (made_files.rs: each map once in a reply).
pub fn maps_of(config: &Config, agent: &str, path: &str) -> Vec<String> {
    let Some(page) = path.strip_prefix(&format!("{PREFIX}/{agent}/")).and_then(|p| p.strip_suffix(".html")) else {
        return Vec::new();
    };
    if !ID.is_match(page) || page.ends_with('\n') {
        return Vec::new();
    }
    let Ok(html) = std::fs::read_to_string(root(config).join(agent).join(format!("{page}.html"))) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for found in MAP_ID.captures_iter(&html) {
        if !out.iter().any(|m| m == &found[1]) {
            out.push(found[1].to_string());
        }
    }
    out
}

pub fn source(config: &Config, agent: &str, page: &str) -> anyhow::Result<String> {
    let page = normalize(page);
    if !ID.is_match(&page) {
        return Err(unknown(&page));
    }
    tk::read_text(&root(config).join(agent).join(format!("{page}.source"))).map_err(|_| unknown(&page))
}

pub fn delete(config: &Config, agent: &str, page: &str) -> bool {
    let page = normalize(page);
    if !ID.is_match(&page) {
        return false;
    }
    let folder = root(config).join(agent);
    let mut removed = false;
    for suffix in [".html", ".source", ".json"] {
        let path = folder.join(format!("{page}{suffix}"));
        if path.is_file() && std::fs::remove_file(&path).is_ok() {
            removed = true;
        }
    }
    removed
}

/// A deleted agent: its pages go with it.
pub fn forget(config: &Config, agent: &str) {
    let folder = root(config).join(agent);
    let symlink = std::fs::symlink_metadata(&folder).map(|m| m.file_type().is_symlink()).unwrap_or(false);
    if folder.is_dir() && !symlink {
        let _ = std::fs::remove_dir_all(&folder);
    }
}

/// `datetime.fromisoformat(text).timestamp()` (a naive time is local, as Python takes it).
fn timestamp(text: &str) -> Option<f64> {
    let text = match text.strip_suffix('Z') {
        Some(t) => format!("{t}+00:00"),
        None => text.to_string(),
    };
    let utc = crate::store::fromisoformat(&text)?;
    let aware = text.len() > 10 && text.get(10..).map(|t| t.contains(['+', '-'])).unwrap_or(false);
    let mut seconds = utc.timestamp() as f64 + utc.timestamp_subsec_micros() as f64 / 1e6;
    if !aware {
        use chrono::{Local, Offset, TimeZone};
        let offset = Local.offset_from_utc_datetime(&utc.naive_utc()).fix().local_minus_utc();
        seconds -= offset as f64;
    }
    Some(seconds)
}

/// « 3 min », « 2 h », « 5 d » since the page last changed (for the agent's list).
pub fn age(meta: &Dict, now: Option<f64>) -> String {
    let updated = py::py_str(meta.get("updated").unwrap_or(&Value::Null));
    let Some(stamp) = timestamp(&updated) else { return "?".into() };
    let now = now.filter(|n| *n != 0.0).unwrap_or_else(py::time_time);
    let seconds = now - stamp;
    for (unit, size) in [("d", 86400.0), ("h", 3600.0), ("min", 60.0)] {
        if seconds >= size {
            return format!("{} {unit}", (seconds / size).floor() as i64);
        }
    }
    "now".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pages_maps_are_its_cards_ids() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        let folder = root(&config).join("vie");
        std::fs::create_dir_all(&folder).unwrap();
        let card = |id: &str| {
            format!("<figure class=\"map-card\" data-map=\"{id}\"><img src=\"data:image/png;base64,AA\"></figure>")
        };
        let html = format!("<h1>Trajet</h1>{}{}{}", card("m-0123456789"), card("m-aaaaaaaaaa"), card("m-0123456789"));
        std::fs::write(folder.join("trajet.html"), html).unwrap();
        assert_eq!(maps_of(&config, "vie", "/bipagents/pages/vie/trajet.html"), vec!["m-0123456789", "m-aaaaaaaaaa"]);
        // another agent's, another folder's, a page not kept, a name that is not a page's
        assert!(maps_of(&config, "lea", "/bipagents/pages/vie/trajet.html").is_empty());
        assert!(maps_of(&config, "vie", "/envoi/trajet.html").is_empty());
        assert!(maps_of(&config, "vie", "/bipagents/pages/vie/absente.html").is_empty());
        assert!(maps_of(&config, "vie", "/bipagents/pages/vie/../vie/trajet.html").is_empty());
    }

    #[test]
    fn a_page_the_model_named_without_making_it_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        let folder = root(&config).join("vie");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("point-du-jour.html"), "<h1>Point</h1>").unwrap();
        assert!(!missing(&config, "/bipagents/pages/vie/point-du-jour.html"));
        // 9/11, routine reports: names made up, never made
        for path in ["/bipagents/pages/vie/placeholder.html", "/bipagents/pages/vie/x.html", "/bipagents/pages/lea/point-du-jour.html",
                     "/bipagents/pages/vie/../vie/point-du-jour.html", "/bipagents/pages/x.html", "/bipagents/pages/vie/Point.html"] {
            assert!(missing(&config, path), "{path}");
        }
        // not the bridge's pages: not its to say
        for path in ["/envoi/x.html", "/bipagents/audio/vie/x.mp3", "/bipagents/pagesx/vie/x.html"] {
            assert!(!missing(&config, path), "{path}");
        }
    }

    #[test]
    fn slugs_and_ages() {
        assert_eq!(slug("Mon <b>titre</b>"), "mon-b-titre-b");
        assert_eq!(slug("!!!"), "page");
        let mut meta = Dict::new();
        meta.insert("updated".into(), json!("2026-10-21T07:00:00+00:00"));
        let t = timestamp("2026-10-21T07:00:00+00:00").unwrap();
        assert_eq!(age(&meta, Some(t + 30.0)), "now");
        assert_eq!(age(&meta, Some(t + 180.0)), "3 min");
        assert_eq!(age(&meta, Some(t + 7300.0)), "2 h");
        assert_eq!(age(&meta, Some(t + 5.0 * 86400.0)), "5 d");
        assert_eq!(age(&Dict::new(), None), "?");
    }

    #[test]
    fn a_page_made_for_a_relative_is_marked_and_keeps_its_mark() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        let mark = Mark { group: true, asked_by: Some("Marie <3".into()), lang: "fr".into() };
        let made = save_marked(&config, "cuisine", "<h1>Menu</h1>", "Menu", None, Some(&mark)).unwrap();
        let page = made["page"].as_str().unwrap().to_string();
        assert_eq!(made["group"], json!(true));
        assert_eq!(made["asked_by"], json!("Marie <3"));
        let html = std::fs::read_to_string(root(&config).join("cuisine").join(format!("{page}.html"))).unwrap();
        // the policy first, the group's mark right after it, the line first in the body (her name escaped)
        assert!(html.contains(&format!("{}{GROUP_META}", *HEAD)));
        let body = html.find("<body>").unwrap();
        assert!(html[body..].starts_with("<body><div class=\"bip-asked\""));
        assert!(html.contains("Faite à la demande de Marie &lt;3</div><h1>Menu</h1>"));
        // the owner changes it later: still marked, once
        let again = save(&config, "cuisine", "<h1>Menu v2</h1>", "Menu", Some(&page)).unwrap();
        assert_eq!((again["version"].as_i64(), again["group"].clone()), (Some(2), json!(true)));
        let html = std::fs::read_to_string(root(&config).join("cuisine").join(format!("{page}.html"))).unwrap();
        assert_eq!(html.matches(GROUP_META).count(), 1);
        assert_eq!(html.matches("bip-asked").count(), 1);
        // a page of the owner's own: no mark at all
        let own = save(&config, "cuisine", "<h1>Budget</h1>", "Budget", None).unwrap();
        assert!(own.get("group").is_none() && own.get("asked_by").is_none());
        let html = std::fs::read_to_string(root(&config).join("cuisine").join("budget.html")).unwrap();
        assert!(!html.contains(GROUP_META) && !html.contains("bip-asked"));
        // a whole document of the agent's: the line right after its own <body …>
        let doc = apply_mark("<html><body class=\"x\"><p>a</p></body></html>",
                             &Mark { group: false, asked_by: Some("Paul".into()), lang: "en".into() });
        assert!(doc.starts_with("<html><body class=\"x\"><div class=\"bip-asked\""));
        assert!(doc.contains("Made at Paul\u{2019}s request</div><p>a</p>"));
    }
}
