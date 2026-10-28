//! Keeping an archive's index up to date, and reading one of its documents back.
//!
//! A folder: walked one folder at a time (never through a link, hidden and secret-looking files left out), each file
//! compared with what the index has (size, modification time): only new or changed files are read again, gone ones
//! taken out at the end of a full walk. Changes are committed every few hundred files, so a walk stopped halfway (the
//! bridge restarted) keeps what it did and the next one goes quickly over it. Memory stays small whatever the folder's
//! size: one file's text at a time.
//!
//! A mailbox: read-only (`EXAMINE`, `BODY.PEEK`: nothing is marked read, moved or deleted), its folders but the trash,
//! spam and drafts (only « All Mail » on Gmail, which has everything else), over the period the person chose. Each
//! folder is followed by its UIDVALIDITY and the last UID indexed: the next run fetches only the newer messages, in
//! batches; a run indexes at most `MAIL_PER_RUN` messages and goes on at the next one. Only the text is indexed
//! (headers, body stripped of HTML, the attachments' names), and nothing of it is kept but in the index's words.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::extract::{self, Doc, Format};
use super::index::Index;
use crate::mailbox::{self, Connect, Connection, MailError};
use crate::tools_knowledge::imap::Data;

/// Files read between two commits.
const BATCH: usize = 200;
/// Messages fetched per IMAP command.
const MAIL_BATCH: usize = 25;
/// Messages indexed per run at most (a big mailbox goes on at the next run).
pub const MAIL_PER_RUN: usize = 20_000;
/// A run stops after this long and goes on at the next one.
pub const RUN_LIMIT: Duration = Duration::from_secs(45 * 60);
/// What is fetched of a message to index it (its text, rarely its attachments).
const MAIL_BYTES: usize = 300_000;
/// Mailbox folders indexed at most.
const MAX_FOLDERS: usize = 60;
/// Separates a mail document's parts in its location.
pub const SEP: char = '\u{1f}';

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub files: usize,
    pub read: usize,
    pub unreadable: usize,
    pub complete: bool,
}

/// A file read whole (at most `limit` bytes), never through a link; a file of a podman volume this account can't
/// open is read as the sandboxes' « root » (its owner is a user of the sandbox), by this binary and the same walk
/// (`bipbridge fs-as-root read`, beneath.rs), never `head` on a path that a folder swapped for a link would lead out.
pub fn read_file(root: &Path, relative: &Path, limit: u64, unshare: Option<&str>) -> std::io::Result<Vec<u8>> {
    match crate::beneath::open_file_exact(root, relative) {
        Ok(file) => {
            let mut out = Vec::new();
            file.take(limit).read_to_end(&mut out)?;
            Ok(out)
        }
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied && unshare.is_some() => {
            let podman = unshare.unwrap_or_default();
            let limit = limit.to_string();
            let args =
                [std::ffi::OsStr::new("read"), root.as_os_str(), relative.as_os_str(), std::ffi::OsStr::new(&limit)];
            let out = crate::beneath::as_root(podman, &args)?;
            if !out.status.success() {
                return Err(err);
            }
            Ok(out.stdout)
        }
        Err(err) => Err(err),
    }
}

/// A file's documents: one, or one per message of an mbox (`#offset` in their location).
fn documents(
    root: &Path,
    relative: &Path,
    format: Format,
    size: u64,
    unshare: Option<&str>,
    each: &mut dyn FnMut(String, Doc),
) -> std::io::Result<()> {
    let name = relative.to_string_lossy().into_owned();
    if format == Format::Mbox {
        let file = crate::beneath::open_file_exact(root, relative)?;
        return extract::mbox_messages(file, |offset, raw| {
            each(format!("{name}#{offset}"), extract::mail(raw));
            true
        });
    }
    if size > extract::MAX_FILE {
        // too big to read: found by its name
        let doc = Doc { title: name.rsplit('/').next().unwrap_or(&name).to_string(), ..Doc::default() };
        each(name, doc);
        return Ok(());
    }
    let doc = match format {
        Format::Pdf => {
            // pdftotext reads the file opened here (no link on the way), not its path
            let file = crate::beneath::open_file_exact(root, relative)?;
            let text = if unshare.is_none() { extract::pdf_text(file) } else { None };
            extract::pdf(&name, text)
        }
        Format::Eml => extract::mail(&read_file(root, relative, extract::MAX_FILE, unshare)?),
        Format::Html => extract::html(&name, &extract::decode(&read_file(root, relative, extract::MAX_FILE, unshare)?)),
        Format::Text | Format::Markdown | Format::Mbox => {
            extract::note(&name, &extract::decode(&read_file(root, relative, extract::MAX_FILE, unshare)?))
        }
    };
    each(name, doc);
    Ok(())
}

