//! The skills an agent wrote itself (Hermes' `skill_manage`: a « remember how to… », or its own review after a task),
//! listed in the app's advanced tools, where each one can be turned off. Hermes keeps an agent's skills in its profile's
//! `skills/` folder, next to the ones it ships (`.bundled_manifest`), the ones installed from its hub (`.hub/lock.json`)
//! and the ones this bridge gives (shared data, the visible browser, contacts): those are not the agent's own. Turning
//! one off lists it in the profile's `skills.disabled`, which Hermes reads again when the profile's config changes.
//!
//! Port of `bridge/bipbridge/own_skills.py` (owner group: agents).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use indexmap::IndexMap;
use serde_json::{json, Value};

use crate::agents_admin::pyyaml::{self, Yaml};
use crate::agents_admin::LookupError;
use crate::Dict;

/// The skills this bridge gives (its package's `skills/` folders) and the contacts list (peers.py).
pub static BRIDGE_SKILLS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    let mut names: HashSet<String> = crate::assets::SKILLS
        .dirs()
        .filter_map(|d| d.path().file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    names.insert("agent-contacts".to_string());
    names
});

fn frontmatter(path: &Path) -> Yaml {
    let Ok(text) = std::fs::read_to_string(path) else { return Yaml::map() };
    let end =
        if text.starts_with("---") { text.get(3..).and_then(|rest| rest.find("\n---")).map(|i| i + 3) } else { None };
    let Some(end) = end else { return Yaml::map() };
    match pyyaml::load(&text[3..end]) {
        Ok(data @ Yaml::Map(_)) => data,
        _ => Yaml::map(),
    }
}

fn bundled(skills: &Path) -> HashSet<String> {
    let Ok(text) = std::fs::read_to_string(skills.join(".bundled_manifest")) else { return HashSet::new() };
    crate::ssh_keys::splitlines(&text)
        .into_iter()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|line| line.split(':').next().unwrap_or("").trim().to_string())
        .collect()
}

fn from_hub(skills: &Path) -> HashSet<String> {
    let Ok(text) = std::fs::read_to_string(skills.join(".hub").join("lock.json")) else { return HashSet::new() };
    let Ok(data) = serde_json::from_str::<Value>(&text) else { return HashSet::new() };
    let found = match &data {
        Value::Object(map) => map.get("installed").or_else(|| map.get("skills")).unwrap_or(&data),
        other => other,
    };
    match found {
        Value::Object(map) => map.keys().cloned().collect(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_object())
            .filter_map(|item| item.get("name").filter(|n| crate::py::truthy(n)).map(crate::py::py_str))
            .collect(),
        _ => HashSet::new(),
    }
}

/// The profile's config.yaml as a dict (empty when missing or not YAML).
fn config(profile: &Path) -> Yaml {
    let Ok(text) = std::fs::read_to_string(profile.join("config.yaml")) else { return Yaml::map() };
    match pyyaml::load(&text) {
        Ok(data @ Yaml::Map(_)) => data,
        Ok(data) if !data.truthy() => Yaml::map(),
        _ => Yaml::map(),
    }
}

fn disabled(data: &Yaml) -> Vec<String> {
    match data.get("skills").filter(|s| s.is_map()).and_then(|s| s.get("disabled")) {
        Some(Yaml::Seq(items)) => items.iter().map(Yaml::py_str).collect(),
        _ => Vec::new(),
    }
}

