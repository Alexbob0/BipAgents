//! « Dépenses partagées » (shared_expenses.rs), from the outside: the project agent's `expense` tool (add, everyone by
//! default from the share's people, fix, delete, one currency), the card the apps get ready to draw, the friends' copy
//! as a `card` item, the owner's delete (and the agent hearing of it), and the demo's account matching the bridge's sums.

mod common;

use std::path::Path;

use bipbridge::config::{parse_config, Config};
use bipbridge::{friends_cards, shared_expenses};
use common::{auth, config_dict, TestApp, Urls};
use serde_json::{json, Value};

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64()
}

fn setup(dir: &Path, archived: bool, shared: bool) -> Config {
    let dict = config_dict(dir, &Urls::default());
    let config = parse_config(&dict, None).unwrap();
    let data = Path::new(&config.data_dir()).to_path_buf();
    std::fs::create_dir_all(&data).unwrap();
    let archived = if archived { json!(now()) } else { Value::Null };
    std::fs::write(
        data.join("project-agents.json"),
        json!({"projects": {"wellness": {"created": now(), "ends": now() + 86400.0, "archived": archived, "lang": "fr"}}})
            .to_string(),
    )
    .unwrap();
    if shared {
        // the project's thread shared with two friends (the owner signs « Alex »)
        std::fs::write(
            data.join("read-shares.json"),
            json!({"shares": {"sh1": {"id": "sh1", "agent": "wellness", "thread": true, "from": "Alex", "ends": now() + 86400.0,
                "viewers": [{"id": "7", "name": "Léa Martin", "state": "ok"}, {"id": "8", "name": "Sam", "state": "ok"},
                            {"id": "9", "name": "Bob", "state": "refused"}]}}})
                .to_string(),
        )
        .unwrap();
    }
    config
}

fn call(config: &Config, args: Value) -> (bool, String) {
    let agent = config.agent(Some("wellness")).unwrap().clone();
    let result = shared_expenses::call(config, &agent, args.as_object().unwrap());
    (result["isError"] == json!(true), result["content"][0]["text"].as_str().unwrap_or("").to_string())
}

fn card_id(text: &str) -> String {
    let line = text.lines().find(|l| l.starts_with("MEDIA:")).unwrap_or_else(|| panic!("{text}"));
    friends_cards::card_in_path("wellness", line.trim_start_matches("MEDIA:")).unwrap().to_string()
}

