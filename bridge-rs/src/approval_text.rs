//! An approval in plain words (owner's feedback 2026-10-26: a card said only « demande de faire quelque chose »). The
//! run hub (runs.rs) adds two fields to each `approval.request` Hermes sends, before the apps see it (a conversation's
//! stream, a room's `answering`, the Boîte's `/v1/approvals`) and in the notification:
//!
//! - `bip_summary`: one short sentence in the agent's language, with its name: « Wellness veut lancer un calcul dans
//!   son espace », « Vie veut créer dans Notion : « Réunion du 12 » », « Wellness veut exécuter : python3 analyse.py »;
//! - `bip_detail`: what « Voir le détail technique » unfolds: the code, the whole command, or the server and tool.
//!
//! What Hermes sends, at the commit the installer pins (tools/approval.py, tools/approval_prompt.py,
//! tools/mcp_tool_handlers.py, gateway/platforms/api_server.py `_approval_request_event`): `command` (redacted),
//! `description`, `pattern_key`, `choices`; per gate:
//!
//! - a terminal command: `pattern_key` = `description` = the dangerous pattern's description (« recursive delete »,
//!   « script execution via -e/-c flag », « script execution via heredoc », « shell command via -c/-lc flag »…);
//! - `execute_code`: `pattern_key` « execute_code », `command` `execute_code <<'PY'\n<code>\nPY`;
//! - the MCP trust gate (a tool without `readOnlyHint` on a `trust: untrusted` server): `pattern_key`
//!   « mcp_elicitation », `command` « MCP tool 'X' on UNTRUSTED server 'Y' wants to run… »; its arguments are not in
//!   it, so the tool's `preview` is taken from the run's `tool.started` just before;
//! - an MCP server's own question (elicitation): `pattern_key` « mcp_elicitation », `command` the server's message;
//!   the bridge's card payment is one (`Card payment pay_…`): the apps have their own card for it, left alone;
//! - a plugin's rule: `command` « <tool> (plugin approval rule) »; the agent's own instructions:
//!   « protected_instruction_file ».
//!
//! Anything else gets no sentence: the apps keep their own reading (`ApprovalTopic`). The bridge's own cards (a site
//! for the agent's programs, an account with a vault password) never pass here.
//!
//! Not a Python module: Rust bridge only (owner group: push-runs).

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

/// The sentence and the folded detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Described {
    pub summary: String,
    pub detail: Option<String>,
}

/// The tool Hermes started just before asking (its `tool.started`): its name and its short preview.
#[derive(Debug, Clone, Default)]
pub struct LastTool {
    pub name: String,
    pub preview: String,
}

static TRUST_GATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^MCP tool '([^']+)' on UNTRUSTED server '([^']+)'").expect("valid regex"));
static PLUGIN_RULE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<?([A-Za-z0-9_.:-]+)>? \(plugin approval rule\)").expect("valid regex"));
static EXECUTE_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^execute_code <<'PY'\n(.*?)\n?PY\s*$").expect("valid regex"));
static NON_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^A-Za-z0-9]+").expect("valid regex"));
/// The tools the apps' own reading names better (ApprovalTopic.toolTopic, both apps).
static PERSONAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"calendar|caldav|event|contact|reminder|health|mail|smtp|imap|vault|payment|card|homeassistant|^ha_|climate\.|light\.|switch\.|thermostat")
        .expect("valid regex")
});

const SUMMARY_COMMAND_CHARS: usize = 80;
/// The lines of a command or a script the card shows before « (+N lignes) ».
const SUMMARY_COMMAND_LINES: usize = 3;
const SUMMARY_PREVIEW_CHARS: usize = 80;
const SUMMARY_MESSAGE_CHARS: usize = 160;

/// What an MCP tool does, from its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Create,
    Change,
    Delete,
    Send,
    Use,
}

