//! Agents of two households (two BipAgents installs, e.g. Alex's and Léa's) that may write to each other.
//!
//! Linking two installs: one person shows an invitation (a QR code: this bridge's address and a one-time code, valid ten
//! minutes); the other scans it, and their bridge introduces itself with the code. Each bridge then gets a key of its
//! own for the other, which only lets it hand messages over, and only along links each person turned on:
//!
//! - on Alex's side, « Vie peut écrire à Cuisine (Léa) » (the link vie → cuisine__lea);
//! - on Léa's side, « Vie (Alex) peut écrire à Cuisine » (the link vie__alex → cuisine).
//!
//! An agent writes to `bip/cuisine__lea` (its bridge is its only peer, see peers.py); its bridge checks its own link and
//! hands the message to Léa's bridge (`/family/p/cuisine/…`), which checks hers and delivers it, signed
//! « Message from Vie (Alex) (@vie__alex) » so Cuisine can answer the same way. No agent ever gets another's key.
//! `<data>/households.json` holds this install's name, the linked households (their address, the key we send, a digest
//! of the key they send, and the certificate their address serves when it is a place's door: door_tls.rs) and the
//! pending invitations.
//!
//! Port of `bridge/bipbridge/households.py` (owner group: agents).

use std::path::PathBuf;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::config::Config;
use crate::Dict;

/// « cuisine__lea »: agent cuisine of the household lea.
pub const SEPARATOR: &str = "__";
pub const INVITE_SECONDS: f64 = 600.0;

/// Python's `HouseholdError` (a `ValueError`).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct HouseholdError(pub String);

fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

pub fn slug(name: &str) -> String {
    let plain: String = name.nfkd().filter(char::is_ascii).collect::<String>().to_lowercase();
    let mut out = String::new();
    let mut gap = false;
    for c in plain.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if gap {
                out.push('-');
                gap = false;
            }
            out.push(c);
        } else {
            gap = true;
        }
    }
    if gap {
        out.push('-');
    }
    let trimmed: String = out.trim_matches('-').chars().take(24).collect();
    if trimmed.is_empty() {
        "famille".to_string()
    } else {
        trimmed
    }
}

/// (agent, household) for « cuisine__lea », None for a local agent.
pub fn split(target: &str) -> Option<(String, String)> {
    let at = target.rfind(SEPARATOR)?;
    let (agent, household) = (&target[..at], &target[at + SEPARATOR.len()..]);
    if agent.is_empty() || household.is_empty() {
        return None;
    }
    Some((agent.to_string(), household.to_string()))
}

pub struct Households {
    pub path: PathBuf,
}

/// `data.get(key, default)` of the file's dict (Python keeps whatever type the file holds).
fn field(data: &Value, key: &str) -> Option<Value> {
    data.as_object().and_then(|d| d.get(key)).cloned()
}

impl Households {
    pub fn new(path: &str) -> Self {
        Households { path: PathBuf::from(path) }
    }