#[test]
fn the_agent_notes_expenses_and_the_bridge_does_the_sums() {
    let dir = tempfile::tempdir().unwrap();
    let config = setup(dir.path(), false, true);

    // for everyone by default: the owner and the friends who joined (not the one refused)
    let (error, text) = call(&config, json!({"paid_by": "léa", "amount": 90, "what": "Dîner à Alfama"}));
    assert!(!error, "{text}");
    assert!(text.starts_with("Noted e1."), "{text}");
    assert!(text.contains("e1 Dîner à Alfama · Léa paid 90,00\u{a0}€ · for everyone"), "{text}");
    assert!(text.contains("To settle: Alex → Léa 30,00\u{a0}€; Sam → Léa 30,00\u{a0}€"), "{text}");
    let id = card_id(&text);

    // for some, an amount as text, the same card
    let (error, text) = call(&config, json!({"paid_by": "Sam", "amount": "20,01", "what": "Taxi", "for": ["sam", "Alex"]}));
    assert!(!error, "{text}");
    assert_eq!(card_id(&text), id);

    // the card the apps get: amounts written, latest first, balances adding up to 0, the fewest transfers
    let card = friends_cards::card(&config, "wellness", &id).unwrap();
    let view = friends_cards::view(&config, "wellness", &card);
    assert_eq!(view["type"], json!("expenses"));
    assert_eq!(view["title"], json!("Dépenses partagées"));
    assert_eq!(view["total"], json!("110,01\u{a0}€"));
    assert_eq!(view["expenses"][0]["id"], json!("e2"));
    assert_eq!(view["expenses"][0]["for"], json!(["Sam", "Alex"]));
    assert_eq!(view["expenses"][0]["all"], json!(false));
    assert_eq!(view["expenses"][1]["all"], json!(true));
    let cents: Vec<i64> = view["balances"].as_array().unwrap().iter().map(|b| b["cents"].as_i64().unwrap()).collect();
    assert_eq!(cents.iter().sum::<i64>(), 0);
    // Taxi 20.01 / 2: e2's odd cent at place 2 % 2 = 0 in name order (Alex, Sam): Alex
    assert_eq!(
        view["balances"],
        json!([{"name": "Alex", "cents": -4001, "amount": "−40,01\u{a0}€"},
               {"name": "Léa", "cents": 6000, "amount": "+60,00\u{a0}€"},
               {"name": "Sam", "cents": -1999, "amount": "−19,99\u{a0}€"}])
    );
    assert_eq!(
        view["transfers"],
        json!([{"from": "Alex", "to": "Léa", "amount": "40,01\u{a0}€"}, {"from": "Sam", "to": "Léa", "amount": "19,99\u{a0}€"}])
    );

    // fixed by its id, then deleted
    let (error, text) = call(&config, json!({"id": "e2", "amount": 30}));
    assert!(!error && text.starts_with("Fixed e2."), "{text}");
    let (error, text) = call(&config, json!({"id": "e2", "delete": true}));
    assert!(!error && text.starts_with("Deleted e2."), "{text}");
    assert!(text.contains("Total 90,00\u{a0}€ in 1 expenses"), "{text}");
    let (error, text) = call(&config, json!({"id": "e9", "delete": true}));
    assert!(error && text.contains("No expense e9"), "{text}");

    // one currency per project: another one is refused plainly
    let (error, text) = call(&config, json!({"paid_by": "Sam", "amount": 12, "what": "Café", "currency": "USD"}));
    assert!(error && text.contains("in EUR") && text.contains("convert"), "{text}");
    // what is missing, said plainly
    let (error, text) = call(&config, json!({"paid_by": "Sam", "what": "Café"}));
    assert!(error && text.contains("paid_by, amount and what"), "{text}");
    let (error, _) = call(&config, json!({"paid_by": "Sam", "amount": -4, "what": "Café"}));
    assert!(error);

    // the friends' copy: the same view as a `card` item, and no tap is taken on it
    let (item_id, item) = friends_cards::media_item(&config, "wellness", &format!("{}/wellness/{id}", friends_cards::PREFIX)).unwrap();
    assert_eq!(item_id, format!("c-{id}"));
    assert_eq!((item["kind"].clone(), item["type"].clone()), (json!("card"), json!("expenses")));
    assert_eq!(item["total"], json!("90,00\u{a0}€"));
    let lea = friends_cards::Voter { v: "7".into(), name: "Léa".into() };
    assert!(friends_cards::apply(&config, "wellness", &id, "e1", &lea).is_err());
}

#[test]
fn nobody_else_known_means_naming_who_shares_it() {
    let dir = tempfile::tempdir().unwrap();
    let config = setup(dir.path(), false, false);
    let (error, text) = call(&config, json!({"paid_by": "Léa", "amount": 40, "what": "Courses"}));
    assert!(error && text.contains("give the names"), "{text}");
    let (error, text) = call(&config, json!({"paid_by": "Léa", "amount": 40, "what": "Courses", "for": ["Léa", "Chi"]}));
    assert!(!error, "{text}");
    // Chi is known now: « everyone » next time
    let (error, text) = call(&config, json!({"paid_by": "Chi", "amount": 10, "what": "Pain"}));
    assert!(!error && text.contains("for everyone"), "{text}");
    // a non-project agent has no account
    let other = bipbridge::config::AgentConfig { name: "coach".into(), ..Default::default() };
    let r = shared_expenses::call(&config, &other, json!({"paid_by": "A", "amount": 1, "what": "x"}).as_object().unwrap());
    assert_eq!(r["isError"], json!(true));
}

