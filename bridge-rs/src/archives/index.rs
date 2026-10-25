//! An archive's full-text index: one SQLite file (FTS5, `unicode61 remove_diacritics 2`: « eleve » finds « élève »).
//! Contentless: the words are indexed, the texts are not kept (a hit's snippet is read again from its file, or fetched
//! from the mailbox), so the index stays a fraction of the archive's size. Derived data: deleted, it is made again.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use super::extract::{self, Doc};

const SCHEMA: &str = "2";

pub struct Index {
    pub conn: Connection,
    pub path: PathBuf,
}

/// One document found.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub key: String,
    pub loc: String,
    pub file: String,
    pub kind: String,
    pub title: String,
    pub sender: String,
    pub date: String,
    pub preview: String,
}

/// One document as the records' extraction sees it: its hit, its rowid, its kind of content and its file's state.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: i64,
    pub hit: Hit,
    pub category: String,
    pub size: i64,
    pub mtime: i64,
}

/// A document's key: 16 hex of its location (its path, `path#offset` in an mbox, `folder␟validity␟uid` in a
/// mailbox), the same across rebuilds.
pub fn key_of(loc: &str) -> String {
    hex::encode(Sha256::digest(loc.as_bytes()))[..16].to_string()
}

/// The schema's version written in `meta`, None when there is no `meta` table yet.
fn schema_of(conn: &Connection) -> rusqlite::Result<Option<String>> {
    let has_meta: bool =
        conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta')", [], |r| {
            r.get(0)
        })?;
    if !has_meta {
        return Ok(None);
    }
    conn.query_row("SELECT value FROM meta WHERE key='schema'", [], |r| r.get(0)).optional()
}

/// The tables (dropped first) and the schema's version.
fn create_tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS fts; DROP TABLE IF EXISTS docs; DROP TABLE IF EXISTS meta;
         CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE docs(id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE, loc TEXT NOT NULL UNIQUE,
             file TEXT NOT NULL, size INTEGER NOT NULL DEFAULT 0, mtime INTEGER NOT NULL DEFAULT 0,
             kind TEXT NOT NULL, title TEXT NOT NULL, sender TEXT NOT NULL, date TEXT NOT NULL,
             category TEXT NOT NULL, preview TEXT NOT NULL, seen INTEGER NOT NULL DEFAULT 0);
         CREATE INDEX docs_file ON docs(file);
         CREATE INDEX docs_date ON docs(date);
         CREATE VIRTUAL TABLE fts USING fts5(title, sender, body, content='', contentless_delete=1,
             tokenize='unicode61 remove_diacritics 2');",
    )?;
    conn.execute("INSERT INTO meta(key, value) VALUES('schema', ?1)", [SCHEMA])?;
    Ok(())
}

/// `work` in one write transaction (BEGIN IMMEDIATE: another connection's waits, then sees all of it or none).
fn in_write(conn: &Connection, work: impl FnOnce(&Connection) -> rusqlite::Result<()>) -> rusqlite::Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    match work(conn) {
        Ok(()) => conn.execute_batch("COMMIT"),
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err)
        }
    }
}