    /// `{"name", "households", "invites"}` (expired invitations left out).
    pub fn load(&self) -> Dict {
        let data: Value = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| json!({}));
        let data = if data.is_object() { data } else { json!({}) };
        let now = crate::py::time_time();
        let mut invites = Dict::new();
        if let Some(Value::Object(found)) = field(&data, "invites") {
            for (k, v) in found {
                if v.as_f64().map(|t| t > now).unwrap_or(false) {
                    invites.insert(k, v);
                }
            }
        }
        let mut out = Dict::new();
        out.insert("name".into(), field(&data, "name").unwrap_or_else(|| json!("")));
        out.insert("households".into(), field(&data, "households").unwrap_or_else(|| json!({})));
        out.insert("invites".into(), Value::Object(invites));
        out
    }

    /// Written whole (0600, through a temporary file).
    pub fn save(&self, data: &Dict) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = PathBuf::from(format!("{}.tmp", self.path.display()));
        crate::py::write_with_mode(
            &tmp.to_string_lossy(),
            crate::py::dumps(&Value::Object(data.clone())).as_bytes(),
            0o600,
        )?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn name(&self) -> String {
        self.load().get("name").map(crate::py::py_str).unwrap_or_default()
    }

    pub fn set_name(&self, name: &str) -> anyhow::Result<()> {
        let name: String = name.trim().chars().take(40).collect();
        if name.is_empty() {
            return Err(HouseholdError("a name is needed".into()).into());
        }
        let mut data = self.load();
        data.insert("name".into(), json!(name));
        self.save(&data)
    }

    /// A one-time code for the other household's bridge (ten minutes).
    pub fn invite(&self) -> anyhow::Result<String> {
        self.invite_for(INVITE_SECONDS)
    }

    /// A one-time code valid `seconds`.
    pub fn invite_for(&self, seconds: f64) -> anyhow::Result<String> {
        if self.name().is_empty() {
            return Err(HouseholdError("set this household's name first".into()).into());
        }
        let code = crate::py::token_urlsafe(24);
        let mut data = self.load();
        if let Some(Value::Object(invites)) = data.get_mut("invites") {
            invites.insert(digest(&code), json!(crate::py::time_time() + seconds));
        }
        self.save(&data)?;
        Ok(code)
    }

    /// The other bridge introduces itself with our code and the key it will call us with (we keep its digest).
    /// Returns the key we will call it with, and our name.
    /// `fingerprint`: the certificate their address serves when it is a door (door_tls.rs), None for a public one.
    pub fn accept(
        &self,
        code: &str,
        name: &str,
        url: &str,
        their_key: &str,
        fingerprint: Option<&str>,
    ) -> Result<Dict, HouseholdError> {
        let mut data = self.load();
        let known = match data.get_mut("invites") {
            Some(Value::Object(invites)) => invites.shift_remove(&digest(code)).is_some(),
            _ => false,
        };
        if !known {
            return Err(HouseholdError("this invitation is unknown or expired".into()));
        }
        let ours = crate::py::token_urlsafe(32);
        Self::add(&mut data, name, url, &ours, their_key, fingerprint)?;
        self.save(&data).map_err(|e| HouseholdError(e.to_string()))?;
        let mut out = Dict::new();
        out.insert("key".into(), json!(ours));
        out.insert("name".into(), data.get("name").cloned().unwrap_or(Value::Null));
        Ok(out)
    }

    /// We scanned their invitation and they accepted: `key_out` is ours to call them, `key_in` theirs to call us.
    pub fn joined(
        &self,
        name: &str,
        url: &str,
        key_out: &str,
        key_in: &str,
        fingerprint: Option<&str>,
    ) -> anyhow::Result<String> {
        let mut data = self.load();
        let household = Self::add(&mut data, name, url, key_out, key_in, fingerprint)?;
        self.save(&data)?;
        Ok(household)
    }

    fn add(
        data: &mut Dict,
        name: &str,
        url: &str,
        key_out: &str,
        key_in: &str,
        fingerprint: Option<&str>,
    ) -> Result<String, HouseholdError> {
        let short = key_out.chars().count().min(key_in.chars().count()) < 32;
        if name.trim().is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) || short {
            return Err(HouseholdError("invalid household".into()));
        }
        let fingerprint = match fingerprint.map(str::trim).filter(|f| !f.is_empty()) {
            Some(given) => {
                Some(crate::door_tls::normalize(given).ok_or_else(|| HouseholdError("invalid fingerprint".into()))?)
            }
            None => None,
        };
        let household = slug(name);
        let own = data.get("name").map(crate::py::py_str).unwrap_or_default();
        if household == slug(&own) {
            return Err(HouseholdError("a household cannot link to itself (or to one with the same name)".into()));
        }
        let mut entry = json!({
            "name": name.trim().chars().take(40).collect::<String>(),
            "url": url.trim_end_matches('/'),
            "key_out": key_out,
            "key_in": digest(key_in),
        });
        if let Some(fingerprint) = fingerprint {
            entry["fingerprint"] = json!(fingerprint);
        }
        match data.get_mut("households") {
            Some(Value::Object(households)) => {
                households.insert(household.clone(), entry);
            }
            _ => {
                let mut households = Dict::new();
                households.insert(household.clone(), entry);
                data.insert("households".into(), Value::Object(households));
            }
        }
        Ok(household)
    }

    pub fn caller(&self, token: &str) -> Option<String> {
        let wanted = digest(token);
        let data = self.load();
        let households = data.get("households")?.as_object()?;
        households
            .iter()
            .find(|(_, v)| {
                let key_in = v
                    .get("key_in")
                    .map(crate::py::py_str)
                    .filter(|_| v.get("key_in").map(crate::py::truthy).unwrap_or(false));
                key_in.map(|k| crate::auth::constant_time_eq(k.as_bytes(), wanted.as_bytes())).unwrap_or(false)
            })
            .map(|(h, _)| h.clone())
    }

    /// The certificate a linked household's bridge says it serves (`POST /family/door`, with its key): kept with its
    /// address, only when `url` (the address it says it serves it at) is the one we call it at. Python's `KeyError`
    /// when there is no such household.
    pub fn set_fingerprint(&self, household: &str, fingerprint: &str, url: Option<&str>) -> anyhow::Result<bool> {
        let fingerprint = crate::door_tls::normalize(fingerprint)
            .ok_or_else(|| anyhow::Error::from(HouseholdError("invalid fingerprint".into())))?;
        let mut data = self.load();
        let Some(entry) = data
            .get_mut("households")
            .and_then(Value::as_object_mut)
            .and_then(|h| h.get_mut(household))
            .and_then(Value::as_object_mut)
        else {
            anyhow::bail!(crate::py::py_repr(&json!(household)));
        };
        let stored = entry.get("url").and_then(Value::as_str).unwrap_or("").trim_end_matches('/');
        if url.is_some_and(|u| u.trim_end_matches('/') != stored) {
            return Err(HouseholdError("not the address this household is reached at".into()).into());
        }
        if entry.get("fingerprint").and_then(Value::as_str) == Some(fingerprint.as_str()) {
            return Ok(false);
        }
        entry.insert("fingerprint".into(), json!(fingerprint));
        self.save(&data)?;
        Ok(true)
    }

    pub fn get(&self, household: &str) -> Option<Dict> {
        let data = self.load();
        data.get("households")?.as_object()?.get(household)?.as_object().cloned()
    }

    /// Python's `KeyError` when there is no such household.
    pub fn remove(&self, household: &str) -> anyhow::Result<()> {
        let mut data = self.load();
        let removed = match data.get_mut("households") {
            Some(Value::Object(households)) => households.shift_remove(household).is_some(),
            _ => false,
        };
        if !removed {
            anyhow::bail!(crate::py::py_repr(&json!(household)));
        }
        self.save(&data)
    }

    /// The linked households, by id: `{id, name, url}`.
    pub fn public(&self) -> Vec<Dict> {
        let data = self.load();
        let Some(households) = data.get("households").and_then(Value::as_object) else { return Vec::new() };
        let mut ids: Vec<&String> = households.keys().collect();
        ids.sort();
        ids.into_iter()
            .map(|id| {
                let v = &households[id];
                let mut out = Dict::new();
                out.insert("id".into(), json!(id));
                out.insert("name".into(), v.get("name").cloned().unwrap_or(Value::Null));
                out.insert("url".into(), v.get("url").cloned().unwrap_or(Value::Null));
                // a door's address with no certificate to check it: the link has to be made again
                let relink = v.as_object().map(crate::door_tls::needs_relink).unwrap_or(false);
                out.insert("relink".into(), json!(relink));
                out
            })
            .collect()
    }
}