#[tokio::test]
async fn the_owner_deletes_one_and_the_agent_hears_of_it() {
    let dir = tempfile::tempdir().unwrap();
    let config = setup(dir.path(), false, false);
    let (_, text) = call(&config, json!({"paid_by": "Léa", "amount": 40, "what": "Courses", "for": ["Léa", "Chi"]}));
    let id = card_id(&text);
    call(&config, json!({"paid_by": "Chi", "amount": 10, "what": "Pain"}));
    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let path = format!("/v1/project-agents/wellness/cards/{id}");
    let card = app.get(&path, &[(k, &v)]).await.json();
    assert_eq!(card["expenses"].as_array().unwrap().len(), 2);
    let gone = app.send_json("DELETE", &format!("{path}/expenses/e1"), &[(k, &v)], &json!({})).await;
    assert_eq!(gone.status.as_u16(), 200);
    let card = gone.json();
    assert_eq!(card["expenses"].as_array().unwrap().len(), 1);
    assert_eq!(card["transfers"], json!([{"from": "Léa", "to": "Chi", "amount": "5,00\u{a0}€"}]));
    assert_eq!(app.send_json("DELETE", &format!("{path}/expenses/e1"), &[(k, &v)], &json!({})).await.status.as_u16(), 404);
    assert_eq!(app.send_json("DELETE", &format!("{path}/expenses/e2"), &[], &json!({})).await.status.as_u16(), 401);
    // the agent's next turn: the owner's change, once
    let agent = config.agent(Some("wellness")).unwrap().clone();
    let head = bipbridge::dates::now::head(&config, &agent);
    assert!(head.contains("- Ta personne a supprimé la dépense e1 « Courses » (40,00\u{a0}€)"), "{head}");
    assert!(!bipbridge::dates::now::head(&config, &agent).contains("bip:friends"));
    // an ended project: nothing changes any more
    let config = setup(dir.path(), true, false);
    let app = TestApp::new(config.clone());
    assert_eq!(app.send_json("DELETE", &format!("{path}/expenses/e2"), &[(k, &v)], &json!({})).await.status.as_u16(), 409);
    let (error, _) = call(&config, json!({"paid_by": "Chi", "amount": 10, "what": "Pain"}));
    assert!(error);
}

/// The demo's account (demo.json `cards.lisbon_expenses`) is drawn from `views`: the bridge's own sums of its
/// `expenses`, in each language. Run with DEMO_PRINT=1 to print them after changing the expenses.
#[test]
fn the_demo_account_is_the_bridge_s() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../App/Resources/Demo/demo.json");
    let demo: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    let card = &demo["cards"]["lisbon_expenses"];
    let people = card["people"].clone();
    let list: Vec<Value> = card["expenses"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(n, e)| {
            let cents = shared_expenses::cents_of(&e["amount"]).unwrap();
            let shared = if e["for"].is_null() { people.clone() } else { e["for"].clone() };
            json!({"id": format!("e{}", n + 1), "what": e["what"], "by": e["by"], "cents": cents, "for": shared})
        })
        .collect();
    let mut views = serde_json::Map::new();
    for lang in ["fr", "en", "de", "es"] {
        let expenses: Vec<Value> = list
            .iter()
            .map(|e| {
                let mut e = e.clone();
                e["what"] = e["what"][lang].clone();
                e
            })
            .collect();
        let kept = json!({"type": "expenses", "currency": "EUR", "people": people, "expenses": expenses});
        let mut view = shared_expenses::view_fields(&kept, lang);
        // as the owner's app gets it (friends_cards.rs `view`), the demo's card ids
        view.insert("id".into(), json!("demo"));
        view.insert("card".into(), json!("demo"));
        view.insert("type".into(), json!("expenses"));
        view.insert("title".into(), card["title"][lang].clone());
        view.insert("open".into(), json!(true));
        view.insert("options".into(), json!([]));
        views.insert(lang.into(), Value::Object(view));
    }
    let views = Value::Object(views);
    if std::env::var("DEMO_PRINT").is_ok() {
        println!("DEMO_VIEWS {}", views);
    }
    assert_eq!(card["views"], views, "run with DEMO_PRINT=1 and paste `views` into demo.json");
}
