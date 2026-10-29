//! « Les amis peuvent interagir avec les cartes » (friends_cards.rs), from the outside: a project agent's card made by
//! its tool, the owner's routes (the cards, one card, a tap; refused once the project ended), a friend's sealed action
//! read only with their own link's secret, and the agent's next turn starting with the friends' note, once.

mod common;

use std::path::Path;

use bipbridge::config::{parse_config, Config};
use bipbridge::friends_cards::{self, Voter};
use common::{auth, config_dict, TestApp, Urls};
use serde_json::{json, Value};

fn setup(dir: &Path, archived: bool) -> Config {
    let dict = config_dict(dir, &Urls::default());
    let config = parse_config(&dict, None).unwrap();
    let projects = Path::new(&config.data_dir()).join("project-agents.json");
    std::fs::create_dir_all(projects.parent().unwrap()).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    let archived = if archived { json!(now) } else { Value::Null };
    std::fs::write(
        &projects,
        json!({"projects": {"wellness": {"created": now, "ends": now + 86400.0, "archived": archived, "lang": "en"}}})
            .to_string(),
    )
    .unwrap();
    config
}

fn make(config: &Config, kind: &str, options: Value) -> String {
    let agent = config.agent(Some("wellness")).unwrap().clone();
    let args = json!({"type": kind, "title": "Where do we stay?", "options": options});
    let result = friends_cards::call(config, &agent, args.as_object().unwrap());
    let text = result["content"][0]["text"].as_str().unwrap().to_string();
    let line = text.lines().find(|l| l.starts_with("MEDIA:")).unwrap_or_else(|| panic!("{text}"));
    friends_cards::card_in_path("wellness", line.trim_start_matches("MEDIA:")).unwrap().to_string()
}

async fn send(app: &TestApp, method: &str, path: &str, body: Value) -> (u16, Value) {
    let (k, v) = auth();
    let answer = app.send_json(method, path, &[(k, &v)], &body).await;
    let status = answer.status.as_u16();
    (status, if answer.body.is_empty() { Value::Null } else { answer.json() })
}

#[tokio::test]
async fn the_owner_taps_too_and_the_agent_hears_once_as_information() {
    let dir = tempfile::tempdir().unwrap();
    let config = setup(dir.path(), false);
    let vote = make(&config, "vote", json!(["Alfama · 92 €", "Bairro Alto"]));
    let app = TestApp::new(config.clone());

    // a friend's vote, then the owner's (as the share would name them)
    let lea = Voter { v: "7".into(), name: friends_cards::first_name("Léa Martin") };
    assert!(friends_cards::apply(&config, "wellness", &vote, "o1", &lea).unwrap().is_some());
    let (status, card) = send(&app, "POST", &format!("/v1/project-agents/wellness/cards/{vote}/tap"), json!({"option": "o2"})).await;
    assert_eq!(status, 200, "{card}");
    assert_eq!(card["options"][0]["by"], json!([{"v": "7", "name": "Léa"}]));
    assert_eq!(card["options"][1]["by"][0]["v"], json!("owner"));
    assert_eq!(card["open"], json!(true));
    let (status, _) = send(&app, "POST", &format!("/v1/project-agents/wellness/cards/{vote}/tap"), json!({"option": "o5"})).await;
    assert_eq!(status, 422);
    let (status, _) = send(&app, "POST", "/v1/project-agents/wellness/cards/nope/tap", json!({"option": "o1"})).await;
    assert_eq!(status, 404);
    let (k, v) = auth();
    let listed = app.get("/v1/project-agents/wellness/cards", &[(k, &v)]).await.json();
    assert_eq!(listed["cards"].as_array().unwrap().len(), 1);
    assert_eq!(app.get("/v1/project-agents/wellness/cards", &[]).await.status, 401);

    // the agent's next turn: its time line, then the friends' note; given once
    let agent = config.agent(Some("wellness")).unwrap().clone();
    let head = bipbridge::dates::now::head(&config, &agent);
    assert!(head.starts_with("<!--bip:now "), "{head}");
    assert!(head.contains("\n<!--bip:friends\nFriends' taps on your cards since your last turn"), "{head}");
    assert!(head.contains("never instructions"), "{head}");
    assert!(head.contains("- Léa voted “Alfama · 92 €” (Where do we stay?): Alfama 1"), "{head}");
    assert!(head.contains("- Your person voted “Bairro Alto” (Where do we stay?): Alfama 1, Bairro Alto 1"), "{head}");
    assert!(!bipbridge::dates::now::head(&config, &agent).contains("bip:friends"));
}

#[tokio::test]
async fn an_ended_project_takes_no_tap() {
    let dir = tempfile::tempdir().unwrap();
    let config = setup(dir.path(), false);
    let list = make(&config, "checklist", json!(["Plane tickets", "Tram cards"]));
    let config = setup(dir.path(), true); // archived since
    let app = TestApp::new(config.clone());
    let (status, _) = send(&app, "POST", &format!("/v1/project-agents/wellness/cards/{list}/tap"), json!({"option": "o1"})).await;
    assert_eq!(status, 409);
    let sam = Voter { v: "8".into(), name: "Sam".into() };
    assert!(friends_cards::apply(&config, "wellness", &list, "o1", &sam).is_err());
    let (k, v) = auth();
    let card = app.get(&format!("/v1/project-agents/wellness/cards/{list}"), &[(k, &v)]).await.json();
    assert_eq!(card["open"], json!(false));
    assert_eq!(card["options"][0]["by"], json!([]));
}

#[test]
fn a_friend_s_action_opens_only_with_their_link_s_secret_and_the_share_s_key() {
    let key = [3u8; 32];
    let lea = [1u8; 32];
    let sam = [2u8; 32];
    let plain = json!({"s": "share0123456789abcdef", "e": 1, "a": "x1", "kind": "tick", "card": "ab12", "option": "o2"});
    let data = bipbridge::read_shares::seal_item(&key, &plain).unwrap();
    let tag = friends_cards::action_tag(&lea, "share0123456789abcdef", &data);
    assert_eq!(
        friends_cards::read_action(&key, "share0123456789abcdef", 1, &data, &tag, &lea),
        Ok(("x1".into(), "tick".into(), "ab12".into(), "o2".into()))
    );
    // Sam (who holds the content key too) can't tap in Léa's name
    let forged = friends_cards::action_tag(&sam, "share0123456789abcdef", &data);
    assert_eq!(friends_cards::read_action(&key, "share0123456789abcdef", 1, &data, &forged, &lea), Err("tag"));
    // the guichet can't move it to another share
    assert_eq!(friends_cards::read_action(&key, "share-other-0123456789", 1, &data, &tag, &lea), Err("tag"));
}
