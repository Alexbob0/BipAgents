//! Files a reply only cites, made openable (Alex 2026-10-28: an agent wrote « c'est dans /…/notes.md » and the
//! person couldn't open it). When a turn ends (runs.rs for the runs the hub follows, cronwatch.rs for a routine's
//! output: the same places where made_files.rs hands over the files the bridge made), the paths the reply cites are
//! looked for; each one that is a file of that agent's own working folders gets a `MEDIA:` line at the end of the
//! reply, so the apps show a file card. The path's text stays in the reply as the agent wrote it.
//!
//! - **Cited**: an absolute path (`/envoi/notes.md`, `/fichiers/2026-10/…`, `/workspace/out/x.png`, a received
//!   files' folder by its path on this machine), or `~/…` (the sandbox's home, `/root`), with a file extension, on
//!   its own or in backticks (which may hold spaces); never a link's path (`https://…/x.png`), never one on a
//!   `MEDIA:` line.
//! - **Its own folders only** (sent_files.rs `cited_file`): its outbox `/envoi`, its sandbox's `/workspace` and
//!   `/root`, its shared folder and the ones it may read, its received files; never its Hermes profile, a folder
//!   mounted by hand, another agent's folders, nor anything copied out of a container. Opened name by name without
//!   following a link (beneath.rs), a regular file, not secret-looking, a type the apps open, at most the upload limit.
//! - **No duplicate**: a file the reply already hands over with a `MEDIA:` line (by either spelling, `~/x` or
//!   `/root/x`) gets no second one.
//!
//! No Python counterpart (owner group: push-runs).

use std::sync::LazyLock;

use regex::Regex;

use crate::config::{AgentConfig, Config};

/// At most this many cards added to one reply.
const MAX_CARDS: usize = 10;

/// A path in backticks (spaces allowed inside).
static QUOTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`((?:~/|/)[^`\n]{1,400}?\.[A-Za-z0-9]{1,8})`").expect("valid regex"));

/// A bare path: after the start, a space or an opening mark, up to a space or a closing mark.
static BARE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)(?:^|[\s(\[«"'“‘*_>])((?:~/|/)[^\s`"'«»“”‘’<>()\[\]{}|*]{1,400})"#).expect("valid regex")
});

/// Whether `text` may cite a path at all (a quick look before any file is opened).
pub fn may_cite(text: &str) -> bool {
    text.contains('/')
}

/// The path as the bridge looks for it: `~/x` is the sandbox's home, `/root/x`.
fn as_seen(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => format!("/root/{rest}"),
        None => path.to_string(),
    }
}

/// The paths `text` cites (its `MEDIA:` lines left out), in order, each once, as the bridge looks for them.
pub fn cited_paths(text: &str) -> Vec<String> {
    let (body, _) = crate::media::extract_media(text);
    let mut out: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        let path = raw.trim_end_matches(['.', ',', ';', ':', '!', '?', '…']);
        let name = path.rsplit('/').next().unwrap_or_default();
        if path.starts_with("//") || path.contains("/../") || !name.contains('.') || name.ends_with('.') {
            return;
        }
        let seen = as_seen(path);
        if !out.contains(&seen) {
            out.push(seen);
        }
    };
    for caps in QUOTED.captures_iter(&body) {
        push(&caps[1]);
    }
    // a link's path (`https://host/x.png`, `host.fr/x.png`) never starts after a space or an opening mark
    for caps in BARE.captures_iter(&body) {
        push(&caps[1]);
    }
    out
}

/// The files `text` cites that `agent` may hand the person and that it doesn't hand over yet (see the module's rules).
pub fn cited(config: &Config, agent: &AgentConfig, text: &str) -> Vec<String> {
    if !may_cite(text) {
        return Vec::new();
    }
    let carried: Vec<String> = crate::media::extract_media(text).1.iter().map(|p| as_seen(p)).collect();
    let mut out = Vec::new();
    for path in cited_paths(text) {
        if out.len() >= MAX_CARDS {
            break;
        }
        if carried.contains(&path) {
            continue;
        }
        if let Some(size) = crate::sent_files::cited_file(config, agent, &path) {
            tracing::info!(target: "bipbridge::media", agent = %agent.name, path = %path, size = size,
                "a file the reply cites handed over (MEDIA line added)");
            out.push(path);
        }
    }
    out
}