/// Whether a name is left out: hidden, or secret-looking (keys, tokens, configs).
fn left_out(relative: &Path) -> bool {
    crate::sent_files::secret_looking(relative)
}

/// The folder's index brought up to date. `progress`: the files gone through so far.
pub fn index_folder(
    root: &Path,
    index: &Index,
    unshare: Option<&str>,
    progress: &dyn Fn(usize),
) -> anyhow::Result<Report> {
    let started = Instant::now();
    let pass: i64 = index.meta("pass").and_then(|p| p.parse().ok()).unwrap_or(0) + 1;
    index.set_meta("pass", &pass.to_string())?;
    let mut report = Report::default();
    let mut stack: Vec<PathBuf> = vec![PathBuf::new()];
    let mut in_batch = 0;
    index.begin()?;
    // The root as it was checked (`refused_folder`), and every folder below it through descriptors: an archive inside
    // a folder its agent's sandbox writes in (`/workspace/notes`) can't be swapped for a link to this machine's files.
    let top = crate::beneath::Dir::open_exact(root)?;
    while let Some(folder) = stack.pop() {
        let entries = match top.sub(&folder).and_then(|dir| dir.entries()) {
            Ok(entries) => entries,
            Err(_) => {
                report.unreadable += 1;
                continue;
            }
        };
        for (name, meta) in entries {
            let relative = folder.join(&name);
            if name.to_string_lossy().starts_with('.') || left_out(&relative) {
                continue;
            }
            match meta.kind {
                crate::beneath::Kind::Dir => {
                    if relative.components().count() < crate::beneath::MAX_DEPTH {
                        stack.push(relative);
                    }
                    continue;
                }
                crate::beneath::Kind::File => {}
                _ => continue,
            }
            let Some(format) = extract::format_of(&name.to_string_lossy()) else { continue };
            report.files += 1;
            let file = relative.to_string_lossy().into_owned();
            let (size, mtime) = (meta.size as i64, meta.mtime);
            if index.file_state(&file) == Some((size, mtime)) {
                index.mark_seen(&file, pass)?;
            } else {
                index.remove_file(&file)?;
                let mut added = 0;
                let result = documents(root, &relative, format, meta.size, unshare, &mut |loc, doc| {
                    if index.insert(&loc, &file, size, mtime, format.kind(), &doc, pass).is_ok() {
                        added += 1;
                    }
                });
                if result.is_err() && added == 0 {
                    report.unreadable += 1;
                }
                report.read += 1;
                in_batch += 1;
            }
            if in_batch >= BATCH || report.files % 1000 == 0 {
                index.commit()?;
                progress(report.files);
                in_batch = 0;
                if started.elapsed() > RUN_LIMIT {
                    return Ok(report); // not complete: the gone files are taken out at the next full walk
                }
                index.begin()?;
            }
        }
    }
    index.prune(pass)?;
    index.commit()?;
    progress(report.files);
    report.complete = true;
    Ok(report)
}

// -- a mailbox -----------------------------------------------------------------------------------------------------

/// A mail document's location: `folder␟uidvalidity␟uid`.
pub fn mail_loc(folder: &str, validity: u64, uid: u64) -> String {
    format!("{folder}{SEP}{validity}{SEP}{uid}")
}

pub fn parse_mail_loc(loc: &str) -> Option<(String, u64, u64)> {
    let mut parts = loc.rsplitn(3, SEP);
    let uid = parts.next()?.parse().ok()?;
    let validity = parts.next()?.parse().ok()?;
    Some((parts.next()?.to_string(), validity, uid))
}

/// The folders indexed: « All Mail » alone when the server has one (Gmail), else all but the trash, spam and drafts.
pub fn mail_folders(conn: &mut Connection) -> anyhow::Result<Vec<String>> {
    let (status, data) = conn.list()?;
    if status != "OK" {
        return Err(MailError("the folders can't be listed".into()).into());
    }
    let rows = mailbox::list_rows(&data);
    let name_of = |raw: &str| raw.trim().trim_matches('"').to_string();
    if let Some((_, all)) = rows.iter().find(|(flags, _)| flags.to_lowercase().contains("\\all")) {
        return Ok(vec![name_of(all)]);
    }
    let skipped = ["\\trash", "\\junk", "\\drafts", "\\noselect", "\\nonexistent"];
    let skipped_names = ["trash", "corbeille", "spam", "junk", "drafts", "brouillons", "papierkorb", "papelera"];
    Ok(rows
        .into_iter()
        .filter(|(flags, name)| {
            let flags = flags.to_lowercase();
            let name = name_of(name).to_lowercase();
            !skipped.iter().any(|s| flags.contains(s)) && !skipped_names.iter().any(|s| name.ends_with(s))
        })
        .map(|(_, name)| name_of(&name))
        .take(MAX_FOLDERS)
        .collect())
}