/// Every `SKILL.md` under `skills/`, as `rglob` finds them (folders that are links are not entered), with the
/// parts of its path below `skills/`, sorted as Python sorts paths.
fn skill_files(skills: &Path) -> Vec<(Vec<String>, PathBuf)> {
    fn walk(folder: &Path, parts: &[String], out: &mut Vec<(Vec<String>, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(folder) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            let mut here = parts.to_vec();
            here.push(name.clone());
            let is_link = entry.file_type().map(|t| t.is_symlink()).unwrap_or(false);
            if name == "SKILL.md" {
                out.push((here.clone(), path.clone()));
            }
            if !is_link && path.is_dir() {
                walk(&path, &here, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(skills, &[], &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The agent's own skills, from its profile folder: name, description, enabled.
pub fn listing(profile: &Path) -> Vec<Dict> {
    let skills = profile.join("skills");
    if !skills.is_dir() {
        return Vec::new();
    }
    let mut others = bundled(&skills);
    others.extend(from_hub(&skills));
    others.extend(BRIDGE_SKILLS.iter().cloned());
    let off: BTreeSet<String> = disabled(&config(profile)).into_iter().collect();
    let mut found: IndexMap<String, Dict> = IndexMap::new();
    for (parts, skill_md) in skill_files(&skills) {
        if parts.iter().any(|p| p.starts_with('.')) {
            continue; // .archive, .hub, pending writes
        }
        let inside_another = (1..parts.len().saturating_sub(1)).any(|depth| {
            let mut ancestor = skills.clone();
            for p in &parts[..depth] {
                ancestor.push(p);
            }
            ancestor.join("SKILL.md").exists()
        });
        if inside_another {
            continue; // a file inside another skill's folder
        }
        let folder =
            skill_md.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let meta = frontmatter(&skill_md);
        let name = match meta.get("name").filter(|n| n.truthy()) {
            Some(n) => n.py_str(),
            None => folder.clone(),
        };
        let name = name.trim().to_string();
        if name.is_empty() || others.contains(&folder) || others.contains(&name) {
            continue;
        }
        let description = meta.get("description").filter(|d| d.truthy()).map(Yaml::py_str).unwrap_or_default();
        let description: String =
            description.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(300).collect();
        let mut entry = Dict::new();
        entry.insert("name".into(), json!(name));
        entry.insert("description".into(), json!(description));
        entry.insert("enabled".into(), json!(!off.contains(&name)));
        found.insert(name, entry);
    }
    let mut out: Vec<Dict> = found.into_values().collect();
    out.sort_by_key(|skill| skill.get("name").and_then(Value::as_str).unwrap_or("").to_lowercase());
    out
}

/// Turns some of the agent's own skills on or off (others, Hermes' or the hub's, are not this switch's): an
/// unknown name is Python's `LookupError` (« a, b »).
pub fn set_enabled(profile: &Path, changes: &IndexMap<String, bool>) -> anyhow::Result<()> {
    let own: HashSet<String> =
        listing(profile).iter().filter_map(|s| s.get("name").and_then(Value::as_str).map(str::to_string)).collect();
    let unknown: BTreeSet<&String> = changes.keys().filter(|k| !own.contains(*k)).collect();
    if !unknown.is_empty() {
        return Err(LookupError(unknown.into_iter().cloned().collect::<Vec<_>>().join(", ")).into());
    }
    let mut data = config(profile);
    let mut off: Vec<String> = disabled(&data).into_iter().filter(|name| changes.get(name) != Some(&true)).collect();
    let sorted: BTreeMap<&String, &bool> = changes.iter().collect();
    for (name, on) in sorted {
        if !*on && !off.contains(name) {
            off.push(name.clone());
        }
    }
    let mut section = match data.get("skills") {
        Some(s @ Yaml::Map(_)) => s.clone(),
        _ => Yaml::map(),
    };
    section.insert("disabled", Yaml::Seq(off.into_iter().map(Yaml::Str).collect()));
    data.insert("skills", section);
    std::fs::write(profile.join("config.yaml"), pyyaml::dump(&data))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(folder: &Path, name: &str, description: &str, frontmatter: bool) {
        std::fs::create_dir_all(folder).unwrap();
        let head =
            if frontmatter { format!("---\nname: {name}\ndescription: {description}\n---\n") } else { String::new() };
        std::fs::write(folder.join("SKILL.md"), format!("{head}# Steps\n")).unwrap();
    }

    pub(crate) fn make_profile(tmp: &Path) -> PathBuf {
        let profile = tmp.join("profiles").join("budget");
        let skills = profile.join("skills");
        skill(&skills.join("productivity").join("arxiv"), "arxiv", "Search papers", true); // shipped with Hermes
        skill(&skills.join("hub-thing"), "hub-thing", "Does a thing", true); // from the hub
        skill(&skills.join("shared-data"), "shared-data", "Does a thing", true); // the bridge's
        skill(&skills.join("finance").join("monthly-budget"), "monthly-budget", "Sums  the\n month's spending", true);
        skill(&skills.join("plant-care"), "plant-care", "Waters the ficus", true);
        skill(&skills.join("plant-care").join("templates").join("nested"), "nested", "Does a thing", true); // inside a skill
        skill(&skills.join(".archive").join("old"), "old", "Does a thing", true); // archived
        skill(&skills.join("no-front"), "x", "Does a thing", false); // name from its folder
        std::fs::write(skills.join(".bundled_manifest"), "arxiv:abc123\n").unwrap();
        std::fs::create_dir_all(skills.join(".hub")).unwrap();
        std::fs::write(skills.join(".hub").join("lock.json"), r#"{"installed": {"hub-thing": {"source": "hub"}}}"#)
            .unwrap();
        std::fs::write(
            profile.join("config.yaml"),
            "model:\n  provider: custom\nskills:\n  disabled: [plant-care, arxiv]\n",
        )
        .unwrap();
        profile
    }

    /// test_own_skills.py: only the agent's own skills are listed.
    #[test]
    fn only_the_agents_own_skills_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        let found = listing(&make_profile(dir.path()));
        let names: Vec<&str> = found.iter().map(|s| s["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["monthly-budget", "no-front", "plant-care"]);
        assert_eq!(
            Value::Object(found[0].clone()),
            json!({"name": "monthly-budget", "description": "Sums the month's spending", "enabled": true})
        );
        assert_eq!(found[2]["enabled"], false);
        assert!(listing(&dir.path().join("nowhere")).is_empty());
    }

    /// test_own_skills.py: turning skills on and off keeps the rest of the config.
    #[test]
    fn turning_skills_on_and_off_keeps_the_rest_of_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let profile = make_profile(dir.path());
        let changes: IndexMap<String, bool> =
            [("plant-care".to_string(), true), ("monthly-budget".to_string(), false)].into_iter().collect();
        set_enabled(&profile, &changes).unwrap();
        let data = pyyaml::load(&std::fs::read_to_string(profile.join("config.yaml")).unwrap()).unwrap();
        assert_eq!(pyyaml::to_json(data.get("model").unwrap()), json!({"provider": "custom"}));
        assert_eq!(
            pyyaml::to_json(data.get("skills").unwrap().get("disabled").unwrap()),
            json!(["arxiv", "monthly-budget"])
        );
        let enabled: Vec<(String, bool)> = listing(&profile)
            .iter()
            .map(|s| (s["name"].as_str().unwrap().to_string(), s["enabled"].as_bool().unwrap()))
            .collect();
        assert_eq!(
            enabled,
            [("monthly-budget".to_string(), false), ("no-front".to_string(), true), ("plant-care".to_string(), true)]
        );
        let not_ours: IndexMap<String, bool> = [("arxiv".to_string(), true)].into_iter().collect();
        let err = set_enabled(&profile, &not_ours).unwrap_err();
        assert!(err.downcast_ref::<LookupError>().is_some()); // not the agent's: not this switch's
    }
}
