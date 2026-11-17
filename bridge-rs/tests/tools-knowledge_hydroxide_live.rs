//! The move path against hydroxide's own IMAP code (its `imap` package over a fake Proton API:
//! internal/notes/hydroxide-moves/, internal/notes/hydroxide-folders/harness/). Runs only with `HYDROXIDE_PORT` (and
//! `HYDROXIDE_MODE`: modern | legacy) set to a running `fakehydro`; skipped otherwise. Both hydroxide builds: as
//! released (folders `\Noinferiors`, none with today's API, no CREATE) and with the folders patch
//! (internal/notes/hydroxide-folders/: today's folders as `Parent/Child`, `\HasChildren`/`\HasNoChildren`, CREATE).

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use bipbridge::mail_tidy::{self, Action, Folders};
use bipbridge::mailbox::{self, Connect, Connection, MailError};
use bipbridge::tools_knowledge::imap::Client;
use bipbridge::Dict;
use serde_json::{json, Value};

fn connect(port: u16) -> Connection {
    let mut client = Client::connect_local("127.0.0.1", port, std::time::Duration::from_secs(10)).unwrap();
    client.login("alex@proton.me", "x").unwrap();
    Box::new(client)
}

#[test]
fn moves_through_hydroxide() {
    let Some(port) = std::env::var("HYDROXIDE_PORT").ok().and_then(|p| p.parse().ok()) else { return };
    if std::env::var("HYDROXIDE_SCENARIO").as_deref() == Ok("support") {
        return; // the folders below are the default scenario's
    }
    let modern = std::env::var("HYDROXIDE_MODE").as_deref() != Ok("legacy");
    let mut conn = connect(port);
    let folders = Folders::of(&mut conn).unwrap();
    println!("LIST {:?}", folders.0);
    // the patched build lists the person's folders without \Noinferiors: a server like any other (it makes folders)
    let patched = !folders.hydroxide();
    println!("patched hydroxide: {patched}");
    let wanted = folders.destination("Copro St Ex");
    println!("destination(Copro St Ex) = {wanted:?}");
    if patched {
        assert_eq!(wanted, Ok(("Copro St Ex".to_string(), false)));
        let nested = if modern { "Parent/Child" } else { "Child" };
        assert_eq!(folders.destination("Parent/Child"), Ok((nested.to_string(), false)));
        assert_eq!(folders.resolve("Child").as_deref(), Some(nested));
        assert_eq!(folders.resolve("impôts").as_deref(), Some("Impôts"));
        // a folder the person doesn't have yet: made (nested too on today's API), then filed into
        assert_eq!(folders.destination("Nouveau dossier"), Ok(("Nouveau dossier".to_string(), true)));
        let made = mail_tidy::ready_dest(&mut conn, "Nouveau dossier", true).unwrap();
        println!("ready_dest(Nouveau dossier, create) = {made:?}");
        assert!(made.is_ok());
        if modern {
            let made = mail_tidy::ready_dest(&mut conn, "Parent/Été", true).unwrap();
            println!("ready_dest(Parent/Été, create) = {made:?}");
            assert!(made.is_ok());
            assert!(Folders::of(&mut conn).unwrap().exists("Parent/Été"));
        }
    } else if modern {
        assert!(wanted.unwrap_err().contains("none of the person's own Proton folders"));
    } else {
        assert_eq!(wanted, Ok(("Copro St Ex".to_string(), false)));
        assert_eq!(folders.destination("Parent/Child"), Ok(("Child".to_string(), false)));
        assert_eq!(folders.resolve("impôts").as_deref(), Some("Impôts"));
    }
    let made = mail_tidy::ready_dest(&mut conn, "Copro St Ex", false).unwrap();
    println!("ready_dest(Copro St Ex) = {made:?}");
    assert_eq!(made.is_ok(), patched || !modern);
    assert_eq!(conn.select(&mailbox::quote_box("INBOX"), false).unwrap().0, "OK");
    let (status, data) = conn.uid("SEARCH", &["ALL"]).unwrap();
    println!("SEARCH {status} {data:?}");
    let refused = mail_tidy::act(&mut conn, &Action::Move("Nowhere".into()), "1", "Nowhere").unwrap();
    println!("act(Nowhere) = {refused:?}");
    assert_eq!(refused, Err("NO No such mailbox".to_string()));
    if patched || !modern {
        let accent = mail_tidy::act(&mut conn, &Action::Move("Impôts".into()), "1", "Impôts").unwrap();
        println!("act(Impôts) = {accent:?}");
        assert_eq!(accent, Ok(()));
    }
    if patched {
        let copro = mail_tidy::act(&mut conn, &Action::Move("Copro St Ex".into()), "3", "Copro St Ex").unwrap();
        println!("act(Copro St Ex) = {copro:?}");
        assert_eq!(copro, Ok(()));
        let made = mail_tidy::act(&mut conn, &Action::Move("Nouveau dossier".into()), "4", "Nouveau dossier").unwrap();
        println!("act(Nouveau dossier) = {made:?}");
        assert_eq!(made, Ok(()));
        if modern {
            let nested = mail_tidy::act(&mut conn, &Action::Move("Parent/Child".into()), "5", "Parent/Child").unwrap();
            println!("act(Parent/Child) = {nested:?}");
            assert_eq!(nested, Ok(()));
        }
    }
    let archived = mail_tidy::act(&mut conn, &Action::Archive, "2", "Archive").unwrap();
    println!("act(Archive) = {archived:?}");
    assert_eq!(archived, Ok(()));
    let _ = conn.logout();
}