fn search_uids(conn: &mut Connection, since: Option<chrono::NaiveDate>) -> anyhow::Result<Vec<u64>> {
    let date = since.map(crate::tools_knowledge::email::imap_date);
    let args: Vec<&str> = match &date {
        Some(d) => vec!["SINCE", d.as_str()],
        None => vec!["ALL"],
    };
    let (status, data) = conn.uid("SEARCH", &args)?;
    if status != "OK" {
        return Err(MailError("the search was refused".into()).into());
    }
    let line: &[u8] = match data.first() {
        Some(Data::Line(line)) | Some(Data::Literal(line, _)) => line,
        _ => b"",
    };
    let mut uids: Vec<u64> = String::from_utf8_lossy(line).split_whitespace().filter_map(|w| w.parse().ok()).collect();
    uids.sort_unstable();
    Ok(uids)
}

static UID: std::sync::LazyLock<regex::bytes::Regex> =
    std::sync::LazyLock::new(|| regex::bytes::Regex::new(r"UID (\d+)").expect("valid regex"));

/// Messages fetched (BODY.PEEK: never marked read): (uid, raw), `bytes` of each at most.
pub fn fetch(conn: &mut Connection, uids: &[u64], bytes: usize) -> anyhow::Result<Vec<(u64, Vec<u8>)>> {
    let mut out = Vec::new();
    for chunk in uids.chunks(MAIL_BATCH) {
        let joined = chunk.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        let (status, data) = conn.uid("FETCH", &[&joined, &format!("(UID BODY.PEEK[]<0.{bytes}>)")])?;
        if status != "OK" {
            continue;
        }
        for part in &data {
            let Data::Literal(line, raw) = part else { continue };
            if let Some(uid) = UID.captures(line).and_then(|c| std::str::from_utf8(&c[1]).ok()?.parse().ok()) {
                out.push((uid, raw.clone()));
            }
        }
    }
    Ok(out)
}

/// Opens `folder` read-only: its UIDVALIDITY (0 when the server gives none).
pub fn examine(conn: &mut Connection, folder: &str) -> anyhow::Result<u64> {
    let (status, _) = conn.select(&mailbox::quote(folder), true)?;
    if status != "OK" {
        return Err(MailError(format!("no folder {folder}")).into());
    }
    Ok(conn.uid_validity().unwrap_or(0))
}

/// The mailbox's index brought up to date over `years` (0: all).
pub fn index_mail(
    account: &crate::Dict,
    years: u32,
    index: &Index,
    opener: &Connect,
    progress: &dyn Fn(usize),
) -> anyhow::Result<Report> {
    let started = Instant::now();
    let since = (years > 0)
        .then(|| chrono::Local::now().date_naive() - chrono::Days::new(365 * years as u64 + years as u64 / 4));
    let mut conn = opener(account)?;
    let result = (|| -> anyhow::Result<Report> {
        let mut report = Report { complete: true, ..Report::default() };
        let folders = mail_folders(&mut conn)?;
        for folder in &folders {
            let validity = match examine(&mut conn, folder) {
                Ok(v) => v,
                Err(_) => {
                    report.unreadable += 1;
                    continue;
                }
            };
            let state_key = format!("mail:{folder}");
            let (known_validity, done): (u64, u64) = index
                .meta(&state_key)
                .and_then(|s| s.split_once(' ').and_then(|(v, d)| Some((v.parse().ok()?, d.parse().ok()?))))
                .unwrap_or((validity, 0));
            let mut done = done;
            if known_validity != validity {
                // the server renumbered the folder: its messages indexed again
                index.remove_file(folder)?;
                done = 0;
            }
            let uids = search_uids(&mut conn, since)?;
            report.files += uids.len();
            let todo: Vec<u64> = uids.iter().copied().filter(|u| *u > done).collect();
            let room = MAIL_PER_RUN.saturating_sub(report.read);
            let (now, later) = todo.split_at(todo.len().min(room));
            for chunk in now.chunks(MAIL_BATCH * 4) {
                let fetched = fetch(&mut conn, chunk, MAIL_BYTES)?;
                index.begin()?;
                for (uid, raw) in &fetched {
                    let doc = extract::mail(raw);
                    index.insert(&mail_loc(folder, validity, *uid), folder, 0, 0, "mail", &doc, 0)?;
                }
                done = chunk.iter().copied().max().unwrap_or(done).max(done);
                index.set_meta(&state_key, &format!("{validity} {done}"))?;
                index.commit()?;
                report.read += chunk.len();
                progress(report.read);
                if started.elapsed() > RUN_LIMIT {
                    report.complete = false;
                    return Ok(report);
                }
            }
            index.set_meta(&state_key, &format!("{validity} {done}"))?;
            if !later.is_empty() {
                report.complete = false;
                continue;
            }
            // messages deleted from the mailbox, or now older than the period: taken out
            let present: HashSet<u64> = uids.into_iter().collect();
            index.begin()?;
            for loc in index.locs_of(folder)? {
                match parse_mail_loc(&loc) {
                    Some((_, v, uid)) if v == validity && present.contains(&uid) => {}
                    _ => {
                        index.remove_loc(&loc)?;
                    }
                }
            }
            index.commit()?;
        }
        // folders no longer indexed
        let kept: HashSet<&String> = folders.iter().collect();
        let mut stmt = index.conn.prepare("SELECT DISTINCT file FROM docs")?;
        let files: Vec<String> = stmt.query_map([], |r| r.get(0))?.flatten().collect();
        drop(stmt);
        for file in files.iter().filter(|f| !kept.contains(f)) {
            index.remove_file(file)?;
        }
        Ok(report)
    })();
    let _ = conn.logout();
    result
}