/// `reply` with a `MEDIA:` line for each of `paths` at its end (trailing blank lines tidied).
pub fn with_cards(reply: &str, paths: &[String]) -> String {
    let mut fixed = reply.trim_end().to_string();
    for path in paths {
        if !fixed.is_empty() {
            fixed.push('\n');
        }
        fixed.push_str(&format!("MEDIA:{path}"));
    }
    fixed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_paths_a_reply_cites() {
        let text = "C'est dans /envoi/notes.md. Et `/fichiers/2026-10/mon devis.pdf`, puis ~/rapport.txt !\n\
            Voir https://exemple.fr/image.png ou exemple.fr/x.png, (/workspace/out/graph.png), « /partage/liste.csv »\n\
            Le dossier /envoi/ et /envoi/sans-extension, /envoi/../secret.txt\n\
            MEDIA:/envoi/deja.pdf";
        assert_eq!(
            cited_paths(text),
            [
                "/fichiers/2026-10/mon devis.pdf",
                "/envoi/notes.md",
                "/root/rapport.txt",
                "/workspace/out/graph.png",
                "/partage/liste.csv",
            ]
        );
        assert!(cited_paths("**/envoi/a.md**").contains(&"/envoi/a.md".to_string()));
        assert!(cited_paths("Rien ici.").is_empty());
    }

    #[test]
    fn cards_at_the_end() {
        assert_eq!(
            with_cards("Voilà /envoi/a.md\n\n", &["/envoi/a.md".into(), "/root/b.txt".into()]),
            "Voilà /envoi/a.md\nMEDIA:/envoi/a.md\nMEDIA:/root/b.txt"
        );
    }

    fn setup() -> (tempfile::TempDir, Config, AgentConfig) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("data/outbox.db").to_string_lossy().into_owned();
        let mut agent = AgentConfig { name: "vie".into(), ..AgentConfig::default() };
        let uploads = dir.path().join("uploads");
        std::fs::create_dir_all(uploads.join("2026-10")).unwrap();
        agent.upload_dir_host = Some(uploads.to_string_lossy().into_owned());
        agent.upload_dir_container = Some("/fichiers".into());
        agent.shares = "external".into();
        config.agents.insert(agent.name.clone(), agent.clone());
        (dir, config, agent)
    }

    #[test]
    fn only_its_own_files_never_through_a_link() {
        let (dir, mut config, agent) = setup();
        let outbox = crate::agent_files::external_outbox(&config, "vie");
        std::fs::create_dir_all(&outbox).unwrap();
        std::fs::write(outbox.join("notes.md"), b"# Notes").unwrap();
        std::fs::write(outbox.join("auth.json"), b"{}").unwrap();
        std::fs::write(outbox.join("script.sh"), b"x").unwrap();
        std::fs::write(dir.path().join("uploads/2026-10/0123456789ab-devis.pdf"), b"%PDF").unwrap();
        std::fs::write(dir.path().join("secret.txt"), b"no").unwrap();
        std::os::unix::fs::symlink(dir.path().join("secret.txt"), outbox.join("lien.txt")).unwrap();
        std::os::unix::fs::symlink(dir.path(), outbox.join("haut")).unwrap();
        let outside = dir.path().join("secret.txt").to_string_lossy().into_owned();
        let reply = format!(
            "C'est dans /envoi/notes.md et /fichiers/2026-10/0123456789ab-devis.pdf. Pas {outside}, ni \
             /envoi/lien.txt, /envoi/haut/secret.txt, /envoi/auth.json, /envoi/script.sh, /envoi/absent.md, \
             /etc/hosts.txt"
        );
        assert_eq!(cited(&config, &agent, &reply), ["/envoi/notes.md", "/fichiers/2026-10/0123456789ab-devis.pdf"]);
        // already handed over: no second card
        let carried = "Voilà /envoi/notes.md\nMEDIA:/envoi/notes.md";
        assert!(cited(&config, &agent, carried).is_empty());
        // another agent never gets them
        let other = AgentConfig { name: "lea".into(), shares: "external".into(), ..AgentConfig::default() };
        assert!(cited(&config, &other, &reply).is_empty());
        // over the size limit: nothing
        config.limits.upload_max_bytes = 3;
        assert_eq!(cited(&config, &agent, "/envoi/notes.md"), Vec::<String>::new());
    }
}
