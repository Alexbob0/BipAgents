//! The styled documents skills (owner-approved 2026-11-02): `slides-styles` (PowerPoint decks, 10 styles) and
//! `docs-styles` (Word documents, 8 styles), from `assets/skills/`. The model writes only the content (JSON); the
//! skill's scripts do the layout with fonts every office suite has, native charts and tables, and real Word styles.
//! They replace Hermes' bundled `powerpoint` (whose 16:9 slides kept the 4:3 template's text boxes) and `docx`.
//!
//! What the bridge does, for every agent whose Hermes profile it reaches (`agent_tools::register`: at each start, so
//! a new bridge version brings the skills' new version, and when an agent's tools change):
//!
//! - writes both skills into the profile (`skills/slides-styles/`, `skills/docs-styles/`: Hermes mounts the profile's
//!   skills in the agent's sandbox at `/root/.hermes/skills`), only the files that changed; outside a Hermes category
//!   folder, so a kind's `skills.compact_categories` (tool_diet.rs) never hides them;
//! - with `defaults.json` and one line of SKILL.md naming the agent's usual style, from its kind (`defaults`): the
//!   agent still picks by subject and says which in one line, and `--style` changes it;
//! - on a place of the all-included offer, Hermes' `powerpoint` and `docx` are listed in the profile's
//!   `skills.disabled` and ours taken out of it, at every start;
//! - on a self-hosted install, the same once (the first time our skills land in that profile, `MARK`); after that the
//!   person's `skills.disabled` is left as they keep it, so they can go back to Hermes' skills (or turn ours off).
//!
//! The libraries (python-pptx, python-docx, lxml, Pillow) come with the agents' sandbox image (sandbox/Containerfile);
//! in a sandbox without them the scripts install them once, pinned (`scripts/_deps.py`).
//!
//! Not a Python module: Rust bridge only (owner group: tools-knowledge).

use std::path::Path;

use serde_json::json;

use crate::agents_admin::pyyaml::Yaml;
use crate::config::{AgentConfig, Config};

/// Our skills, as folders of `assets/skills/`.
pub const SKILLS: [&str; 2] = ["slides-styles", "docs-styles"];
/// Hermes' bundled skills they replace.
pub const REPLACED: [&str; 2] = ["powerpoint", "docx"];
/// In the profile's `skills/`: written once our skills were first set there (self-hosted: the person's choice from
/// then on).
pub const MARK: &str = ".bipagents-style-skills.json";
/// The line of each SKILL.md that names the agent's usual style.
const PLACEHOLDER: &str = "{{DEFAULT_STYLE}}";

/// The usual styles of a kind of agent (tool_diet::kind_of): (deck, Word document). Every deck style exists; the Word
/// styles are the deck styles but `carnet` and `bauhaus`.
pub fn defaults(kind: &str) -> (&'static str, &'static str) {
    match kind {
        // everyday life and the guide: trips, family events, letters
        "daily" | "guide" => ("editorial", "editorial"),
        "wellness" => ("maison", "editorial"),
        "finance" => ("rapport", "rapport"),
        "home" => ("artisan", "artisan"),
        "work" => ("suisse", "suisse"),
        "learning" => ("ecole", "ecole"),
        "creative" => ("bauhaus", "editorial"),
        "tech" => ("terminal", "terminal"),
        _ => ("suisse", "rapport"),
    }
}

/// A file of our skills, as the agent gets it: SKILL.md with its usual style named, `__pycache__` left out.
fn files_of(skill: &str, style: &str) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let Some(root) = crate::assets::SKILLS.get_dir(skill) else { return out };
    fn walk(dir: &include_dir::Dir<'static>, skill: &str, style: &str, out: &mut Vec<(String, Vec<u8>)>) {
        for file in dir.files() {
            let path = file.path().to_string_lossy().into_owned();
            let relative = path.strip_prefix(&format!("{skill}/")).unwrap_or(&path).to_string();
            if relative.contains("__pycache__") || relative.ends_with(".pyc") {
                continue;
            }
            let mut bytes = file.contents().to_vec();
            if relative == "SKILL.md" {
                bytes = String::from_utf8_lossy(&bytes).replace(PLACEHOLDER, style).into_bytes();
            }
            out.push((relative, bytes));
        }
        for sub in dir.dirs() {
            if !sub.path().to_string_lossy().contains("__pycache__") {
                walk(sub, skill, style, out);
            }
        }
    }
    walk(root, skill, style, &mut out);
    out.push(("defaults.json".into(), defaults_json(skill, style).into_bytes()));
    out.sort();
    out
}

fn defaults_json(skill: &str, style: &str) -> String {
    let key = if skill == "slides-styles" { "slides" } else { "docs" };
    format!("{}\n", json!({ key: style }))
}

/// Writes `bytes` at `path` when it differs; whether it did.
fn write_if_changed(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    if std::fs::read(path).is_ok_and(|current| current == bytes) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("bipagents-part");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(true)
}

