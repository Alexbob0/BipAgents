//! The move path against hydroxide's own IMAP code (its `imap` package over a fake Proton API:
//! internal/notes/hydroxide-moves/). Runs only with `HYDROXIDE_PORT` (and `HYDROXIDE_MODE`: modern | legacy) set to a
//! running `fakehydro`; skipped otherwise.

use bipbridge::mail_tidy::{self, Action, Folders};
use bipbridge::mailbox::{self, Connection};
use bipbridge::tools_knowledge::imap::Client;

fn connect(port: u16) -> Connection {
    let mut client = Client::connect_local("127.0.0.1", port, std::time::Duration::from_secs(10)).unwrap();
    client.login("alex@proton.me", "x").unwrap();
    Box::new(client)
}

#[test]
fn moves_through_hydroxide() {
    let Some(port) = std::env::var("HYDROXIDE_PORT").ok().and_then(|p| p.parse().ok()) else { return };
    let modern = std::env::var("HYDROXIDE_MODE").as_deref() != Ok("legacy");
    let mut conn = connect(port);
    let folders = Folders::of(&mut conn).unwrap();
    println!("LIST {:?}", folders.0);
    assert!(folders.hydroxide());
    let wanted = folders.destination("Copro St Ex");
    println!("destination(Copro St Ex) = {wanted:?}");
    if modern {
        assert!(wanted.unwrap_err().contains("none of the person's own Proton folders"));
    } else {
        assert_eq!(wanted, Ok(("Copro St Ex".to_string(), false)));
        assert_eq!(folders.destination("Parent/Child"), Ok(("Child".to_string(), false)));
        assert_eq!(folders.resolve("impôts").as_deref(), Some("Impôts"));
    }
    let made = mail_tidy::ready_dest(&mut conn, "Copro St Ex", false).unwrap();
    println!("ready_dest(Copro St Ex) = {made:?}");
    assert_eq!(made.is_ok(), !modern);
    assert_eq!(conn.select(&mailbox::quote_box("INBOX"), false).unwrap().0, "OK");
    let (status, data) = conn.uid("SEARCH", &["ALL"]).unwrap();
    println!("SEARCH {status} {data:?}");
    let refused = mail_tidy::act(&mut conn, &Action::Move("Nowhere".into()), "1", "Nowhere").unwrap();
    println!("act(Nowhere) = {refused:?}");
    assert_eq!(refused, Err("NO No such mailbox".to_string()));
    if !modern {
        let accent = mail_tidy::act(&mut conn, &Action::Move("Impôts".into()), "1", "Impôts").unwrap();
        println!("act(Impôts) = {accent:?}");
        assert_eq!(accent, Ok(()));
    }
    let archived = mail_tidy::act(&mut conn, &Action::Archive, "2", "Archive").unwrap();
    println!("act(Archive) = {archived:?}");
    assert_eq!(archived, Ok(()));
    let _ = conn.logout();
}
