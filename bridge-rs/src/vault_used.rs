//! The vault list's « Récents » and sites (design-coffre-rassurer.md §7.2, D6 A, Alex 16/11).
//!
//! - **Last used** (`last_used_at`, `last_used_by` on a login of `GET /v1/vault`): when an agent last had the vault type
//!   that login, and which agent. Set by the bridge's own fills (`vault_login`, `vault_create_login`, vault_logins.rs);
//!   a fill by Hermes' `browser_vault` does not go through the bridge and leaves none. Kept beside the vault in
//!   `vault_used.json` (`{item id: {"at": ISO 8601 UTC, "by": agent id}}`, 0600), never a secret, so the encrypted
//!   vault is not written again at each login. Absent until a first use.
//! - **Domain** (`domain` on every login, vault.rs `Item::public`): the site as the vault matches it (vault_match.rs):
//!   its registrable domain (`www.amazon.fr` and `smile.amazon.fr`: `amazon.fr`), else its exact host without
//!   « www. » (an IP, a one-label name, a suffix the list doesn't know). The apps group a site's logins by it.
//!
//! No Python counterpart.

use std::path::Path;

use serde_json::Value;

/// Beside `vault.json.enc`.
pub const FILE: &str = "vault_used.json";
/// Items kept at most (the most recent).
pub const KEEP: usize = 500;

/// Agent `agent` had the vault type login `item` now.
pub fn note(_folder: &Path, _item: &str, _agent: &str) {}

/// Every item's last use: id → (at, by).
pub fn all(_folder: &Path) -> std::collections::BTreeMap<String, (String, String)> {
    Default::default()
}

/// The logins of a `GET /v1/vault` answer with their last use, when there is one.
pub fn decorate(_folder: &Path, _items: &mut [Value]) {}

/// A login's site as the vault matches it (see above); None: not a web address.
pub fn domain_of(_origin: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_logins_domain_is_the_site_the_vault_fills_on() {
        let cases = [
            ("https://www.amazon.fr", Some("amazon.fr")),
            ("https://smile.amazon.fr", Some("amazon.fr")),
            ("https://extranet2.example.fr", Some("example.fr")),
            ("https://alice-shop.myshopify.com", Some("alice-shop.myshopify.com")),
            ("https://www.fournisseur.example", Some("fournisseur.example")),
            ("https://extranet.reseau-agences.example", Some("extranet.reseau-agences.example")),
            ("http://192.168.1.10:8080", Some("192.168.1.10")),
            ("https://localhost", Some("localhost")),
            ("https://www.EXAMPLE.co.uk", Some("example.co.uk")),
            ("", None),
            ("not a site", None),
        ];
        for (origin, domain) in cases {
            assert_eq!(domain_of(origin).as_deref(), domain, "{origin}");
        }
    }

    #[test]
    fn the_app_sees_each_logins_domain() {
        let login = crate::vault::Item {
            id: "item_0123456789ab".into(),
            kind: "login".into(),
            origin: Some("https://connexion.rail-express.fr".into()),
            ..Default::default()
        };
        assert_eq!(login.public()["domain"], json!("rail-express.fr"));
        let card = crate::vault::Item { kind: "card".into(), ..Default::default() };
        assert!(!card.public().as_object().unwrap().contains_key("domain"));
    }

    #[test]
    fn the_last_use_is_kept_beside_the_vault_without_any_secret() {
        let dir = tempfile::tempdir().unwrap();
        let mut listed = vec![
            json!({"id": "item_a", "kind": "login", "label": "Fournisseur"}),
            json!({"id": "item_b", "kind": "login", "label": "Mutuelle"}),
            json!({"id": "item_c", "kind": "address", "label": "Camille (pro)"}),
        ];
        decorate(dir.path(), &mut listed);
        assert!(listed.iter().all(|i| i.get("last_used_at").is_none()), "never used: nothing said");
        note(dir.path(), "item_a", "vie");
        note(dir.path(), "item_c", "vie");
        decorate(dir.path(), &mut listed);
        assert_eq!(listed[0]["last_used_by"], json!("vie"));
        let at = listed[0]["last_used_at"].as_str().unwrap();
        assert!(at.ends_with("+00:00") && at.len() == 25, "{at}");
        assert!(listed[1].get("last_used_at").is_none());
        assert!(listed[2].get("last_used_at").is_none(), "logins only");
        // used again by another agent: the latest
        note(dir.path(), "item_a", "paul");
        assert_eq!(all(dir.path())["item_a"].1, "paul");
        let path = dir.path().join(FILE);
        let text = std::fs::read_to_string(&path).unwrap();
        let kept: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(kept["item_a"]["by"], json!("paul"));
        assert_eq!(kept["item_a"].as_object().unwrap().len(), 2, "when and who, nothing else");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        // a file that can't be read: nothing said, nothing lost
        std::fs::write(&path, "{not json").unwrap();
        assert!(all(dir.path()).is_empty());
        note(dir.path(), "item_b", "vie");
        assert_eq!(all(dir.path()).len(), 1);
    }

    #[test]
    fn only_the_most_recent_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let mut kept = serde_json::Map::new();
        for n in 0..KEEP + 20 {
            kept.insert(format!("item_{n:012}"), json!({"at": format!("2026-11-01T00:{:02}:{:02}+00:00", n / 60 % 60, n % 60), "by": "vie"}));
        }
        std::fs::write(dir.path().join(FILE), Value::Object(kept).to_string()).unwrap();
        note(dir.path(), "item_new", "vie");
        let now = all(dir.path());
        assert_eq!(now.len(), KEEP);
        assert!(now.contains_key("item_new") && !now.contains_key("item_000000000000"));
    }

    /// contracts/vault-list/cases.json, read by the apps' tests: the `domain` its logins carry is the bridge's.
    #[test]
    fn the_shared_cases_carry_the_bridges_domains() {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/vault-list/cases.json");
        let cases: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let mut seen = 0;
        for vault in cases["vaults"].as_object().unwrap().values() {
            for item in vault["items"].as_array().unwrap() {
                if item["kind"] != "login" || item.get("domain").is_none() {
                    continue;
                }
                let origin = item["origin"].as_str().unwrap();
                assert_eq!(item["domain"].as_str(), domain_of(origin).as_deref(), "{origin}");
                seen += 1;
            }
        }
        assert!(seen >= 10, "{seen} logins with their domain");
    }
}