/// `data_dir/households.json` (next to the database).
pub fn path_for(config: &Config) -> String {
    crate::py::join(&config.data_dir(), "households.json")
}

/// Where the other household's bridge reaches this one (its tailnet address).
pub fn own_url(config: &Config) -> Option<String> {
    config.public_url.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_targets() {
        assert_eq!(slug("Léa"), "lea");
        assert_eq!(slug("  Les Dupont-Martin! "), "les-dupont-martin");
        assert_eq!(slug("🙂"), "famille");
        assert_eq!(split("cuisine__lea"), Some(("cuisine".into(), "lea".into())));
        assert_eq!(split("a__b__c"), Some(("a__b".into(), "c".into())));
        assert_eq!(split("vie"), None);
        assert_eq!(split("__lea"), None);
    }

    /// test_households.py: invitations are one-time and expire.
    #[test]
    fn invitations_are_one_time_and_expire() {
        let dir = tempfile::tempdir().unwrap();
        let households = Households::new(&dir.path().join("households.json").to_string_lossy());
        assert!(households.invite().is_err()); // name the household first
        households.set_name("Alex").unwrap();
        let code = households.invite().unwrap();
        let answer = households.accept(&code, "Léa", "https://b.ts.net", &"k".repeat(40), None).unwrap();
        assert_eq!(households.caller(&"k".repeat(40)).as_deref(), Some("lea"));
        assert!(answer["key"].as_str().unwrap().len() >= 32 && answer["name"] == "Alex");
        assert!(!std::fs::read_to_string(&households.path).unwrap().contains(&"k".repeat(40))); // only a digest
        assert!(households.accept(&code, "Léa", "https://b.ts.net", &"k".repeat(40), None).is_err()); // used once
        let late = households.invite().unwrap();
        let mut data = households.load();
        let expired: Dict = data["invites"]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| (k.clone(), json!(crate::py::time_time() - 1.0)))
            .collect();
        data.insert("invites".into(), Value::Object(expired));
        households.save(&data).unwrap();
        assert!(households.accept(&late, "Paul", "https://c.ts.net", &"p".repeat(40), None).is_err());
    }
}
