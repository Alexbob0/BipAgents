//! Words a voice says wrong, respelled by the person (`[pronunciations.<lang>]` in the bridge's config):
//!
//!     [pronunciations.fr]
//!     Garmin = "Garmine"
//!     "Mme Dupuis" = "Madame Dupuis"
//!
//! Whole words or phrases, any case, applied before the bridge's own lexicon (`pronounce`, assets/pronunciation/), so
//! they win over it.
//!
//! Port of `bridge/bipbridge/pronunciations.py` (owner group: voice-speech).

use indexmap::IndexMap;

use crate::voice_speech::pyre::{escape, Regex, IGNORECASE};
use crate::voice_speech::pystr;

/// Respellings by language (`[pronunciations.<lang>]`).
#[derive(Debug, Clone, Default)]
pub struct Lexicon {
    pub words: IndexMap<String, IndexMap<String, String>>,
    /// Per language: (whole word or phrase, any case) -> its spelling, the longest first.
    rules: IndexMap<String, Vec<(Regex, String)>>,
}

impl Lexicon {
    pub fn new(words: &IndexMap<String, IndexMap<String, String>>) -> Self {
        let rules = words
            .iter()
            .map(|(lang, table)| {
                let mut entries: Vec<(&String, &String)> = table.iter().collect();
                // sorted(key=-len(word)): stable, so equal lengths keep the file's order
                entries.sort_by_key(|(word, _)| std::cmp::Reverse(pystr::len(word)));
                let compiled = entries
                    .into_iter()
                    .map(|(word, spoken)| {
                        (Regex::with_flags(&format!(r"(?<!\w){}(?!\w)", escape(word)), IGNORECASE), spoken.clone())
                    })
                    .collect();
                (lang.clone(), compiled)
            })
            .collect();
        Lexicon { words: words.clone(), rules }
    }

    pub fn apply(&self, text: &str, language: &str) -> String {
        let mut text = text.to_string();
        for (pattern, spoken) in self.rules.get(language).map(Vec::as_slice).unwrap_or(&[]) {
            text = pattern.replace_with(&text, |_| spoken.clone());
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_words_any_case_longest_first() {
        let mut fr = IndexMap::new();
        fr.insert("Garmin".to_string(), "Garmine".to_string());
        fr.insert("Mme Dupuis".to_string(), "Madame Dupuis".to_string());
        fr.insert("Mme".to_string(), "Mâdame".to_string());
        let mut words = IndexMap::new();
        words.insert("fr".to_string(), fr);
        let lexicon = Lexicon::new(&words);
        assert_eq!(
            lexicon.apply("ta GARMIN et Mme Dupuis, Mme X, garminette", "fr"),
            "ta Garmine et Madame Dupuis, Mâdame X, garminette"
        );
        assert_eq!(lexicon.apply("your Garmin", "en"), "your Garmin");
    }
}