impl Index {
    pub fn open(path: &Path) -> anyhow::Result<Index> {
        if let Some(parent) = path.parent() {
            crate::py::makedirs(&parent.to_string_lossy(), 0o700)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        // The app's view opens the index while the update writes it: a failed read is an error, never a reason to
        // start the tables again (that wiped a finished index), and the tables are made once, under the write lock.
        if schema_of(&conn)?.as_deref() != Some(SCHEMA) {
            in_write(&conn, |conn| {
                if schema_of(conn)?.as_deref() != Some(SCHEMA) {
                    create_tables(conn)?;
                }
                Ok(())
            })?;
        }
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        Ok(Index { conn, path: path.to_path_buf() })
    }

    /// Every document and meta value dropped (the archive's source changed), in place: the files stay, as other
    /// connections (the app's view) may have them open.
    pub fn reset(&self) -> rusqlite::Result<()> {
        in_write(&self.conn, create_tables)
    }

    pub fn meta(&self, key: &str) -> Option<String> {
        self.conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0)).optional().ok().flatten()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value=?2",
            [key, value],
        )?;
        Ok(())
    }

    pub fn begin(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch("BEGIN")
    }

    pub fn commit(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch("COMMIT")
    }

    /// A file's size and modification time as last indexed.
    pub fn file_state(&self, file: &str) -> Option<(i64, i64)> {
        self.conn
            .query_row("SELECT size, mtime FROM docs WHERE file=?1 LIMIT 1", [file], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .ok()
            .flatten()
    }

    pub fn mark_seen(&self, file: &str, pass: i64) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE docs SET seen=?1 WHERE file=?2", params![pass, file])?;
        Ok(())
    }

    fn remove_where(&self, condition: &str, value: &dyn rusqlite::ToSql) -> rusqlite::Result<usize> {
        self.conn
            .execute(&format!("DELETE FROM fts WHERE rowid IN (SELECT id FROM docs WHERE {condition})"), [value])?;
        self.conn.execute(&format!("DELETE FROM docs WHERE {condition}"), [value])
    }

    /// A file's documents taken out (changed or gone).
    pub fn remove_file(&self, file: &str) -> rusqlite::Result<usize> {
        self.remove_where("file=?1", &file)
    }

    pub fn remove_loc(&self, loc: &str) -> rusqlite::Result<usize> {
        self.remove_where("loc=?1", &loc)
    }

    /// The documents not seen in pass `pass` (their files gone).
    pub fn prune(&self, pass: i64) -> rusqlite::Result<usize> {
        self.remove_where("seen<?1", &pass)
    }

    /// The locations of a file's documents (a mailbox folder's messages).
    pub fn locs_of(&self, file: &str) -> rusqlite::Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT loc FROM docs WHERE file=?1")?;
        let rows = stmt.query_map([file], |r| r.get::<_, String>(0))?;
        rows.collect()
    }

    /// One document added (or replaced).
    #[allow(clippy::too_many_arguments)]
    pub fn insert(
        &self,
        loc: &str,
        file: &str,
        size: i64,
        mtime: i64,
        kind: &str,
        doc: &Doc,
        pass: i64,
    ) -> rusqlite::Result<()> {
        self.remove_loc(loc)?;
        let preview = extract::cut(&crate::tools_knowledge::squeeze(&doc.text), extract::PREVIEW_CHARS);
        self.conn.execute(
            "INSERT INTO docs(key, loc, file, size, mtime, kind, title, sender, date, category, preview, seen)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                key_of(loc),
                loc,
                file,
                size,
                mtime,
                kind,
                doc.title,
                doc.sender,
                doc.date,
                extract::category(doc),
                preview,
                pass
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "INSERT INTO fts(rowid, title, sender, body) VALUES(?1, ?2, ?3, ?4)",
            params![id, doc.title, doc.sender, doc.text],
        )?;
        Ok(())
    }

    pub fn count(&self) -> i64 {
        self.conn.query_row("SELECT count(*) FROM docs", [], |r| r.get(0)).unwrap_or(0)
    }

    /// Documents per year and kind: [(year, category, n)].
    pub fn by_year(&self) -> Vec<(String, String, i64)> {
        let Ok(mut stmt) = self.conn.prepare(
            "SELECT substr(date, 1, 4) AS y, category, count(*) FROM docs WHERE date != '' GROUP BY y, category ORDER BY y DESC",
        ) else {
            return Vec::new();
        };
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    pub fn get(&self, key: &str) -> Option<Hit> {
        self.conn
            .query_row(
                "SELECT key, loc, file, kind, title, sender, date, preview FROM docs WHERE key=?1",
                [key],
                hit_of,
            )
            .optional()
            .ok()
            .flatten()
    }

    /// Documents matching `expr` (an FTS5 query, None: any), between two dates (YYYY-MM-DD, empty: open), the best
    /// first (or the newest), with how many match in all.
    pub fn search(
        &self,
        expr: Option<&str>,
        after: &str,
        before: &str,
        limit: usize,
        newest: bool,
    ) -> rusqlite::Result<(Vec<Hit>, i64)> {
        let dates = "(?2 = '' OR d.date >= ?2) AND (?3 = '' OR (d.date != '' AND d.date <= ?3))";
        let (select, count) = match expr {
            Some(_) => (
                format!(
                    "SELECT d.key, d.loc, d.file, d.kind, d.title, d.sender, d.date, d.preview FROM fts JOIN docs d ON d.id = fts.rowid
                     WHERE fts MATCH ?1 AND {dates} ORDER BY {} LIMIT ?4",
                    if newest { "d.date DESC" } else { "bm25(fts, 6.0, 2.0, 1.0)" }
                ),
                format!("SELECT count(*) FROM fts JOIN docs d ON d.id = fts.rowid WHERE fts MATCH ?1 AND {dates}"),
            ),
            None => (
                format!(
                    "SELECT d.key, d.loc, d.file, d.kind, d.title, d.sender, d.date, d.preview FROM docs d
                     WHERE ?1 = '' AND {dates} ORDER BY d.date DESC LIMIT ?4"
                ),
                format!("SELECT count(*) FROM docs d WHERE ?1 = '' AND {dates}"),
            ),
        };
        let expr = expr.unwrap_or("");
        let mut stmt = self.conn.prepare(&select)?;
        let hits = stmt
            .query_map(params![expr, after, before, limit as i64], hit_of)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let total = self.conn.query_row(&count, params![expr, after, before], |r| r.get(0))?;
        Ok((hits, total))
    }

    /// Every document, the oldest first (those without a date last): what the records' extraction walks through.
    pub fn rows(&self) -> rusqlite::Result<Vec<Row>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, key, loc, file, kind, title, sender, date, category, size, mtime, preview FROM docs
             ORDER BY date = '', date, id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Row {
                id: r.get(0)?,
                hit: Hit {
                    key: r.get(1)?,
                    loc: r.get(2)?,
                    file: r.get(3)?,
                    kind: r.get(4)?,
                    title: r.get(5)?,
                    sender: r.get(6)?,
                    date: r.get(7)?,
                    preview: r.get(11)?,
                },
                category: r.get(8)?,
                size: r.get(9)?,
                mtime: r.get(10)?,
            })
        })?;
        rows.collect()
    }

    /// The ids of the documents matching an FTS5 expression.
    pub fn matching_ids(&self, expr: &str) -> rusqlite::Result<std::collections::HashSet<i64>> {
        let mut stmt = self.conn.prepare("SELECT rowid FROM fts WHERE fts MATCH ?1")?;
        let ids = stmt.query_map([expr], |r| r.get::<_, i64>(0))?;
        ids.collect()
    }

    /// The index's size on disk (with its journal).
    pub fn bytes(path: &Path) -> u64 {
        ["", "-wal", "-shm"]
            .iter()
            .filter_map(|suffix| std::fs::metadata(format!("{}{suffix}", path.display())).ok())
            .map(|m| m.len())
            .sum()
    }
}