/// The names in a config's `skills.disabled`.
fn disabled_of(data: &Yaml) -> Vec<String> {
    match data.get("skills").filter(|s| s.is_map()).and_then(|s| s.get("disabled")) {
        Some(Yaml::Seq(items)) => items.iter().map(Yaml::py_str).collect(),
        _ => Vec::new(),
    }
}

/// `data` with Hermes' skills off and ours on; None when it already is so.
pub fn with_ours_on(data: &Yaml) -> Option<Yaml> {
    let mut data = if data.is_map() { data.clone() } else { Yaml::map() };
    let before = disabled_of(&data);
    let mut off: Vec<String> = before.iter().filter(|n| !SKILLS.contains(&n.as_str())).cloned().collect();
    for name in REPLACED {
        if !off.iter().any(|n| n == name) {
            off.push(name.to_string());
        }
    }
    if off == before {
        return None;
    }
    let mut section = match data.get("skills") {
        Some(s @ Yaml::Map(_)) => s.clone(),
        _ => Yaml::map(),
    };
    section.insert("disabled", Yaml::Seq(off.into_iter().map(Yaml::str).collect()));
    data.insert("skills", section);
    Some(data)
}

/// What `apply` did to a profile.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Applied {
    /// Files of our skills written (new or changed).
    pub written: usize,
    /// `skills.disabled` changed.
    pub switched: bool,
    pub slides: &'static str,
    pub docs: &'static str,
}

/// Our skills in the profile `folder` of an agent of `kind`; `offer`: a place of the all-included offer.
pub fn apply_to(folder: &Path, kind: &str, offer: bool) -> anyhow::Result<Applied> {
    let (slides, docs) = defaults(kind);
    let skills = folder.join("skills");
    let first = !skills.join(MARK).exists();
    let mut written = 0;
    for (skill, style) in [("slides-styles", slides), ("docs-styles", docs)] {
        for (relative, bytes) in files_of(skill, style) {
            if write_if_changed(&skills.join(skill).join(&relative), &bytes)? {
                written += 1;
            }
        }
    }
    let mut switched = false;
    if offer || first {
        // the profile's one writer (profile_files.rs): under its lock, nothing written when already so
        switched = crate::profile_files::edit(&folder.join("config.yaml"), |data| {
            if let Some(changed) = with_ours_on(data) {
                *data = changed;
            }
        })?;
    }
    if first {
        write_if_changed(
            &skills.join(MARK),
            format!("{}\n", json!({"set": crate::py::time_time() as i64})).as_bytes(),
        )?;
    }
    Ok(Applied { written, switched, slides, docs })
}

