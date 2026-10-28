//! The scrubber of « Partager comme modèle » (internal/next/agent-templates.md §2): before a model of an agent is
//! shared, what looks personal or secret in its texts and code is flagged for the person, who confirms or removes the
//! part. It never changes the text and never blocks: it points.
//!
//! Flagged: e-mail addresses, phone numbers, IBANs, postal addresses (a number and a street word), API-key-like
//! strings (known prefixes, JWTs, long random tokens) and URLs that carry a token (`?token=`, `key=`, `sig=`… or
//! `user:pass@`). Each flag: its kind, the line, a masked excerpt.
//!
//! No Python counterpart.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

/// At most this many flags per text (a file full of addresses says so with the first ones).
const PER_TEXT: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Flag {
    pub kind: &'static str,
    pub line: usize,
    pub excerpt: String,
}

pub fn label(kind: &str) -> &'static str {
    match kind {
        "email" => "Une adresse e-mail",
        "phone" => "Un numéro de téléphone",
        "iban" => "Un IBAN",
        "address" => "Une adresse postale",
        "key" => "Une clé ou un mot de passe",
        "url_token" => "Un lien avec un jeton",
        _ => "À vérifier",
    }
}

static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b[a-z0-9._%+-]+@[a-z0-9-]+(?:\.[a-z0-9-]+)*\.[a-z]{2,}\b").unwrap());
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)\b[a-z][a-z0-9+.-]*://[^\s"'<>()\[\]{}]+"#).unwrap());
static URL_TOKEN_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[?&#;](?:access_?token|token|api_?key|apikey|key|secret|sig|signature|password|passwd|pwd|auth|code|session|sid)=[^&\s]{6,}")
        .unwrap()
});
static URL_USERINFO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[a-z][a-z0-9+.-]*://[^/@\s:]+:[^/@\s]+@").unwrap());
static PHONE: LazyLock<Regex> = LazyLock::new(|| {
    // +33 6 12 34 56 78, 06 12 34 56 78, 06.12.34.56.78, (555) 123-4567, +1-555-123-4567
    Regex::new(r"(?:\+\d{1,3}[\s.-]?)?(?:\(\d{1,4}\)[\s.-]?)?\d{1,4}(?:[\s.-]\d{2,4}){2,5}|\+\d{8,14}|\b0\d{9}\b")
        .unwrap()
});
static IBAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Z]{2}\d{2}(?:[ ]?[A-Z0-9]{4}){2,7}(?:[ ]?[A-Z0-9]{1,4})?\b").unwrap());
static ADDRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b\d{1,5}(?:\s?(?:bis|ter|[a-c]))?,?\s+(?:rue|avenue|av\.|boulevard|bd|chemin|impasse|allée|allee|place|quai|route|cours|square|passage|street|st\.|road|rd\.|lane|drive|dr\.|way|court|straße|strasse|str\.|weg|gasse|platz|calle|avenida|plaza|paseo|camino)\s+[\p{L}'’ -]{2,}",
    )
    .unwrap()
});
static KEY_PREFIXED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:sk-[A-Za-z0-9_-]{16,}|sk_(?:live|test)_[A-Za-z0-9]{10,}|pk_(?:live|test)_[A-Za-z0-9]{10,}|rk_live_[A-Za-z0-9]{10,}|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|xox[abprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{30,}|glpat-[A-Za-z0-9_-]{16,}|hf_[A-Za-z0-9]{20,}|eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,})",
    )
    .unwrap()
});
static KEY_ASSIGNED: LazyLock<Regex> = LazyLock::new(|| {
    // password = "…", api_key: '…', TOKEN="…" (a literal value, not a variable read from the environment)
    Regex::new(r#"(?i)\b(?:password|passwd|pwd|secret|api_?key|apikey|token|access_?token|client_?secret|mot_de_passe)\b\s*[:=]\s*["']([^"'\s]{6,})["']"#)
        .unwrap()
});
static LONG_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[A-Za-z0-9_\-+/]{32,}={0,2}").unwrap());

/// Masks the middle: « al…@ex… »-like, so the excerpt says what was found without spreading it.
fn mask(found: &str) -> String {
    let chars: Vec<char> = found.chars().collect();
    if let Some(at) = found.find('@').filter(|_| !found.contains("://")) {
        let (user, domain) = found.split_at(at);
        let keep = |s: &str| s.chars().take(2).collect::<String>();
        return format!("{}…@{}…", keep(user), keep(&domain[1..]));
    }
    if chars.len() <= 6 {
        return "…".into();
    }
    let head: String = chars.iter().take(4.min(chars.len() / 3)).collect();
    let tail: String = chars[chars.len() - 2..].iter().collect();
    format!("{head}…{tail}")
}

fn digits(text: &str) -> usize {
    text.chars().filter(char::is_ascii_digit).count()
}

/// The IBAN's checksum (ISO 13616: mod 97 = 1).
fn iban_ok(text: &str) -> bool {
    let raw: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if !(15..=34).contains(&raw.len()) {
        return false;
    }
    let moved = format!("{}{}", &raw[4..], &raw[..4]);
    let mut rest: u64 = 0;
    for c in moved.chars() {
        let value = match c {
            '0'..='9' => c as u64 - '0' as u64,
            'A'..='Z' => c as u64 - 'A' as u64 + 10,
            _ => return false,
        };
        rest = if value >= 10 { (rest * 100 + value) % 97 } else { (rest * 10 + value) % 97 };
    }
    rest == 1
}

/// A long token that is random-looking: letters and digits mixed, high entropy, not a path or a word.
fn random_looking(token: &str) -> bool {
    if token.contains('/') && token.split('/').filter(|p| !p.is_empty()).count() > 2 {
        return false; // a path
    }
    let has_digit = token.chars().any(|c| c.is_ascii_digit());
    let has_upper = token.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = token.chars().any(|c| c.is_ascii_lowercase());
    if !(has_digit && has_lower && (has_upper || token.len() >= 40)) {
        return false;
    }
    if token.chars().all(|c| c.is_ascii_hexdigit()) && matches!(token.len(), 40 | 64) {
        return false; // a commit or a SHA-256 written in code
    }
    let mut counts = std::collections::HashMap::new();
    for c in token.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
    }
    let n = token.chars().count() as f64;
    let entropy: f64 = counts
        .values()
        .map(|&k| {
            let p = k as f64 / n;
            -p * p.log2()
        })
        .sum();
    entropy > 4.0
}