fn verb_of(tool: &str) -> Verb {
    let words: Vec<String> = NON_WORD.split(&tool.to_lowercase()).map(str::to_string).collect();
    let has = |list: &[&str]| words.iter().any(|w| list.contains(&w.as_str()));
    if has(&["delete", "remove", "trash", "archive", "unlink", "rm", "destroy", "purge"]) {
        Verb::Delete
    } else if has(&["send", "post", "reply", "publish", "comment", "forward", "share", "invite", "notify", "pay"]) {
        Verb::Send
    } else if has(&["create", "add", "new", "insert", "append", "upload", "write", "save", "duplicate", "copy"]) {
        Verb::Create
    } else if has(&["update", "edit", "patch", "set", "move", "rename", "modify", "change", "replace", "toggle"]) {
        Verb::Change
    } else {
        Verb::Use
    }
}

/// « home_assistant » → « Home Assistant ».
fn server_name(server: &str) -> String {
    NON_WORD
        .split(server)
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t\x0B\x0C\r]+").expect("valid regex"));

/// One line with its runs of spaces and tabs folded to one space.
fn folded(line: &str) -> String {
    SPACES.replace_all(line.trim(), " ").to_string()
}

/// At most `max` characters (« … » when cut).
fn cut_line(line: &str, max: usize) -> String {
    if line.chars().count() <= max {
        return line.to_string();
    }
    let head: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", head.trim_end())
}

/// The first line, spaces folded, at most `max` characters (« … » when cut or when more lines follow).
fn cut(text: &str, max: usize) -> String {
    let mut lines = text.trim().lines().map(folded).filter(|l| !l.is_empty());
    let line = lines.next().unwrap_or_default();
    if line.chars().count() <= max && lines.next().is_none() {
        return line;
    }
    let head: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", head.trim_end())
}

/// A command or a script as the card shows it: its first `SUMMARY_COMMAND_LINES` non-empty lines, spaces folded,
/// each at most `max` characters, then « (+N lignes) » when more follow (the whole of it stays in the detail).
fn command_lines(lang: &str, text: &str, max: usize) -> String {
    let lines: Vec<String> = text.trim().lines().map(folded).filter(|l| !l.is_empty()).collect();
    let mut shown: Vec<String> = lines.iter().take(SUMMARY_COMMAND_LINES).map(|l| cut_line(l, max)).collect();
    let more = lines.len().saturating_sub(SUMMARY_COMMAND_LINES);
    if more > 0 {
        let word = match (lang, more) {
            ("fr", 1) => "ligne",
            ("fr", _) => "lignes",
            ("de", 1) => "Zeile",
            ("de", _) => "Zeilen",
            ("es", 1) => "línea",
            ("es", _) => "líneas",
            (_, 1) => "line",
            _ => "lines",
        };
        shown.push(format!("(+{more} {word})"));
    }
    shown.join("\n")
}

fn quoted(lang: &str, text: &str) -> String {
    match lang {
        "fr" => format!("« {text} »"),
        "de" => format!("„{text}“"),
        "es" => format!("«{text}»"),
        _ => format!("“{text}”"),
    }
}

fn pick<'a>(lang: &str, fr: &'a str, en: &'a str, de: &'a str, es: &'a str) -> &'a str {
    match lang {
        "fr" => fr,
        "de" => de,
        "es" => es,
        _ => en,
    }
}

/// The colon before what follows, as each language writes it.
fn colon(lang: &str) -> &'static str {
    if lang == "fr" {
        " : "
    } else {
        ": "
    }
}

fn text<'a>(data: &'a Map<String, Value>, key: &str) -> &'a str {
    data.get(key).and_then(Value::as_str).map(str::trim).unwrap_or("")
}

static DELETES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(delete|deletes|destroys|truncate|drop|rm|wipe|uninstall)\b").expect("valid regex")
});

/// A terminal command Hermes flagged whose description says it deletes something.
fn deletes(description: &str) -> bool {
    DELETES.is_match(description)
}