/// The agent's: whether its profile changed (Hermes reads its skills again in its next conversation).
pub fn apply(config: &Config, agent: &AgentConfig) -> anyhow::Result<bool> {
    let Some(folder) = crate::shares::profile_dir(config, agent).filter(|f| f.join("config.yaml").is_file()) else {
        return Ok(false);
    };
    let soul = std::fs::read_to_string(folder.join("SOUL.md")).unwrap_or_default();
    let kind = crate::tool_diet::kind_of(config, agent, &soul);
    let done = apply_to(&folder, &kind, crate::handover::on_offer())?;
    if done.written > 0 || done.switched {
        tracing::info!(target: "bipbridge::agents", agent = %agent.name, kind = %kind, slides = done.slides,
            docs = done.docs, files = done.written, switched = done.switched, "styled documents skills set");
    }
    Ok(done.written > 0 || done.switched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents_admin::pyyaml;

    fn disabled(folder: &Path) -> Vec<String> {
        disabled_of(&pyyaml::load(&std::fs::read_to_string(folder.join("config.yaml")).unwrap()).unwrap())
    }

    fn profile(dir: &Path, config: &str) -> std::path::PathBuf {
        let folder = dir.join("profiles").join("bip");
        std::fs::create_dir_all(folder.join("skills/productivity/powerpoint")).unwrap();
        std::fs::write(folder.join("skills/productivity/powerpoint/SKILL.md"), "---\nname: powerpoint\n---\n").unwrap();
        std::fs::write(folder.join("config.yaml"), config).unwrap();
        folder
    }

    /// Both skills are embedded whole: their scripts, the fonts' metrics, no test or cache file.
    #[test]
    fn the_skills_are_embedded() {
        let slides: Vec<String> = files_of("slides-styles", "pop").into_iter().map(|(p, _)| p).collect();
        for want in [
            "SKILL.md",
            "defaults.json",
            "scripts/deck.py",
            "scripts/styles.py",
            "scripts/photo.py",
            "scripts/look.py",
            "scripts/metrics.json",
            "scripts/_deps.py",
        ] {
            assert!(slides.iter().any(|p| p == want), "{want} missing in {slides:?}");
        }
        assert!(!slides.iter().any(|p| p.contains("map_image")), "maps come from the bridge's map_image tool");
        let docs: Vec<String> = files_of("docs-styles", "rapport").into_iter().map(|(p, _)| p).collect();
        for want in ["SKILL.md", "defaults.json", "scripts/doc.py", "scripts/look.py", "scripts/_deps.py"] {
            assert!(docs.iter().any(|p| p == want), "{want} missing in {docs:?}");
        }
        assert!(slides.iter().chain(&docs).all(|p| !p.contains("__pycache__") && !p.contains("test")));
        // the bridge's own skills, never listed as the agent's (own_skills.rs)
        assert!(crate::own_skills::BRIDGE_SKILLS.contains("slides-styles"));
        assert!(crate::own_skills::BRIDGE_SKILLS.contains("docs-styles"));
    }

    /// Each kind's usual styles exist in the scripts' tables, and SKILL.md names them.
    #[test]
    fn every_kind_has_existing_default_styles() {
        let slides_table = crate::assets::text(&crate::assets::SKILLS, "slides-styles/scripts/styles.py").unwrap();
        let docs_table = crate::assets::text(&crate::assets::SKILLS, "docs-styles/scripts/doc.py").unwrap();
        for kind in crate::souls::CATEGORIES.iter().chain(&["guide", "custom"]) {
            let (slides, docs) = defaults(kind);
            assert!(slides_table.contains(&format!("    \"{slides}\": dict(")), "{kind}: {slides}");
            assert!(docs_table.contains(&format!("    \"{docs}\": dict(")), "{kind}: {docs}");
        }
        let skill = files_of("slides-styles", "terminal").into_iter().find(|(p, _)| p == "SKILL.md").unwrap().1;
        let skill = String::from_utf8(skill).unwrap();
        assert!(skill.contains("**terminal**") && !skill.contains(PLACEHOLDER));
        let docs = files_of("docs-styles", "ecole").into_iter().find(|(p, _)| p == "SKILL.md").unwrap().1;
        assert!(String::from_utf8(docs).unwrap().contains("**ecole**"));
    }

    /// On a place of the offer: our skills written for every agent, Hermes' `powerpoint` and `docx` off, ours on,
    /// at every start (whatever was changed in between), the rest of the config kept; nothing rewritten when nothing
    /// changed.
    #[test]
    fn offer_places_replace_hermes_skills() {
        let dir = tempfile::tempdir().unwrap();
        let folder = profile(dir.path(), "model:\n  provider: custom\nskills:\n  disabled: [arxiv, slides-styles]\n");
        let done = apply_to(&folder, "tech", true).unwrap();
        assert!(done.written >= 10 && done.switched);
        assert_eq!(disabled(&folder), ["arxiv", "powerpoint", "docx"]);
        let skill = std::fs::read_to_string(folder.join("skills/slides-styles/SKILL.md")).unwrap();
        assert!(skill.contains("/root/.hermes/skills/slides-styles/scripts/") && skill.contains("**terminal**"));
        assert_eq!(
            std::fs::read_to_string(folder.join("skills/slides-styles/defaults.json")).unwrap(),
            "{\"slides\":\"terminal\"}\n"
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("skills/docs-styles/defaults.json")).unwrap(),
            "{\"docs\":\"terminal\"}\n"
        );
        let data = pyyaml::load(&std::fs::read_to_string(folder.join("config.yaml")).unwrap()).unwrap();
        assert_eq!(pyyaml::to_json(data.get("model").unwrap()), json!({"provider": "custom"}));
        // the next start: nothing to do
        assert_eq!(
            apply_to(&folder, "tech", true).unwrap(),
            Applied { slides: "terminal", docs: "terminal", ..Default::default() }
        );
        // turned back by hand: put right again on the offer
        std::fs::write(folder.join("config.yaml"), "skills:\n  disabled: [docs-styles]\n").unwrap();
        assert!(apply_to(&folder, "tech", true).unwrap().switched);
        assert_eq!(disabled(&folder), ["powerpoint", "docx"]);
        // the kind changed: its usual styles follow
        let done = apply_to(&folder, "finance", true).unwrap();
        assert_eq!((done.slides, done.docs, done.written), ("rapport", "rapport", 4));
    }

    /// Self-hosted: the same the first time, then the person's choice is kept (back to Hermes' skills, or ours off).
    #[test]
    fn self_hosted_installs_keep_the_persons_choice() {
        let dir = tempfile::tempdir().unwrap();
        let folder = profile(dir.path(), "skills:\n  disabled: []\n");
        assert!(apply_to(&folder, "daily", false).unwrap().switched);
        assert_eq!(disabled(&folder), ["powerpoint", "docx"]);
        assert!(folder.join("skills").join(MARK).exists());
        std::fs::write(folder.join("config.yaml"), "skills:\n  disabled: [slides-styles]\n").unwrap();
        let done = apply_to(&folder, "daily", false).unwrap();
        assert!(!done.switched);
        assert_eq!(disabled(&folder), ["slides-styles"]);
        assert!(folder.join("skills/slides-styles/scripts/deck.py").exists()); // kept up to date all the same
    }
}