/// The flags of one text.
pub fn scan(text: &str) -> Vec<Flag> {
    let mut out: Vec<Flag> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        // a plain link: never the e-mail or the token rules inside it
        let mut taken: Vec<(usize, usize)> = URL
            .find_iter(line)
            .filter(|m| !(URL_TOKEN_KEY.is_match(m.as_str()) || URL_USERINFO.is_match(m.as_str())))
            .map(|m| (m.start(), m.end()))
            .collect();
        let mut push = |kind: &'static str, start: usize, end: usize, found: &str, out: &mut Vec<Flag>| {
            if taken.iter().any(|(s, e)| start < *e && *s < end) {
                return;
            }
            taken.push((start, end));
            out.push(Flag { kind, line: n + 1, excerpt: mask(found) });
        };
        for m in URL.find_iter(line) {
            let url = m.as_str();
            if URL_TOKEN_KEY.is_match(url) || URL_USERINFO.is_match(url) {
                push("url_token", m.start(), m.end(), url, &mut out);
            }
        }
        for m in KEY_PREFIXED.find_iter(line) {
            push("key", m.start(), m.end(), m.as_str(), &mut out);
        }
        for c in KEY_ASSIGNED.captures_iter(line) {
            let value = c.get(1).expect("a group");
            let v = value.as_str();
            if v.starts_with('$') || v.starts_with('{') || v.starts_with('<') || v.to_lowercase().contains("environ") {
                continue;
            }
            push("key", value.start(), value.end(), v, &mut out);
        }
        for m in EMAIL.find_iter(line) {
            let found = m.as_str().to_lowercase();
            // placeholders the person can't mean as theirs
            if found.ends_with("@example.com") || found.ends_with("@example.org") || found.ends_with("@example.net") {
                continue;
            }
            push("email", m.start(), m.end(), m.as_str(), &mut out);
        }
        for m in IBAN.find_iter(line) {
            if iban_ok(m.as_str()) {
                push("iban", m.start(), m.end(), m.as_str(), &mut out);
            }
        }
        for m in ADDRESS.find_iter(line) {
            push("address", m.start(), m.end(), m.as_str(), &mut out);
        }
        for m in PHONE.find_iter(line) {
            let found = m.as_str();
            let d = digits(found);
            // a phone: 9 to 15 digits; never a time, a date or a version written with separators
            if !(9..=15).contains(&d) || found.contains(':') {
                continue;
            }
            let looks_date = found
                .split(['.', '-', '/', ' '])
                .filter(|p| !p.is_empty())
                .any(|p| p.len() == 4 && p.starts_with("20"));
            if looks_date && !found.starts_with('+') && !found.starts_with('0') {
                continue;
            }
            push("phone", m.start(), m.end(), found.trim(), &mut out);
        }
        for m in LONG_TOKEN.find_iter(line) {
            if random_looking(m.as_str()) {
                push("key", m.start(), m.end(), m.as_str(), &mut out);
            }
        }
        if out.len() >= PER_TEXT {
            out.truncate(PER_TEXT);
            break;
        }
    }
    out
}