fn hit_of(r: &rusqlite::Row) -> rusqlite::Result<Hit> {
    Ok(Hit {
        key: r.get(0)?,
        loc: r.get(1)?,
        file: r.get(2)?,
        kind: r.get(3)?,
        title: r.get(4)?,
        sender: r.get(5)?,
        date: r.get(6)?,
        preview: r.get(7)?,
    })
}

const STOP: [&str; 64] = [
    "le", "la", "les", "un", "une", "des", "du", "de", "et", "ou", "en", "au", "aux", "pour", "par", "sur", "dans",
    "avec", "que", "qui", "quoi", "quand", "est", "ai", "as", "mon", "ma", "mes", "je", "tu", "il", "elle", "on",
    "nous", "vous", "the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "with", "when", "did", "my", "what",
    "who", "der", "die", "das", "und", "ein", "eine", "el", "los", "las", "y", "con", "por", "mi",
];

/// The words and phrases of a person's or an agent's query: (FTS5 expression, words for the snippets). `any`: a
/// document with one of them is enough (else all). Words of 3 letters or more match as prefixes (« achat » finds
/// « achats »); "a phrase" in quotes matches as written; the most common little words are left out.
pub fn match_expr(query: &str, any: bool) -> Option<(String, Vec<String>)> {
    let mut parts: Vec<String> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let pieces: Vec<&str> = query.split('"').collect();
    for (i, piece) in pieces.iter().enumerate() {
        let tokens: Vec<String> =
            piece.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(|t| t.to_lowercase()).collect();
        if i % 2 == 1 && pieces.len() > i + 1 {
            // a quoted phrase
            if !tokens.is_empty() {
                parts.push(format!("\"{}\"", tokens.join(" ")));
                words.push(tokens.join(" "));
            }
            continue;
        }
        for token in tokens {
            if token.chars().count() < 2 || STOP.contains(&token.as_str()) {
                continue;
            }
            words.push(token.clone());
            parts.push(if token.chars().count() >= 3 { format!("\"{token}\"*") } else { format!("\"{token}\"") });
        }
    }
    parts.dedup();
    parts.truncate(12);
    if parts.is_empty() {
        return None;
    }
    Some((parts.join(if any { " OR " } else { " AND " }), words))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_as_fts5_expressions() {
        assert_eq!(match_expr("Quand ai-je acheté mon vélo ?", false).unwrap().0, "\"acheté\"* AND \"vélo\"*");
        assert_eq!(match_expr("\"carte grise\" voiture", true).unwrap().0, "\"carte grise\" OR \"voiture\"*");
        assert!(match_expr("le la ?", false).is_none());
        assert_eq!(match_expr("a\"bc", false).unwrap().0, "\"bc\"");
    }

    #[test]
    fn an_index_finds_accents_and_prefixes_and_forgets() {
        let dir = tempfile::tempdir().unwrap();
        let index = Index::open(&dir.path().join("i.sqlite")).unwrap();
        let doc = |t: &str, d: &str, body: &str| Doc {
            title: t.into(),
            sender: "Shop".into(),
            date: d.into(),
            text: body.into(),
        };
        index.insert("a.md", "a.md", 1, 1, "note", &doc("Commande vélo", "2024-05-02", "Votre Élève"), 1).unwrap();
        index.insert("b.md", "b.md", 1, 1, "note", &doc("Billet", "2022-01-02", "train pour Lyon"), 1).unwrap();
        let (expr, _) = match_expr("velo eleve", false).unwrap();
        let (hits, total) = index.search(Some(&expr), "", "", 10, false).unwrap();
        assert_eq!((hits.len(), total), (1, 1));
        assert_eq!(hits[0].key, key_of("a.md"));
        let (expr, _) = match_expr("command", false).unwrap();
        assert_eq!(index.search(Some(&expr), "", "", 10, false).unwrap().1, 1);
        assert_eq!(index.search(None, "2023-01-01", "", 10, false).unwrap().1, 1);
        assert_eq!(index.search(None, "", "2023-01-01", 10, false).unwrap().0[0].title, "Billet");
        assert_eq!(index.by_year(), vec![("2024".into(), "purchase".into(), 1), ("2022".into(), "travel".into(), 1)]);
        index.mark_seen("a.md", 2).unwrap();
        assert_eq!(index.prune(2).unwrap(), 1);
        let (expr, _) = match_expr("train", false).unwrap();
        assert_eq!(index.search(Some(&expr), "", "", 10, false).unwrap().1, 0);
        assert_eq!(index.count(), 1);
    }
}