/// The sentence and detail for an `approval.request` of agent `name`, in `lang` (« fr », « en », « de », « es »);
/// `last`: the tool Hermes started just before; `isolated`: the agent's programs run in its own sandbox
/// (sandbox::isolates), so a script stays « dans son espace »; otherwise it runs on the person's machine and can reach
/// their files, and the card says so with its first lines. None: nothing recognised, the apps keep their own reading.
pub fn describe(name: &str, lang: &str, data: &Map<String, Value>, last: Option<&LastTool>, isolated: bool)
    -> Option<Described> {
    let command = text(data, "command");
    let description = text(data, "description");
    let key = text(data, "pattern_key");
    if command.is_empty() && description.is_empty() {
        return None;
    }
    // the bridge's card payment: the apps' own card
    if command.starts_with("Card payment pay_") {
        return None;
    }
    // the MCP trust gate: a tool that may change something, on a server the person added
    if let Some(caps) = TRUST_GATE.captures(command) {
        let (tool, server) = (caps[1].to_string(), caps[2].to_string());
        // the person's calendar, contacts, reminders, health, mail, home, card: the apps say it better (« veut
        // modifier ton calendrier »), as for the bridge's own tools
        if server == crate::agent_tools::SERVER_NAME || PERSONAL.is_match(&tool.to_lowercase()) {
            return None;
        }
        let preview = last
            .filter(|t| {
                let wanted = NON_WORD.replace_all(&tool.to_lowercase(), "_").to_string();
                let started = NON_WORD.replace_all(&t.name.to_lowercase(), "_").to_string();
                !t.preview.trim().is_empty() && (started == wanted || started.ends_with(&format!("_{wanted}")))
            })
            .map(|t| t.preview.trim().to_string());
        return Some(mcp_tool(name, lang, &tool, &server, preview.as_deref()));
    }
    if key == "mcp_elicitation" {
        // an MCP server's own question
        let said = pick(lang, "{n} demande ta confirmation", "{n} asks you to confirm", "{n} bittet dich um Bestätigung",
                        "{n} te pide que confirmes")
            .replace("{n}", name);
        let summary = format!("{said}{}{}", colon(lang), quoted(lang, &cut(command, SUMMARY_MESSAGE_CHARS)));
        let detail = [command, description].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join("\n\n");
        return Some(Described { summary, detail: Some(detail) });
    }
    if let Some(caps) = PLUGIN_RULE.captures(command) {
        let summary = pick(lang, "{n} veut utiliser l’outil {t}", "{n} wants to use the tool {t}",
                           "{n} möchte das Werkzeug {t} verwenden", "{n} quiere usar la herramienta {t}")
            .replace("{n}", name)
            .replace("{t}", &caps[1]);
        return Some(Described { summary, detail: Some(description.to_string()).filter(|d| !d.is_empty()) });
    }
    if key == "protected_instruction_file" {
        let summary = pick(lang, "{n} veut modifier ses propres consignes", "{n} wants to change its own instructions",
                           "{n} möchte seine eigenen Anweisungen ändern", "{n} quiere cambiar sus propias instrucciones")
            .replace("{n}", name);
        return Some(Described { summary, detail: Some(command.to_string()).filter(|c| !c.is_empty()) });
    }
    // a script it wrote (execute_code, python -c, a heredoc): a computation in its space when the agent is isolated,
    // otherwise a program on the person's machine that can reach their files (audit 2026-10-27 #14)
    let code = EXECUTE_CODE.captures(command).map(|c| c[1].to_string());
    let script = key == "execute_code"
        || code.is_some()
        || description.starts_with("script execution via")
        || key.starts_with("script execution via");
    if script {
        let detail = code.unwrap_or_else(|| command.to_string());
        let summary = if isolated {
            pick(lang, "{n} veut lancer un calcul dans son espace", "{n} wants to run a calculation in its own space",
                 "{n} möchte in seinem Bereich eine Berechnung ausführen", "{n} quiere ejecutar un cálculo en su espacio")
                .replace("{n}", name)
        } else {
            let said = pick(lang, "{n} veut lancer un programme sur ta machine, avec accès à tes fichiers",
                            "{n} wants to run a program on your machine, with access to your files",
                            "{n} möchte ein Programm auf deinem Gerät ausführen, mit Zugriff auf deine Dateien",
                            "{n} quiere ejecutar un programa en tu equipo, con acceso a tus archivos")
                .replace("{n}", name);
            let lines = command_lines(lang, &detail, SUMMARY_COMMAND_CHARS);
            if lines.is_empty() { said } else { format!("{said}{}\n{lines}", colon(lang).trim_end()) }
        };
        return Some(Described { summary, detail: Some(detail).filter(|d| !d.trim().is_empty()) });
    }
    if command.is_empty() || command.starts_with('<') || key.starts_with("cua:") || key == "ssh_config_write" {
        return None;
    }
    // a terminal command Hermes flagged
    let said = if deletes(description) {
        pick(lang, "{n} veut supprimer", "{n} wants to delete", "{n} möchte löschen", "{n} quiere borrar")
    } else {
        pick(lang, "{n} veut exécuter", "{n} wants to run", "{n} möchte ausführen", "{n} quiere ejecutar")
    }
    .replace("{n}", name);
    let summary = format!("{said}{}{}", colon(lang), command_lines(lang, command, SUMMARY_COMMAND_CHARS));
    let detail = if description.is_empty() { command.to_string() } else { format!("{command}\n\n({description})") };
    Some(Described { summary, detail: Some(detail) })
}