/// The flags of one part's text as the export lists them.
pub fn flags_json(part: &str, file: Option<&str>, text: &str) -> Vec<Value> {
    scan(text)
        .into_iter()
        .map(|f| {
            json!({"part": part, "file": file, "line": f.line, "kind": f.kind, "label": label(f.kind),
                   "excerpt": f.excerpt})
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<&'static str> {
        scan(text).into_iter().map(|f| f.kind).collect()
    }

    #[test]
    fn personal_and_secret_things_are_flagged() {
        assert_eq!(kinds("Écris à camille.durand@laposte.net si besoin"), ["email"]);
        assert_eq!(kinds("Appelle le 06 12 34 56 78 le soir"), ["phone"]);
        assert_eq!(kinds("call +1-555-123-4567"), ["phone"]);
        assert_eq!(kinds("virement sur FR76 3000 6000 0112 3456 7890 189"), ["iban"]);
        assert_eq!(kinds("J'habite au 12 rue des Lilas, Lyon"), ["address"]);
        assert_eq!(kinds("key = \"sk-abcdefghijklmnopqrstuvwx\""), ["key"]);
        assert_eq!(kinds("password: 'Hunter2Secret'"), ["key"]);
        assert_eq!(kinds("AWS AKIAIOSFODNN7EXAMPLE"), ["key"]);
        assert_eq!(kinds("tok eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N"), ["key"]);
        assert_eq!(kinds("https://api.example.test/feed?user=1&token=abcdef123456"), ["url_token"]);
        assert_eq!(kinds("https://bob:hunter22@ftp.example.test/x"), ["url_token"]);
        assert_eq!(kinds("random Zx9Qm2Lr8Tv4Kp7Wn3Bc6Hd1Fg5Js0Ya"), ["key"]);
    }

    #[test]
    fn ordinary_text_and_code_are_left_alone() {
        for text in [
            "Tu es un coach de course. Prépare une séance chaque matin à 07:00.",
            "import os\npassword = os.environ[\"GARMIN_PASSWORD\"]\nURL = \"https://connect.garmin.com/modern/proxy\"",
            "token = os.environ.get('TOKEN')",
            "écris à alex@example.org (exemple)",
            "version 2.10.3, le 2026-10-28",
            "sha 3f2a9c1d0b7e4a5f6c7d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b",
            "path /workspace/connecteurs/garmin/server_tools/helpers.py",
            "FR76 3000 6000 0112 3456 7890 180", // checksum wrong: not an IBAN
        ] {
            assert!(kinds(text).is_empty(), "{text}: {:?}", scan(text));
        }
    }

    #[test]
    fn excerpts_are_masked_and_lines_counted() {
        let flags = scan("ligne un\nmail: camille.durand@laposte.net");
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].line, 2);
        assert_eq!(flags[0].excerpt, "ca…@la…");
        assert!(!flags[0].excerpt.contains("durand"));
        let key = scan("sk-abcdefghijklmnopqrstuvwx")[0].excerpt.clone();
        assert!(!key.contains("ghijkl"), "{key}");
    }
}