/// The « Support BipAgents » routine through hydroxide's own code (fakehydro `SCENARIO=support`, patched build,
/// `HYDROXIDE_MODE=modern`): each tool call its own login, as on Alex's box when nothing else is logged in.
#[test]
fn the_support_routine_through_hydroxide() {
    let Some(port) = std::env::var("HYDROXIDE_PORT").ok().and_then(|p| p.parse::<u16>().ok()) else { return };
    if std::env::var("HYDROXIDE_SCENARIO").as_deref() != Ok("support") {
        return;
    }
    let api = std::env::var("HYDROXIDE_API").unwrap_or_default();
    let dir = tempfile::tempdir().unwrap();
    let config = common::config(dir.path(), &common::Urls::default());
    let account: Dict = serde_json::from_value(json!({"id": "hx", "provider": "proton", "host": "127.0.0.1",
        "port": port, "starttls": true, "username": "support@bipagents.com", "password": "x", "label": "Proton",
        "agents": ["coach"]}))
    .unwrap();
    mailbox::of(&config).write(&[account]).unwrap();
    let open: Connect = Arc::new(move |_: &Dict| -> Result<Connection, MailError> { Ok(connect(port)) });
    let call = |name: &str, a: Value| {
        let result = Value::Object(mailbox::call(&config, "coach", name, &serde_json::from_value(a).unwrap(), &open).unwrap());
        (result["isError"] == true, result["content"][0]["text"].as_str().unwrap().to_string())
    };
    let ids = |listed: &str| -> HashMap<String, String> {
        listed
            .lines()
            .filter(|l| l.starts_with("- id "))
            .map(|l| (l.rsplit(" · ").next().unwrap().to_string(), l[5..].split(" · ").next().unwrap().to_string()))
            .collect()
    };
    let bulk = |a: Value| {
        let called = mail_tidy::call(&config, "coach", "mail_bulk", &serde_json::from_value(a).unwrap(), &open).unwrap();
        let card = called.card.expect("a card");
        let job = mail_tidy::answer(&config, "coach", &card.id, true, "test").unwrap().unwrap();
        mail_tidy::run_job(&config, &job.id, &open).unwrap();
        mail_tidy::jobs_of(&config, "coach").into_iter().find(|j| j.id == job.id).unwrap()
    };
    // Q1: Support's own messages only
    let (error, found) = call("mail_search", json!({"folder": "Support", "limit": 20}));
    println!("search Support: {found}");
    let support = ids(&found);
    let mut subjects: Vec<&String> = support.keys().collect();
    subjects.sort();
    assert!(!error);
    assert_eq!(subjects, ["Avis : P2R-8ZT", "Message d'aide K7Q-4XN", "Question abonnement"]);
    // the folder as a person says it
    let (error, found) = call("mail_search", json!({"folder": "traite"}));
    println!("search traite: {error} {found}");
    assert!(!error && ids(&found).contains_key("Message d'aide AAA-111"), "{found}");
    let (error, said) = call("mail_search", json!({"folder": "Nowhere"}));
    println!("search Nowhere: {error} {said}");
    assert!(error && said.contains("« Support/En attente »"), "{said}");
    // the Reply-To of a help message
    let (_, read) = call("mail_read", json!({"id": support["Message d'aide K7Q-4XN"]}));
    println!("read K7Q-4XN: {read}");
    assert!(read.contains("Reply-To: Léa <lea@example.org>") && read.contains("comment connecter Gmail"), "{read}");
    // filed into Support/Traité, then found there by « Traité » and its code
    let job = bulk(json!({"action": "move:Support/Traité", "folder": "Support", "from": "no-reply@bipagents.com",
        "subject_contains": "K7Q-4XN"}));
    println!("job: {} acted {:?} skipped {} {}", job.status, job.acted, job.skipped, job.error);
    let (error, found) = call("mail_search", json!({"folder": "Traité", "query": "K7Q-4XN"}));
    println!("search Traité K7Q-4XN: {found}");
    assert!(!error && ids(&found).contains_key("Message d'aide K7Q-4XN"), "{found}");
    // ids taken before the move still read their own message (hydroxide numbered Support again from 1)
    for (subject, text) in [("Avis : P2R-8ZT", "Super appli."), ("Question abonnement", "mon abonnement")] {
        let (error, read) = call("mail_read", json!({"id": support[subject]}));
        println!("read {subject} by its old id {}: {read}", support[subject]);
        assert!(!error && read.contains(subject) && read.contains(text), "{subject}: {read}");
    }
    // a card, then a new help message filed into Support by Proton (its id sorts first) before the job runs
    let called = mail_tidy::call(&config, "coach", "mail_bulk", &serde_json::from_value(json!({
        "action": "move:Support/En attente", "folder": "Support", "from": "tom@example.net"})).unwrap(), &open)
    .unwrap();
    let card = called.card.expect("a card");
    let _ = std::process::Command::new("curl")
        .args(["-s", &format!("{api}/admin/new?id=a0&label=fSup&subject=Message%20d%27aide%20NEW-000")])
        .output();
    let job = mail_tidy::answer(&config, "coach", &card.id, true, "test").unwrap().unwrap();
    mail_tidy::run_job(&config, &job.id, &open).unwrap();
    let (_, waiting) = call("mail_search", json!({"folder": "Support/En attente"}));
    let (_, left) = call("mail_search", json!({"folder": "Support"}));
    println!("En attente after the job: {waiting}\nSupport after the job: {left}");
    assert!(ids(&waiting).contains_key("Question abonnement"), "{waiting}");
    assert!(ids(&left).contains_key("Message d'aide NEW-000") && ids(&left).contains_key("Avis : P2R-8ZT"), "{left}");
}