fn mcp_tool(name: &str, lang: &str, tool: &str, server: &str, preview: Option<&str>) -> Described {
    let detail = Some(match preview {
        Some(p) => format!("{server} · {tool}\n{p}"),
        None => format!("{server} · {tool}"),
    });
    // the sandbox's shared folders (sandbox.rs): only deleting asks
    if server == crate::sandbox::SERVER_NAME {
        let said = pick(lang, "{n} veut supprimer un fichier de ses dossiers partagés",
                        "{n} wants to delete a file from its shared folders",
                        "{n} möchte eine Datei aus seinen geteilten Ordnern löschen",
                        "{n} quiere borrar un archivo de sus carpetas compartidas")
            .replace("{n}", name);
        let summary = match (verb_of(tool), preview) {
            (Verb::Delete, Some(p)) => format!("{said}{}{}", colon(lang), quoted(lang, &cut(p, SUMMARY_PREVIEW_CHARS))),
            (Verb::Delete, None) => said,
            _ => pick(lang, "{n} veut modifier ses dossiers partagés", "{n} wants to change its shared folders",
                      "{n} möchte seine geteilten Ordner ändern", "{n} quiere cambiar sus carpetas compartidas")
                .replace("{n}", name),
        };
        return Described { summary, detail };
    }
    let place = server_name(server);
    let said = match verb_of(tool) {
        Verb::Create => pick(lang, "{n} veut créer dans {s}", "{n} wants to create something in {s}",
                             "{n} möchte in {s} etwas anlegen", "{n} quiere crear algo en {s}"),
        Verb::Change => pick(lang, "{n} veut modifier dans {s}", "{n} wants to change something in {s}",
                             "{n} möchte in {s} etwas ändern", "{n} quiere cambiar algo en {s}"),
        Verb::Delete => pick(lang, "{n} veut supprimer dans {s}", "{n} wants to delete something in {s}",
                             "{n} möchte in {s} etwas löschen", "{n} quiere borrar algo en {s}"),
        Verb::Send => pick(lang, "{n} veut envoyer avec {s}", "{n} wants to send something with {s}",
                           "{n} möchte mit {s} etwas senden", "{n} quiere enviar algo con {s}"),
        Verb::Use => pick(lang, "{n} veut utiliser {t} de {s}", "{n} wants to use {s}’s {t}",
                          "{n} möchte {t} von {s} verwenden", "{n} quiere usar {t} de {s}"),
    }
    .replace("{n}", name)
    .replace("{s}", &place)
    .replace("{t}", tool);
    let summary = match preview {
        Some(p) => format!("{said}{}{}", colon(lang), quoted(lang, &cut(p, SUMMARY_PREVIEW_CHARS))),
        None => said,
    };
    Described { summary, detail }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn data(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    fn summary(lang: &str, v: Value, last: Option<&LastTool>) -> Option<String> {
        describe("Wellness", lang, &data(v), last, true).map(|d| d.summary)
    }

    #[test]
    fn code_and_scripts_are_a_calculation() {
        let code = json!({"command": "execute_code <<'PY'\nimport sqlite3\nprint(1)\nPY", "pattern_key": "execute_code",
                          "description": "execute_code script execution. The script can spawn subprocesses…"});
        let d = describe("Wellness", "fr", &data(code), None, true).unwrap();
        assert_eq!(d.summary, "Wellness veut lancer un calcul dans son espace");
        assert_eq!(d.detail.as_deref(), Some("import sqlite3\nprint(1)"));
        let heredoc = json!({"command": "python3 - <<'EOF'\nprint(2)\nEOF", "pattern_key": "script execution via heredoc",
                             "description": "script execution via heredoc"});
        assert_eq!(summary("en", heredoc, None).as_deref(), Some("Wellness wants to run a calculation in its own space"));
        let flag = json!({"command": "python3 -c 'print(3)'", "description": "script execution via -e/-c flag"});
        assert_eq!(summary("de", flag, None).as_deref(), Some("Wellness möchte in seinem Bereich eine Berechnung ausführen"));
    }

    #[test]
    fn commands_say_what_they_run() {
        let rm = json!({"command": "rm -rf /workspace/old", "pattern_key": "recursive delete",
                        "description": "recursive delete"});
        let d = describe("Wellness", "fr", &data(rm), None, true).unwrap();
        assert_eq!(d.summary, "Wellness veut supprimer : rm -rf /workspace/old");
        assert_eq!(d.detail.as_deref(), Some("rm -rf /workspace/old\n\n(recursive delete)"));
        let shell = json!({"command": "bash -c 'systemctl --user restart x'", "description": "shell command via -c/-lc flag"});
        assert_eq!(summary("es", shell, None).as_deref(), Some("Wellness quiere ejecutar: bash -c 'systemctl --user restart x'"));
        let long = json!({"command": format!("curl https://example.org/{}\nsecond line", "a".repeat(100)),
                          "description": "pipe remote content to shell"});
        let s = summary("en", long, None).unwrap();
        let first = s.lines().next().unwrap();
        assert!(first.starts_with("Wellness wants to run: curl https://example.org/aaa") && first.ends_with('…'), "{s}");
        assert!(first.chars().count() < 110);
        assert!(s.ends_with("\nsecond line"), "{s}");
    }

    #[test]
    fn a_script_outside_a_sandbox_says_it_reaches_the_files() {
        let code = json!({"command": "execute_code <<'PY'\nimport os\nfor f in os.listdir('/home'):\n    print(f)\nos.remove('x')\nprint('ok')\nPY",
                          "pattern_key": "execute_code", "description": "execute_code script execution."});
        let d = describe("Wellness", "fr", &data(code), None, false).unwrap();
        assert!(!d.summary.contains("dans son espace"), "{}", d.summary);
        assert_eq!(d.summary, "Wellness veut lancer un programme sur ta machine, avec accès à tes fichiers :\n\
                               import os\nfor f in os.listdir('/home'):\nprint(f)\n(+2 lignes)");
        // the whole code stays in the detail
        assert_eq!(d.detail.as_deref(), Some("import os\nfor f in os.listdir('/home'):\n    print(f)\nos.remove('x')\nprint('ok')"));
        let flag = json!({"command": "python3 -c 'print(3)'", "description": "script execution via -e/-c flag"});
        assert_eq!(describe("Wellness", "en", &data(flag), None, false).map(|d| d.summary).as_deref(),
                   Some("Wellness wants to run a program on your machine, with access to your files:\npython3 -c 'print(3)'"));
    }

    #[test]
    fn a_long_command_shows_its_first_lines_spaces_folded() {
        let cmd = json!({"command": "cd   /srv\t&&  ls\n\nrm -rf    old\necho a\necho b", "description": "recursive delete"});
        assert_eq!(summary("fr", cmd, None).as_deref(),
                   Some("Wellness veut supprimer : cd /srv && ls\nrm -rf old\necho a\n(+1 ligne)"));
    }

    #[test]
    fn untrusted_mcp_tools_name_the_service_and_what_they_touch() {
        let gate = |tool: &str, server: &str| {
            json!({"command": format!("MCP tool '{tool}' on UNTRUSTED server '{server}' wants to run. This tool is write-capable \
                                       (no readOnlyHint=true annotation) and may modify external state."),
                   "description": format!("Server '{server}' is configured 'trust: untrusted'."),
                   "pattern_key": "mcp_elicitation"})
        };
        let last = LastTool { name: "mcp_notion_notion_create_pages".into(), preview: "Réunion du 12".into() };
        let d = describe("Vie", "fr", &data(gate("notion-create-pages", "notion")), Some(&last), true).unwrap();
        assert_eq!(d.summary, "Vie veut créer dans Notion : « Réunion du 12 »");
        assert_eq!(d.detail.as_deref(), Some("notion · notion-create-pages\nRéunion du 12"));
        // another tool started last: no preview borrowed
        let other = LastTool { name: "terminal".into(), preview: "ls".into() };
        assert_eq!(summary("en", gate("archive_page", "home_assistant"), Some(&other)).as_deref(),
                   Some("Wellness wants to delete something in Home Assistant"));
        // the person's calendar, contacts…, and the bridge's own tools: the apps' own words
        assert_eq!(summary("fr", gate("calendar_add", "bipagents"), None), None);
        assert_eq!(summary("fr", gate("contacts_search", "phone"), None), None);
        assert_eq!(summary("fr", gate("send_message", "slack"), None).as_deref(), Some("Wellness veut envoyer avec Slack"));
        assert_eq!(summary("fr", gate("lookup", "finary"), None).as_deref(), Some("Wellness veut utiliser lookup de Finary"));
        let shared = LastTool { name: "mcp_bipagents_files_shared_files_delete".into(), preview: "notes/2026.txt".into() };
        assert_eq!(summary("fr", gate("shared_files_delete", "bipagents_files"), Some(&shared)).as_deref(),
                   Some("Wellness veut supprimer un fichier de ses dossiers partagés : « notes/2026.txt »"));
    }

    #[test]
    fn questions_rules_and_what_is_left_to_the_apps() {
        let asked = json!({"command": "Allow the import of 3 files?", "description": "Server 'x' asks", "pattern_key": "mcp_elicitation"});
        assert_eq!(summary("fr", asked, None).as_deref(),
                   Some("Wellness demande ta confirmation : « Allow the import of 3 files? »"));
        let rule = json!({"command": "<reminders_add> (plugin approval rule)", "description": "adds a reminder"});
        assert_eq!(summary("en", rule, None).as_deref(), Some("Wellness wants to use the tool reminders_add"));
        let soul = json!({"command": "<write to SOUL.md>", "pattern_key": "protected_instruction_file", "description": "x"});
        assert_eq!(summary("fr", soul, None).as_deref(), Some("Wellness veut modifier ses propres consignes"));
        // the bridge's card payment, a computer-use action, nothing at all: the apps' own reading
        let pay = json!({"command": "Card payment pay_0123456789ab: 12.50 EUR on sncf-connect.com with card \"x\" ending 4242, for \"y\".",
                         "pattern_key": "mcp_elicitation"});
        assert_eq!(summary("fr", pay, None), None);
        assert_eq!(summary("fr", json!({"command": "<click>", "pattern_key": "cua:click", "description": "x"}), None), None);
        assert_eq!(summary("fr", json!({}), None), None);
    }
}