/// Messages' texts fetched for snippets or reading: (uid → raw), one connection, `bytes` each.
pub fn fetch_messages(
    account: &crate::Dict,
    opener: &Connect,
    folder: &str,
    validity: u64,
    uids: &[u64],
    bytes: usize,
) -> anyhow::Result<Vec<(u64, Vec<u8>)>> {
    let mut conn = opener(account)?;
    let result = (|| {
        let now = examine(&mut conn, folder)?;
        if validity != 0 && now != 0 && now != validity {
            return Err(MailError("the mailbox renumbered this folder: search again".into()).into());
        }
        fetch(&mut conn, uids, bytes)
    })();
    let _ = conn.logout();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mail_locations_go_both_ways() {
        let loc = mail_loc("[Gmail]/Tous les messages", 7, 42);
        assert_eq!(parse_mail_loc(&loc), Some(("[Gmail]/Tous les messages".into(), 7, 42)));
        assert_eq!(parse_mail_loc("notes/a.md"), None);
    }

    #[test]
    fn a_folder_is_indexed_then_only_its_changes() {
        let dir = tempfile::tempdir().unwrap();
        // an archive's folder is a real path (`locate` resolves it): the walks check it still leads there
        let root = std::fs::canonicalize(dir.path()).unwrap().join("docs");
        std::fs::create_dir_all(root.join("2024")).unwrap();
        std::fs::create_dir_all(root.join(".cache")).unwrap();
        std::fs::write(root.join("2024/achat.md"), "# Commande vélo\nDate: 2024-05-02\nMerci").unwrap();
        std::fs::write(root.join("2024/page.html"), "<title>Billet</title><p>Train pour Lyon</p>").unwrap();
        std::fs::write(root.join(".cache/x.md"), "secret").unwrap();
        std::fs::write(root.join("token.txt"), "secret").unwrap();
        std::fs::write(root.join("image.png"), "png").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("link.txt")).unwrap();
        let index = Index::open(&dir.path().join("i.sqlite")).unwrap();
        let report = index_folder(&root, &index, None, &|_| {}).unwrap();
        assert_eq!((report.files, report.read, report.complete), (2, 2, true));
        assert_eq!(index.count(), 2);
        let again = index_folder(&root, &index, None, &|_| {}).unwrap();
        assert_eq!((again.files, again.read), (2, 0));
        std::fs::remove_file(root.join("2024/page.html")).unwrap();
        index_folder(&root, &index, None, &|_| {}).unwrap();
        assert_eq!(index.count(), 1);
    }

    /// An archive in a folder its agent's sandbox writes in (`/workspace/notes`): the sandbox swaps that folder for a
    /// link to this machine's files after it was checked. The walk and the reads refuse it (security audit
    /// 2026-10-27).
    #[test]
    fn an_archive_folder_swapped_for_a_link_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let (root, home) = (base.join("workspace/notes"), base.join("home"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(root.join("a.md"), "notes").unwrap();
        std::fs::write(home.join("a.md"), "home secret").unwrap();
        let index = Index::open(&base.join("i.sqlite")).unwrap();
        assert_eq!(index_folder(&root, &index, None, &|_| {}).unwrap().files, 1);
        assert_eq!(read_file(&root, Path::new("a.md"), 100, None).unwrap(), b"notes");
        std::fs::rename(&root, base.join("workspace/notes.real")).unwrap();
        std::os::unix::fs::symlink(&home, &root).unwrap();
        assert!(index_folder(&root, &Index::open(&base.join("j.sqlite")).unwrap(), None, &|_| {}).is_err());
        assert!(read_file(&root, Path::new("a.md"), 100, None).is_err());
    }
}
