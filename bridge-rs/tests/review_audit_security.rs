//! Review of audit-security-1117 (18/11): tests that describe what should happen (red on c13a3d927 where a finding).

use bipbridge::config::Config;
use bipbridge::{card_place, held_approvals};
use serde_json::{json, Map, Value};

fn config_in(dir: &tempfile::TempDir) -> Config {
    let mut config = Config::with_key("k");
    config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
    config
}

fn data(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap()
}

/// What person_ok.rs `data_of` gives held_approvals for a card (command, summary, detail).
fn card(command: &str, summary: &str, detail: &str) -> Map<String, Value> {
    data(json!({"command": command, "description": summary, "bip_summary": summary, "bip_detail": detail}))
}

/// A routine held a deletion of the person's calendar (critical) and, in the same night, a form to a new site and a
/// terminal command: three different things. Each should be its own card: one « Oui » must not delete the event AND
/// send the form AND run the command. person_ok's held cards are classified by `classify_hermes` (« command »), so
/// card_place.rs `fold` merges them into one card under the first one's sentence.
#[test]
fn a_held_deletion_is_never_folded_with_other_held_requests() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(&dir);
    let now = bipbridge::py::time_time();
    let delete = card("calendar_delete ev1", "Vie veut supprimer un événement de ton calendrier : « Dentiste »",
                      "2026-11-20 10:00\n« Dentiste »");
    let send = card("ask_to_send forum.example", "Envoyer sur ce site ? Vie veut envoyer un formulaire à forum.example",
                    "Ce que l’agent dit envoyer : « un message »");
    let shell = data(json!({"command": "rm -rf /tmp/x", "description": "recursive delete", "pattern_key": "recursive delete"}));
    held_approvals::hold(&config, "vie", "run_1", None, &delete, None, now + 3600.0);
    held_approvals::hold(&config, "vie", "run_1", None, &send, None, now + 3600.0);
    held_approvals::hold(&config, "vie", "run_1", None, &shell, None, now + 3600.0);
    let items = held_approvals::pending_items(&config, Some("vie"));
    assert_eq!(items.len(), 3);
    let kinds: Vec<Value> = items.iter().map(|i| i["bip_held"]["kind"].clone()).collect();
    let cards = card_place::fold(&config, items);
    assert_eq!(cards.len(), 3, "folded into one card (held kinds {kinds:?}): {:?}",
               cards.iter().map(|c| c["bip_summary"].clone()).collect::<Vec<_>>());
}

/// ask_to_send held (nobody follows the turn), the person says « Oui »; the agent, told to « call ask_to_send again
/// for the same site », says what it sends in other words: the grant is for that site (the card's command), yet its
/// hash covers the agent's own free words (`what` in the detail), so the retry finds nothing and asks again.
#[test]
fn a_held_yes_for_a_site_serves_the_retry_whatever_words_the_agent_uses() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_in(&dir);
    let now = bipbridge::py::time_time();
    let summary = "Envoyer sur ce site ? Vie veut envoyer un formulaire à forum.example";
    let first = card("ask_to_send forum.example", summary, "Ce que l’agent dit envoyer : « un message au forum »");
    let retry = card("ask_to_send forum.example", summary, "Ce que l’agent dit envoyer : « le message pour le forum »");
    let (held, _) = held_approvals::hold(&config, "vie", "run_1", None, &first, None, now + 3600.0);
    held_approvals::answer(&config, "vie", &held.id, true).unwrap();
    let grant = held_approvals::take_grant(&config, "vie", "ask_to_send forum.example", &held_approvals::args_sha(&retry));
    assert!(grant.is_some(), "the person's « Oui » for forum.example not found by the retry");
}
