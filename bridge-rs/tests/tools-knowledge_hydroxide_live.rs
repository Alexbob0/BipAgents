//! The move path against hydroxide's own IMAP code (its `imap` package over a fake Proton API:
//! internal/notes/hydroxide-moves/, internal/notes/hydroxide-folders/harness/). Runs only with `HYDROXIDE_PORT` (and
//! `HYDROXIDE_MODE`: modern | legacy) set to a running `fakehydro`; skipped otherwise. Both hydroxide builds: as
//! released (folders `\Noinferiors`, none with today's API, no CREATE) and with the folders patch
//! (internal/notes/hydroxide-folders/: today's folders as `Parent/Child`, `\HasChildren`/`\HasNoChildren`, CREATE).

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
