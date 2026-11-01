//! Accents written escaped in the Carnet, put back (2026-11-01). Until then a page's frontmatter quoted its values with
//! ASCII-only JSON (an alias « Résidence Les Pins, bât. A » was written `"R\u00e9sidence Les Pins, b\u00e2t. A"`), and
//! `notebook_read` gives the agents the page's raw text: a model reading `\u00e9` took it for a broken encoding, and
//! some copied the escapes into the facts they wrote (as with Hermes' routine results, patch 0014). Alex then scheduled
//! « corrige tout le Carnet » for an agent to fix page after page.
//!
//! What can be put back without a model is what was escaped: `\uXXXX` standing for a letter or a sign beyond ASCII
//! (a surrogate pair too) becomes that character, in the frontmatter (still valid YAML: a double-quoted scalar may
//! hold any character) and in the facts. Nothing else is touched: an ASCII escape (`\u0041`, as in a snippet of code),
//! a lone surrogate, the person's own straight or curly apostrophes (both are right, and `fold` reads them the same).
//! Words a model wrote without their accents (« Copropriete ») cannot be put back by rule; that stays an agent's job.
//!
//! Not run by itself: `repaired` and `plan` are read-only, for a route or a one-off command to call on a Carnet the
//! person chose to repair (the page's history keeps the earlier text).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::{Captures, Regex};

/// `\uXXXX`, maybe a high surrogate followed by its low one.
static ESCAPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\u([0-9a-fA-F]{4})(?:\\u([dD][c-fC-F][0-9a-fA-F]{2}))?").expect("valid regex"));

/// The text with its escaped accents put back; None when there is nothing to put back.
pub fn repaired(text: &str) -> Option<String> {
    let mut changed = false;
    let out = ESCAPE.replace_all(text, |c: &Captures| {
        let high = u32::from_str_radix(&c[1], 16).unwrap_or(0);
        let low = c.get(2).and_then(|l| u32::from_str_radix(l.as_str(), 16).ok());
        let code = match (high, low) {
            (0xD800..=0xDBFF, Some(low)) => 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00),
            (_, None) => high,
            // a code unit, then a low surrogate that is not its pair: the first alone, the second as written
            (_, Some(_)) => {
                return match char::from_u32(high).filter(|ch| !ch.is_ascii() && !ch.is_control()) {
                    Some(ch) => {
                        changed = true;
                        format!("{ch}{}", &c[0][6..])
                    }
                    None => c[0].to_string(),
                };
            }
        };
        match char::from_u32(code).filter(|ch| !ch.is_ascii() && !ch.is_control()) {
            Some(ch) => {
                changed = true;
                ch.to_string()
            }
            None => c[0].to_string(),
        }
    });
    changed.then(|| out.into_owned())
}

/// The Carnet's pages (`.md` under `root`) whose text would change, with their repaired text. Reads only.
pub fn plan(root: &Path) -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            let hidden = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'));
            if hidden || path.is_symlink() {
                continue; // the history's repository, Obsidian's settings; never a link out of the Carnet
            }
            if path.is_dir() {
                folders.push(path);
            } else if path.extension().is_some_and(|e| e == "md") {
                if let Some(text) = std::fs::read_to_string(&path).ok().as_deref().and_then(repaired) {
                    found.push((path, text));
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaped_accents_are_put_back() {
        let page = "---\naliases:\n  - \"R\\u00e9sidence Les Pins, b\\u00e2t. A\"\n---\n# Copropri\\u00e9t\\u00e9\n\n\
- L\\u2019assembl\\u00e9e g\\u00e9n\\u00e9rale a lieu le 12 \\u2014 salle \\u00ab B \\u00bb \\ud83c\\udfe0\n";
        let fixed = repaired(page).unwrap();
        assert_eq!(
            fixed,
            "---\naliases:\n  - \"Résidence Les Pins, bât. A\"\n---\n# Copropriété\n\n\
- L’assemblée générale a lieu le 12 — salle « B » 🏠\n"
        );
        // still the same frontmatter for YAML
        let yaml = |t: &str| serde_yaml::from_str::<serde_json::Value>(t.split("---\n").nth(1).unwrap()).unwrap();
        assert_eq!(yaml(page), yaml(&fixed));
        assert_eq!(repaired(&fixed), None, "once is enough");
    }

    #[test]
    fn a_page_is_now_written_with_its_accents() {
        let mut page = super::super::Page::new("subject", "Copropriété");
        page.front.insert("aliases".into(), serde_json::json!(["Résidence Les Pins, bât. A", "L’immeuble"]));
        let text = page.render();
        assert!(text.contains("  - \"Résidence Les Pins, bât. A\"\n") && text.contains("  - L’immeuble\n"), "{text}");
        assert!(!text.contains("\\u"), "{text}");
        assert_eq!(super::super::Page::parse(&text, "x").front["aliases"], page.front["aliases"]);
        assert_eq!(repaired(&text), None);
    }

    #[test]
    fn nothing_else_is_touched() {
        for text in [
            "Aujourd'hui, l’agence a écrit.", // straight and curly apostrophes: both right
            "print(\"\\u0041\")",             // an ASCII escape, as in code
            "\\u000a and \\u0009",            // control characters
            "\\ud83c alone",                  // a lone surrogate
            "Copropriete",                    // accents dropped: not by rule
            "\\uZZZZ",
        ] {
            assert_eq!(repaired(text), None, "{text}");
        }
        assert_eq!(repaired("caf\\u00e9 \\u0041").as_deref(), Some("café \\u0041"));
        assert_eq!(repaired("\\u00e9\\udc00").as_deref(), Some("é\\udc00"), "a stray low surrogate stays as written");
    }

    #[test]
    fn the_plan_lists_the_pages_to_repair_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let carnet = dir.path().join("carnet");
        std::fs::create_dir_all(carnet.join("Personnes")).unwrap();
        std::fs::create_dir_all(carnet.join(".git")).unwrap();
        std::fs::write(carnet.join("Personnes").join("Kévin.md"), "# K\\u00e9vin\n").unwrap();
        std::fs::write(carnet.join("Maison.md"), "# Maison\n- Chaudière révisée\n").unwrap();
        std::fs::write(carnet.join(".git").join("x.md"), "# \\u00e9").unwrap();
        std::fs::write(carnet.join("notes.txt"), "\\u00e9").unwrap();
        let plan = plan(&carnet);
        assert_eq!(plan, vec![(carnet.join("Personnes").join("Kévin.md"), "# Kévin\n".to_string())]);
        assert_eq!(std::fs::read_to_string(carnet.join("Personnes").join("Kévin.md")).unwrap(), "# K\\u00e9vin\n");
    }
}
