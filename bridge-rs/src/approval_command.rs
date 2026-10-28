//! A terminal command in plain words, for the approval card (approval_text.rs; plain words pass of 2026-10-28):
//! « Wellness veut supprimer 3 fichiers de ses dossiers », « Wellness veut installer un outil : pypdf, pour lire les
//! PDF », « Wellness veut envoyer des données à example.com ». The command itself stays in `bip_detail`.
//!
//! The command is cut into its steps (`&&`, `||`, `;`, `|`, `&`, new lines; heredoc bodies skipped), each step read
//! with shell quoting (`sudo`, `env X=1`, `nohup`, `xargs` and redirections set aside, `bash -c '…'` read inside).
//! Each step gets an effect, ranked by risk; the card names the riskiest one, « et d'autres actions » when other
//! steps do something too. A step the bridge doesn't know is never given an effect: when nothing is recognised, the
//! caller says only that the agent wants to run a command.
//!
//! Not a Python module: Rust bridge only (owner group: push-runs).

use std::sync::LazyLock;

use regex::Regex;

use crate::approval_text::{colon, pick, quoted, Place};

/// What one step does.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Effect {
    /// Reading, listing, printing, moving around: nothing to say.
    Harmless,
    /// A step the bridge can't read: never given an effect.
    Unknown,
    Delete(Deleted),
    /// A program downloaded and run at once (`curl … | sh`): its site.
    RunDownloaded(Option<String>),
    /// `git reset --hard`, `git clean`, `git checkout -- .`…
    WipeChanges,
    Send(Option<String>),
    GitPush { force: bool },
    Stop(Option<String>),
    Service(ServiceVerb),
    Crontab { erase: bool },
    Permissions(Option<String>),
    Install(Installed),
    Uninstall(Vec<String>),
    Upgrade,
    Move(Vec<String>),
    Copy(Vec<String>),
    Program(Option<String>),
    Download(Option<String>),
    Clone(Option<String>),
    Pull,
    Commit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Deleted {
    /// Everything under `/`, a home or the current folder.
    All(String),
    /// Named targets: their names, whether recursive.
    Named(Vec<String>, bool),
    /// A pattern, `find -delete`, `xargs rm`: files the bridge can't count.
    Some,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Installed {
    Packages(Vec<String>),
    /// `pip install -r requirements.txt`, `npm install`: what a project lists.
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceVerb {
    Stop,
    Restart,
    Start,
}

impl Effect {
    fn rank(&self) -> u8 {
        match self {
            Effect::Harmless => 0,
            Effect::Unknown => 0,
            Effect::Delete(Deleted::All(_)) => 12,
            Effect::RunDownloaded(_) => 11,
            Effect::Delete(_) => 10,
            Effect::WipeChanges => 9,
            Effect::Send(_) => 9,
            Effect::GitPush { .. } => 8,
            Effect::Crontab { erase: true } => 8,
            Effect::Stop(_) | Effect::Service(_) | Effect::Crontab { .. } => 7,
            Effect::Permissions(_) => 6,
            Effect::Install(_) | Effect::Uninstall(_) | Effect::Upgrade => 5,
            Effect::Move(_) => 4,
            Effect::Program(_) => 4,
            Effect::Download(_) | Effect::Clone(_) => 3,
            Effect::Copy(_) | Effect::Pull | Effect::Commit => 2,
        }
    }
}

/// One step of the command, and whether its output goes into the next one (`|`).
struct Step {
    text: String,
    piped: bool,
}

static HEREDOC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<<-?\s*(['"]?)([A-Za-z_][A-Za-z0-9_]*)(['"]?)"#).expect("valid regex"));
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=").expect("valid regex"));
static URL_HOST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[a-z][a-z0-9+.-]*://(?:[^/@?#]*@)?([^/:?#\[\]]+)").expect("valid regex"));
/// `user@host:path` (scp, rsync, git).
static REMOTE_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[A-Za-z0-9._-]+@)?([A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+):").expect("valid regex"));
static VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\[<>=!~;@ ].*$").expect("valid regex"));

/// The command without its heredoc bodies and line continuations.
fn without_heredocs(command: &str) -> String {
    let joined = command.replace("\\\r\n", " ").replace("\\\n", " ");
    let mut out = Vec::new();
    let mut lines = joined.lines();
    while let Some(line) = lines.next() {
        out.push(line);
        if let Some(caps) = HEREDOC.captures(line) {
            let word = caps[2].to_string();
            for body in lines.by_ref() {
                if body.trim() == word {
                    break;
                }
            }
        }
    }
    out.join("\n")
}

/// The steps, cut on `&&`, `||`, `;`, `|`, `&` and new lines outside quotes.
fn steps(command: &str) -> Vec<Step> {
    let chars: Vec<char> = command.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    let (mut single, mut double) = (false, false);
    let mut i = 0;
    let flush = |current: &mut String, piped: bool, out: &mut Vec<Step>| {
        let text = current.trim().to_string();
        if !text.is_empty() {
            out.push(Step { text, piped });
        }
        current.clear();
    };
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let prev = if i > 0 { Some(chars[i - 1]) } else { None };
        if single {
            single = c != '\'';
            current.push(c);
        } else if c == '\\' {
            current.push(c);
            if let Some(n) = next {
                current.push(n);
                i += 1;
            }
        } else if double {
            double = c != '"';
            current.push(c);
        } else if c == '\'' {
            single = true;
            current.push(c);
        } else if c == '"' {
            double = true;
            current.push(c);
        } else if (c == '&' && next == Some('&')) || (c == '|' && next == Some('|')) {
            flush(&mut current, false, &mut out);
            i += 1;
        } else if c == '|' {
            flush(&mut current, true, &mut out);
            if next == Some('&') {
                i += 1;
            }
        } else if c == ';' || c == '\n' || (c == '&' && next != Some('>') && !matches!(prev, Some('>') | Some('<'))) {
            flush(&mut current, false, &mut out);
        } else {
            current.push(c);
        }
        i += 1;
    }
    flush(&mut current, false, &mut out);
    out
}

/// The words of a step, its redirections set aside; None: quoting the bridge can't read.
fn words(step: &str) -> Option<Vec<String>> {
    let raw = shlex::split(step)?;
    let mut out = Vec::new();
    let mut skip_next = false;
    for w in raw {
        if skip_next {
            skip_next = false;
            continue;
        }
        let redirect = w.trim_start_matches(|c: char| c.is_ascii_digit() || c == '&');
        if [">", ">>", "<", "<<", "<<<", ">|"].contains(&redirect) {
            skip_next = true;
            continue;
        }
        if redirect.starts_with('>') || redirect.starts_with('<') {
            continue;
        }
        out.push(w);
    }
    Some(out)
}

fn base(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// A target's name as the card shows it: its last part, the whole when nothing is left.
fn shown_name(target: &str) -> String {
    let trimmed = target.trim_end_matches('/');
    let last = base(trimmed);
    if last.is_empty() {
        target.to_string()
    } else {
        last.to_string()
    }
}

fn host_of(word: &str) -> Option<String> {
    let host = URL_HOST.captures(word).map(|c| c[1].to_lowercase())?;
    Some(host.strip_prefix("www.").unwrap_or(&host).to_string())
}

fn is_flag(word: &str) -> bool {
    word.starts_with('-') && word.len() > 1
}

/// The words after the command's own options: `sudo`, `env X=1`, `nohup`, `time`, `exec`, `command`, assignments.
fn unwrap_prefixes(mut words: Vec<String>, sudo: &mut bool) -> Vec<String> {
    loop {
        let Some(first) = words.first().cloned() else { return words };
        if ASSIGNMENT.is_match(&first) {
            words.remove(0);
            continue;
        }
        match base(&first) {
            "sudo" | "doas" => {
                *sudo = true;
                words.remove(0);
                while let Some(w) = words.first().cloned() {
                    if !is_flag(&w) {
                        break;
                    }
                    words.remove(0);
                    if ["-u", "-g", "-U", "-C", "-h", "-p"].contains(&w.as_str()) && !words.is_empty() {
                        words.remove(0);
                    }
                }
            }
            "env" | "nohup" | "time" | "exec" | "command" | "nice" | "stdbuf" | "timeout" => {
                let timed = base(&first) == "timeout";
                words.remove(0);
                while words.first().is_some_and(|w| is_flag(w) || ASSIGNMENT.is_match(w)) {
                    words.remove(0);
                }
                if timed && !words.is_empty() {
                    words.remove(0); // the duration
                }
            }
            _ => return words,
        }
    }
}

const HARMLESS: &[&str] = &[
    "cd", "ls", "pwd", "echo", "printf", "cat", "head", "tail", "grep", "egrep", "rg", "wc", "sort", "uniq", "mkdir",
    "touch", "true", "false", "sleep", "which", "type", "export", "set", "unset", "source", ".", "test", "[", "date",
    "whoami", "id", "uname", "stat", "file", "du", "df", "less", "more", "jq", "awk", "cut", "tr", "basename",
    "dirname", "realpath", "readlink", "ps", "free", "uptime", "hostname", "diff", "md5sum", "sha256sum", "tree",
];

const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "fish"];
const INTERPRETERS: &[&str] = &["python", "python3", "node", "deno", "bun", "ruby", "perl", "php", "Rscript", "osascript"];

fn is_interpreter(word: &str) -> bool {
    let b = base(word);
    INTERPRETERS.contains(&b) || (b.starts_with("python3.") && b[8..].chars().all(|c| c.is_ascii_digit()))
}

/// The effects of one step (several for `bash -c '…'`).
fn read_step(text: &str, sudo: &mut bool, depth: u8) -> Vec<Effect> {
    let Some(words) = words(text) else { return vec![Effect::Unknown] };
    let words = unwrap_prefixes(words, sudo);
    let Some(first) = words.first() else { return vec![Effect::Harmless] };
    let cmd = base(first).to_string();
    let args: Vec<String> = words[1..].to_vec();
    // `bash -c '…'`: what is inside
    if SHELLS.contains(&cmd.as_str()) {
        if let Some(pos) = args.iter().position(|a| a == "-c" || a == "-lc" || a == "-ic") {
            if let Some(inner) = args.get(pos + 1).filter(|_| depth < 3) {
                return read(inner, sudo, depth + 1);
            }
            return vec![Effect::Unknown];
        }
        return vec![program(&args)];
    }
    if cmd == "xargs" {
        let rest: Vec<String> = args.iter().skip_while(|a| is_flag(a)).cloned().collect();
        if rest.is_empty() {
            return vec![Effect::Unknown];
        }
        let text = shlex::try_join(rest.iter().map(String::as_str)).unwrap_or_default();
        let effects = read_step(&text, sudo, depth + 1);
        // `xargs rm`: its targets come from before
        return effects
            .into_iter()
            .map(|e| match e {
                Effect::Unknown if base(&rest[0]) == "rm" => Effect::Delete(Deleted::Some),
                other => other,
            })
            .collect();
    }
    vec![effect_of(&cmd, &args)]
}

/// A script or program: its file's name when it has one.
fn program(args: &[String]) -> Effect {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == "-c" || a == "-e" || a == "-" {
            return Effect::Program(None);
        }
        if a == "-m" {
            return match iter.next().map(String::as_str) {
                Some("pip") => effect_of("pip", &iter.cloned().collect::<Vec<_>>()),
                Some(module) => Effect::Program(Some(module.to_string())),
                None => Effect::Unknown,
            };
        }
        if !is_flag(a) {
            return Effect::Program(Some(shown_name(a)));
        }
    }
    Effect::Program(None)
}

fn targets(args: &[String]) -> (Vec<String>, String) {
    let mut out = Vec::new();
    let mut flags = String::new();
    let mut after = false;
    for a in args {
        if after || !is_flag(a) {
            out.push(a.clone());
        } else if a == "--" {
            after = true;
        } else {
            flags.push_str(a);
            flags.push(' ');
        }
    }
    (out, flags)
}

fn dynamic(word: &str) -> bool {
    word.contains(['*', '?', '$', '`', '{'])
}

fn delete(targets: Vec<String>, recursive: bool) -> Effect {
    if targets.is_empty() {
        return Effect::Unknown;
    }
    for t in &targets {
        let t = t.trim_end_matches('/');
        if ["", "/*", "~", "$HOME", "${HOME}", "~/*", "$HOME/*", ".", "./*", "*", "..", "/home", "/Users"].contains(&t) {
            return Effect::Delete(Deleted::All(if t.is_empty() { "/".into() } else { t.to_string() }));
        }
    }
    if targets.iter().any(|t| dynamic(t)) {
        return Effect::Delete(Deleted::Some);
    }
    Effect::Delete(Deleted::Named(targets.iter().map(|t| shown_name(t)).collect(), recursive))
}

/// Package names without their versions and extras.
fn packages(args: &[String], value_flags: &[&str]) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    let mut project = false;
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if ["-r", "--requirement", "-e", "--editable"].contains(&a.as_str()) {
            project = true;
            iter.next();
        } else if a.starts_with("--requirement=") || (a.starts_with("-r") && a.len() > 2 && !a.starts_with("--")) {
            project = true;
        } else if value_flags.contains(&a.as_str()) {
            iter.next();
        } else if is_flag(a) {
        } else if a == "." || a.starts_with("./") || a.starts_with('/') || a.ends_with(".txt") {
            project = true;
        } else {
            let name = VERSION.replace(a, "").to_string();
            let name = if name.contains("://") || name.starts_with("git+") {
                shown_name(name.trim_end_matches(".git"))
            } else {
                base(&name).to_string()
            };
            if !name.is_empty() {
                out.push(name);
            }
        }
    }
    (out, project)
}

fn installs(args: &[String], value_flags: &[&str]) -> Effect {
    let (names, project) = packages(args, value_flags);
    if !names.is_empty() {
        Effect::Install(Installed::Packages(names))
    } else if project || args.iter().all(|a| is_flag(a)) {
        Effect::Install(Installed::Project)
    } else {
        Effect::Unknown
    }
}

fn uninstalls(args: &[String]) -> Effect {
    let (names, _) = packages(args, &[]);
    if names.is_empty() {
        Effect::Unknown
    } else {
        Effect::Uninstall(names)
    }
}

/// curl or wget: sending or downloading, and from where.
fn transfer(cmd: &str, args: &[String]) -> Effect {
    let host = args.iter().find_map(|a| host_of(a.trim_start_matches("--url=")));
    let sends = args.iter().enumerate().any(|(i, a)| {
        let a = a.as_str();
        let next = args.get(i + 1).map(|s| s.to_uppercase()).unwrap_or_default();
        if cmd == "curl" {
            ["-d", "-F", "-T", "--form", "--json", "--upload-file"].contains(&a)
                || a.starts_with("--data")
                || a.starts_with("--form")
                || a.starts_with("--json")
                || ((a.starts_with("-d") || a.starts_with("-F") || a.starts_with("-T")) && !a.starts_with("--"))
                || ((a == "-X" || a == "--request") && ["POST", "PUT", "PATCH", "DELETE"].contains(&next.as_str()))
                || ["-XPOST", "-XPUT", "-XPATCH", "-XDELETE"].contains(&a.to_uppercase().as_str())
                || (a.starts_with("--request=") && !a.to_uppercase().ends_with("=GET"))
        } else {
            a.starts_with("--post-")
                || a.starts_with("--body-")
                || (a.starts_with("--method=") && !a.to_uppercase().ends_with("=GET"))
        }
    });
    if sends {
        Effect::Send(host)
    } else {
        Effect::Download(host)
    }
}

fn git(args: &[String]) -> Effect {
    let mut iter = args.iter().peekable();
    while let Some(a) = iter.peek() {
        if a.as_str() == "-C" || a.as_str() == "-c" {
            iter.next();
            iter.next();
        } else if is_flag(a) {
            iter.next();
        } else {
            break;
        }
    }
    let Some(sub) = iter.next() else { return Effect::Unknown };
    let rest: Vec<String> = iter.cloned().collect();
    let has = |f: &[&str]| rest.iter().any(|a| f.contains(&a.as_str()));
    match sub.as_str() {
        "clone" => Effect::Clone(rest.iter().find_map(|a| host_of(a).or_else(|| remote_host(a)))),
        "pull" | "fetch" => Effect::Pull,
        "push" => Effect::GitPush {
            force: has(&["-f", "--force", "--mirror", "--delete", "-d"]) || rest.iter().any(|a| a.starts_with("--force")),
        },
        "commit" => Effect::Commit,
        "reset" if has(&["--hard", "--merge", "--keep"]) => Effect::WipeChanges,
        "clean" if rest.iter().any(|a| a.starts_with('-') && a.contains('f')) => Effect::WipeChanges,
        "checkout" if has(&["--", ".", "-f", "--force"]) => Effect::WipeChanges,
        "restore" => Effect::WipeChanges,
        "stash" if has(&["drop", "clear"]) => Effect::WipeChanges,
        "rm" => {
            let (t, flags) = targets(&rest);
            delete(t, flags.contains('r'))
        }
        "status" | "log" | "diff" | "show" | "add" | "branch" | "remote" | "config" | "init" | "switch" | "rev-parse" => {
            Effect::Harmless
        }
        _ => Effect::Unknown,
    }
}

fn remote_host(word: &str) -> Option<String> {
    if word.contains("://") {
        return None;
    }
    REMOTE_PATH.captures(word).map(|c| c[1].to_lowercase())
}

fn effect_of(cmd: &str, args: &[String]) -> Effect {
    let sub = args.iter().find(|a| !is_flag(a)).map(String::as_str).unwrap_or("");
    let after_sub = || -> Vec<String> {
        let pos = args.iter().position(|a| !is_flag(a)).map(|p| p + 1).unwrap_or(args.len());
        args[pos..].to_vec()
    };
    if HARMLESS.contains(&cmd) {
        return Effect::Harmless;
    }
    if is_interpreter(cmd) {
        return program(args);
    }
    if cmd.ends_with(".sh") || cmd.ends_with(".py") || cmd.ends_with(".js") {
        return Effect::Program(Some(cmd.to_string()));
    }
    match cmd {
        "rm" | "shred" | "unlink" | "trash" | "trash-put" | "srm" => {
            let (t, flags) = targets(args);
            delete(t, flags.contains('r') || flags.contains('R') || flags.contains("--recursive"))
        }
        "rmdir" => {
            let (t, _) = targets(args);
            delete(t, true)
        }
        "find" if args.iter().any(|a| a == "-delete")
            || args.windows(2).any(|w| w[0].starts_with("-exec") && ["rm", "shred", "unlink"].contains(&base(&w[1]))) =>
        {
            Effect::Delete(Deleted::Some)
        }
        "find" => Effect::Harmless,
        "sed" if !args.iter().any(|a| a.starts_with("-i") || a == "--in-place") => Effect::Harmless,
        "pip" | "pip3" | "pipx" | "conda" | "mamba" | "poetry" | "uv" => {
            // `uv pip install x`, `uv add x`, `poetry add x`
            let (sub, rest) = if (cmd == "uv" && sub == "pip") || sub == "pip" {
                let rest = after_sub();
                let s = rest.iter().find(|a| !is_flag(a)).cloned().unwrap_or_default();
                let pos = rest.iter().position(|a| !is_flag(a)).map(|p| p + 1).unwrap_or(rest.len());
                (s, rest[pos..].to_vec())
            } else {
                (sub.to_string(), after_sub())
            };
            let value_flags = ["-i", "--index-url", "--extra-index-url", "-t", "--target", "-c", "--constraint",
                               "--python", "-p", "-n", "--name", "-f", "--find-links"];
            match sub.as_str() {
                "install" | "add" | "sync" | "inject" => {
                    if rest.iter().any(|a| a == "-U" || a == "--upgrade") && sub == "install" {
                        installs(&rest, &value_flags)
                    } else if sub == "sync" {
                        Effect::Install(Installed::Project)
                    } else {
                        installs(&rest, &value_flags)
                    }
                }
                "uninstall" | "remove" => uninstalls(&rest),
                "list" | "show" | "freeze" | "search" | "--version" => Effect::Harmless,
                "run" if cmd == "uv" || cmd == "poetry" => program(&rest),
                _ => Effect::Unknown,
            }
        }
        "npm" | "pnpm" | "yarn" | "bun" => match sub {
            "install" | "i" | "add" | "ci" => installs(&after_sub(), &["--registry", "--prefix"]),
            "" if cmd == "yarn" => Effect::Install(Installed::Project),
            "uninstall" | "remove" | "rm" | "un" => uninstalls(&after_sub()),
            "update" | "upgrade" | "up" => Effect::Upgrade,
            "run" | "start" | "test" | "exec" | "x" | "dlx" => Effect::Program(None),
            "list" | "ls" | "view" | "info" | "outdated" => Effect::Harmless,
            _ => Effect::Unknown,
        },
        "npx" | "bunx" | "pnpx" => program(args),
        "apt" | "apt-get" | "dnf" | "yum" | "apk" | "zypper" | "brew" | "port" | "snap" | "gem" | "cargo" | "go"
        | "flatpak" => match sub {
            "install" | "add" | "reinstall" => {
                let rest = after_sub();
                let rest: Vec<String> = if cmd == "go" {
                    rest.iter().map(|a| VERSION.replace(a, "").to_string()).collect()
                } else {
                    rest
                };
                installs(&rest, &["--cask", "-t", "--git", "--path", "--version"])
            }
            "remove" | "purge" | "uninstall" | "del" | "erase" | "autoremove" => uninstalls(&after_sub()),
            "update" | "upgrade" | "dist-upgrade" | "full-upgrade" | "refresh" => Effect::Upgrade,
            "list" | "search" | "show" | "info" | "policy" => Effect::Harmless,
            "build" | "run" | "test" | "check" if cmd == "cargo" || cmd == "go" => Effect::Program(None),
            _ => Effect::Unknown,
        },
        "pacman" => {
            let flags: String = args.iter().filter(|a| is_flag(a)).cloned().collect();
            let rest: Vec<String> = args.iter().filter(|a| !is_flag(a)).cloned().collect();
            if flags.contains('R') {
                uninstalls(&rest)
            } else if flags.contains('S') && !rest.is_empty() {
                installs(&rest, &[])
            } else if flags.contains("Syu") {
                Effect::Upgrade
            } else {
                Effect::Unknown
            }
        }
        "curl" | "wget" => transfer(cmd, args),
        "scp" | "rsync" | "sftp" => {
            let plain: Vec<&String> = args.iter().filter(|a| !is_flag(a)).collect();
            match plain.split_last() {
                Some((last, _)) if remote_host(last).is_some() => Effect::Send(remote_host(last)),
                Some((_, before)) => match before.iter().find_map(|a| remote_host(a)) {
                    Some(host) => Effect::Download(Some(host)),
                    None => Effect::Copy(plain.iter().take(plain.len().saturating_sub(1)).map(|a| shown_name(a)).collect()),
                },
                None => Effect::Unknown,
            }
        }
        "git" => git(args),
        "mv" | "cp" => {
            let (t, _) = targets(args);
            if t.len() < 2 {
                return Effect::Unknown;
            }
            let moved: Vec<String> = t[..t.len() - 1].iter().map(|s| shown_name(s)).collect();
            if moved.iter().any(|m| dynamic(m)) {
                return if cmd == "mv" { Effect::Move(Vec::new()) } else { Effect::Copy(Vec::new()) };
            }
            if cmd == "mv" {
                Effect::Move(moved)
            } else {
                Effect::Copy(moved)
            }
        }
        "chmod" | "chown" | "chgrp" | "chattr" | "setfacl" => {
            let (t, _) = targets(args);
            // the mode or owner first
            let files: Vec<&String> = t.iter().skip(1).collect();
            match files.as_slice() {
                [one] if !dynamic(one) => Effect::Permissions(Some(shown_name(one))),
                _ => Effect::Permissions(None),
            }
        }
        "kill" | "pkill" | "killall" => {
            let name = if cmd == "kill" { None } else { args.iter().rfind(|a| !is_flag(a)).cloned() };
            Effect::Stop(name)
        }
        "systemctl" | "service" | "launchctl" | "rc-service" => {
            let verbs: Vec<&str> = args.iter().map(String::as_str).collect();
            if verbs.iter().any(|v| ["stop", "kill", "disable", "mask", "unload", "bootout", "remove"].contains(v)) {
                Effect::Service(ServiceVerb::Stop)
            } else if verbs.iter().any(|v| ["restart", "reload", "try-restart", "reload-or-restart", "kickstart"].contains(v)) {
                Effect::Service(ServiceVerb::Restart)
            } else if verbs.iter().any(|v| ["start", "enable", "load", "bootstrap", "unmask"].contains(v)) {
                Effect::Service(ServiceVerb::Start)
            } else if verbs.iter().any(|v| ["status", "list-units", "is-active", "list", "print", "show"].contains(v)) {
                Effect::Harmless
            } else {
                Effect::Unknown
            }
        }
        "crontab" => {
            if args.iter().any(|a| a == "-r") {
                Effect::Crontab { erase: true }
            } else if args.iter().all(|a| a == "-l" || a == "-u") && !args.is_empty() {
                Effect::Harmless
            } else {
                Effect::Crontab { erase: false }
            }
        }
        _ => Effect::Unknown,
    }
}

/// The effects of a whole command.
fn read(command: &str, sudo: &mut bool, depth: u8) -> Vec<Effect> {
    let steps = steps(&without_heredocs(command));
    let mut out = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let next = steps.get(i + 1).filter(|_| step.piped).map(|s| s.text.as_str());
        let mut effects = read_step(&step.text, sudo, depth);
        // `curl … | sh`: a program downloaded and run at once
        if let (Some(next), [Effect::Download(host)]) = (next, effects.as_slice()) {
            let mut ignored = false;
            let receiver = words(next).map(|w| unwrap_prefixes(w, &mut ignored)).unwrap_or_default();
            if receiver.first().is_some_and(|w| SHELLS.contains(&base(w)) || is_interpreter(w)) {
                effects = vec![Effect::RunDownloaded(host.clone())];
            }
        }
        out.extend(effects);
    }
    // the shell that runs a downloaded program is part of it
    let mut kept = Vec::new();
    let mut after_download = false;
    for e in out {
        if after_download && matches!(e, Effect::Program(None)) {
            after_download = false;
            continue;
        }
        after_download = matches!(e, Effect::RunDownloaded(_));
        kept.push(e);
    }
    kept
}

/// Well-known tools and what they are for (a small list, kept safe: only what each one plainly does).
fn use_of(lang: &str, package: &str) -> Option<&'static str> {
    let p = package.to_lowercase().replace('_', "-");
    let (fr, en, de, es) = match p.as_str() {
        "pypdf" | "pypdf2" | "pdfplumber" | "pymupdf" | "fitz" | "pdfminer.six" | "pdfminer" | "poppler-utils"
        | "poppler" | "pikepdf" => ("pour lire les PDF", "to read PDFs", "um PDFs zu lesen", "para leer PDF"),
        "openpyxl" | "xlsxwriter" | "xlrd" => ("pour les fichiers Excel", "for Excel files", "für Excel-Dateien",
                                               "para archivos de Excel"),
        "python-docx" | "docx" => ("pour les documents Word", "for Word documents", "für Word-Dokumente",
                                   "para documentos de Word"),
        "pandas" | "polars" => ("pour les tableaux de données", "for data tables", "für Datentabellen",
                                "para tablas de datos"),
        "matplotlib" | "plotly" | "seaborn" => ("pour faire des graphiques", "to draw charts", "für Diagramme",
                                                "para hacer gráficos"),
        "requests" | "httpx" | "aiohttp" => ("pour aller chercher des pages sur Internet", "to fetch pages from the web",
                                             "um Seiten aus dem Internet zu holen", "para obtener páginas de Internet"),
        "beautifulsoup4" | "bs4" | "lxml" => ("pour lire les pages web", "to read web pages", "um Webseiten zu lesen",
                                              "para leer páginas web"),
        "pillow" | "imagemagick" => ("pour travailler sur les images", "to work on images", "für Bilder",
                                     "para trabajar con imágenes"),
        "ffmpeg" => ("pour les sons et les vidéos", "for sound and video", "für Ton und Video", "para audio y vídeo"),
        "yt-dlp" => ("pour télécharger des vidéos", "to download videos", "um Videos herunterzuladen",
                     "para descargar vídeos"),
        "tesseract" | "tesseract-ocr" | "pytesseract" => ("pour lire le texte des images", "to read text in images",
                                                          "um Text in Bildern zu lesen", "para leer texto en imágenes"),
        _ => return None,
    };
    Some(pick(lang, fr, en, de, es))
}

/// « a, b, c » (three at most, then « … »).
fn listed(names: &[String]) -> String {
    let shown: Vec<&str> = names.iter().take(3).map(String::as_str).collect();
    let mut text = shown.join(", ");
    if names.len() > 3 {
        text.push_str(", …");
    }
    text
}

/// Where a program runs, as two parts: one before the verb (German), one at the end.
fn place_parts(lang: &str, place: Place) -> (&'static str, &'static str) {
    match (lang, place) {
        ("fr", Place::Own) => ("", " dans son espace"),
        ("fr", Place::Machine) => ("", " sur ta machine, avec accès à tes fichiers"),
        ("fr", Place::Hosted) => ("", ", avec accès à tes fichiers"),
        ("de", Place::Own) => (" in seinem Bereich", ""),
        ("de", Place::Machine) => (" auf deinem Gerät", ", mit Zugriff auf deine Dateien"),
        ("de", Place::Hosted) => ("", ", mit Zugriff auf deine Dateien"),
        ("es", Place::Own) => ("", " en su espacio"),
        ("es", Place::Machine) => ("", " en tu equipo, con acceso a tus archivos"),
        ("es", Place::Hosted) => ("", ", con acceso a tus archivos"),
        (_, Place::Own) => ("", " in its own space"),
        (_, Place::Machine) => ("", " on your machine, with access to your files"),
        (_, Place::Hosted) => ("", ", with access to your files"),
    }
}

/// `{n}` the agent, `{x}` a quoted name, `{k}` a count, `{h}` a site, `{w}`/`{a}` where it runs.
fn fill(template: &str, name: &str, lang: &str, place: Place) -> String {
    let (w, a) = place_parts(lang, place);
    template.replace("{n}", name).replace("{w}", w).replace("{a}", a)
}

fn sentence(name: &str, lang: &str, place: Place, effect: &Effect) -> String {
    let q = |s: &str| quoted(lang, s);
    let t = |fr: &str, en: &str, de: &str, es: &str| fill(pick(lang, fr, en, de, es), name, lang, place);
    let k = |list: &[String]| list.len().to_string();
    match effect {
        Effect::Harmless | Effect::Unknown => String::new(),
        Effect::Delete(Deleted::All(target)) => match target.as_str() {
            "/" | "/*" => t("{n} veut supprimer tous les fichiers", "{n} wants to delete all the files",
                            "{n} möchte alle Dateien löschen", "{n} quiere borrar todos los archivos"),
            "." | "./*" | "*" => t("{n} veut supprimer tout le dossier où il travaille",
                                   "{n} wants to delete everything in the folder it works in",
                                   "{n} möchte alles im Ordner löschen, in dem es arbeitet",
                                   "{n} quiere borrar todo la carpeta donde trabaja"),
            _ => t("{n} veut supprimer tous les fichiers du dossier personnel",
                   "{n} wants to delete all the files of the home folder",
                   "{n} möchte alle Dateien des persönlichen Ordners löschen",
                   "{n} quiere borrar todos los archivos de la carpeta personal"),
        },
        Effect::Delete(Deleted::Named(names, recursive)) => {
            let folder = *recursive && !names[0].contains('.');
            if names.len() == 1 && folder {
                t("{n} veut supprimer le dossier {x} et tout son contenu",
                  "{n} wants to delete the folder {x} and everything in it",
                  "{n} möchte den Ordner {x} mit seinem ganzen Inhalt löschen",
                  "{n} quiere borrar la carpeta {x} y todo su contenido")
                    .replace("{x}", &q(&names[0]))
            } else if names.len() == 1 {
                t("{n} veut supprimer le fichier {x}", "{n} wants to delete the file {x}",
                  "{n} möchte die Datei {x} löschen", "{n} quiere borrar el archivo {x}")
                    .replace("{x}", &q(&names[0]))
            } else if *recursive {
                t("{n} veut supprimer {k} dossiers ou fichiers, avec leur contenu",
                  "{n} wants to delete {k} folders or files, with what they contain",
                  "{n} möchte {k} Ordner oder Dateien mit ihrem Inhalt löschen",
                  "{n} quiere borrar {k} carpetas o archivos, con su contenido")
                    .replace("{k}", &k(names))
            } else {
                deleted_files(name, lang, place, Some(names.len()))
            }
        }
        Effect::Delete(Deleted::Some) => deleted_files(name, lang, place, None),
        Effect::RunDownloaded(host) => match host.clone() {
            Some(h) => t("{n} veut télécharger un programme depuis {h} et le lancer tout de suite",
                         "{n} wants to download a program from {h} and run it at once",
                         "{n} möchte ein Programm von {h} herunterladen und sofort ausführen",
                         "{n} quiere descargar un programa de {h} y ejecutarlo enseguida")
                .replace("{h}", &h),
            None => t("{n} veut télécharger un programme et le lancer tout de suite",
                      "{n} wants to download a program and run it at once",
                      "{n} möchte ein Programm herunterladen und sofort ausführen",
                      "{n} quiere descargar un programa y ejecutarlo enseguida"),
        },
        Effect::WipeChanges => t("{n} veut effacer les modifications en cours d’un projet",
                                 "{n} wants to throw away a project’s unsaved changes",
                                 "{n} möchte die laufenden Änderungen eines Projekts verwerfen",
                                 "{n} quiere descartar los cambios en curso de un proyecto"),
        Effect::Send(host) => match host.clone() {
            Some(h) => t("{n} veut envoyer des données à {h}", "{n} wants to send data to {h}",
                         "{n} möchte Daten an {h} senden", "{n} quiere enviar datos a {h}")
                .replace("{h}", &h),
            None => t("{n} veut envoyer des données sur Internet", "{n} wants to send data over the Internet",
                      "{n} möchte Daten ins Internet senden", "{n} quiere enviar datos por Internet"),
        },
        Effect::GitPush { force: false } => t("{n} veut envoyer les modifications d’un projet en ligne",
                                              "{n} wants to send a project’s changes online",
                                              "{n} möchte die Änderungen eines Projekts online senden",
                                              "{n} quiere enviar los cambios de un proyecto en línea"),
        Effect::GitPush { force: true } => t("{n} veut envoyer les modifications d’un projet en ligne, en remplaçant ce qui y est",
                                             "{n} wants to send a project’s changes online, replacing what is there",
                                             "{n} möchte die Änderungen eines Projekts online senden und das Vorhandene ersetzen",
                                             "{n} quiere enviar los cambios de un proyecto en línea, reemplazando lo que hay"),
        Effect::Stop(Some(program)) => t("{n} veut arrêter le programme {x}", "{n} wants to stop the program {x}",
                                         "{n} möchte das Programm {x} beenden", "{n} quiere detener el programa {x}")
            .replace("{x}", &q(program)),
        Effect::Stop(None) => t("{n} veut arrêter un programme en cours", "{n} wants to stop a running program",
                                "{n} möchte ein laufendes Programm beenden", "{n} quiere detener un programa en marcha"),
        Effect::Service(ServiceVerb::Stop) => t("{n} veut arrêter un programme qui tourne en fond",
                                                "{n} wants to stop a program running in the background",
                                                "{n} möchte ein Programm beenden, das im Hintergrund läuft",
                                                "{n} quiere detener un programa que funciona en segundo plano"),
        Effect::Service(ServiceVerb::Restart) => t("{n} veut redémarrer un programme qui tourne en fond",
                                                   "{n} wants to restart a program running in the background",
                                                   "{n} möchte ein Programm neu starten, das im Hintergrund läuft",
                                                   "{n} quiere reiniciar un programa que funciona en segundo plano"),
        Effect::Service(ServiceVerb::Start) => t("{n} veut démarrer un programme en fond",
                                                 "{n} wants to start a program in the background",
                                                 "{n} möchte ein Programm im Hintergrund starten",
                                                 "{n} quiere iniciar un programa en segundo plano"),
        Effect::Crontab { erase: true } => t("{n} veut effacer toutes ses tâches programmées",
                                             "{n} wants to erase all its scheduled tasks",
                                             "{n} möchte alle seine geplanten Aufgaben löschen",
                                             "{n} quiere borrar todas sus tareas programadas"),
        Effect::Crontab { erase: false } => t("{n} veut changer ses tâches programmées",
                                              "{n} wants to change its scheduled tasks",
                                              "{n} möchte seine geplanten Aufgaben ändern",
                                              "{n} quiere cambiar sus tareas programadas"),
        Effect::Permissions(Some(file)) => t("{n} veut changer qui peut lire ou modifier {x}",
                                             "{n} wants to change who can read or change {x}",
                                             "{n} möchte ändern, wer {x} lesen oder ändern darf",
                                             "{n} quiere cambiar quién puede leer o modificar {x}")
            .replace("{x}", &q(file)),
        Effect::Permissions(None) => t("{n} veut changer qui peut lire ou modifier des fichiers",
                                       "{n} wants to change who can read or change some files",
                                       "{n} möchte ändern, wer Dateien lesen oder ändern darf",
                                       "{n} quiere cambiar quién puede leer o modificar unos archivos"),
        Effect::Install(Installed::Packages(names)) if names.len() == 1 => {
            let said = t("{n} veut installer un outil", "{n} wants to install a tool", "{n} möchte ein Werkzeug installieren",
                         "{n} quiere instalar una herramienta");
            match use_of(lang, &names[0]) {
                Some(why) => format!("{said}{}{}, {why}", colon(lang), names[0]),
                None => format!("{said}{}{}", colon(lang), names[0]),
            }
        }
        Effect::Install(Installed::Packages(names)) => {
            let said = t("{n} veut installer {k} outils", "{n} wants to install {k} tools", "{n} möchte {k} Werkzeuge installieren",
                         "{n} quiere instalar {k} herramientas")
                .replace("{k}", &k(names));
            format!("{said}{}{}", colon(lang), listed(names))
        }
        Effect::Install(Installed::Project) => t("{n} veut installer les outils dont un projet a besoin",
                                                 "{n} wants to install the tools a project needs",
                                                 "{n} möchte die Werkzeuge installieren, die ein Projekt braucht",
                                                 "{n} quiere instalar las herramientas que necesita un proyecto"),
        Effect::Uninstall(names) => {
            let said = if names.len() == 1 {
                t("{n} veut désinstaller un outil", "{n} wants to uninstall a tool", "{n} möchte ein Werkzeug deinstallieren",
                  "{n} quiere desinstalar una herramienta")
            } else {
                t("{n} veut désinstaller {k} outils", "{n} wants to uninstall {k} tools", "{n} möchte {k} Werkzeuge deinstallieren",
                  "{n} quiere desinstalar {k} herramientas")
                    .replace("{k}", &k(names))
            };
            format!("{said}{}{}", colon(lang), listed(names))
        }
        Effect::Upgrade => t("{n} veut mettre à jour les outils installés", "{n} wants to upgrade the installed tools",
                             "{n} möchte die installierten Werkzeuge aktualisieren",
                             "{n} quiere actualizar las herramientas instaladas"),
        Effect::Move(names) => match names.as_slice() {
            [one] => t("{n} veut déplacer ou renommer {x}", "{n} wants to move or rename {x}",
                       "{n} möchte {x} verschieben oder umbenennen", "{n} quiere mover o renombrar {x}")
                .replace("{x}", &q(one)),
            [] => t("{n} veut déplacer des fichiers", "{n} wants to move some files", "{n} möchte Dateien verschieben",
                    "{n} quiere mover unos archivos"),
            many => t("{n} veut déplacer {k} fichiers", "{n} wants to move {k} files", "{n} möchte {k} Dateien verschieben",
                      "{n} quiere mover {k} archivos")
                .replace("{k}", &k(many)),
        },
        Effect::Copy(names) => match names.as_slice() {
            [one] => t("{n} veut copier {x}", "{n} wants to copy {x}", "{n} möchte {x} kopieren", "{n} quiere copiar {x}")
                .replace("{x}", &q(one)),
            [] => t("{n} veut copier des fichiers", "{n} wants to copy some files", "{n} möchte Dateien kopieren",
                    "{n} quiere copiar unos archivos"),
            many => t("{n} veut copier {k} fichiers", "{n} wants to copy {k} files", "{n} möchte {k} Dateien kopieren",
                      "{n} quiere copiar {k} archivos")
                .replace("{k}", &k(many)),
        },
        Effect::Program(Some(file)) => t("{n} veut lancer le programme {x}{a}", "{n} wants to run the program {x}{a}",
                                         "{n} möchte das Programm {x}{w} ausführen{a}", "{n} quiere ejecutar el programa {x}{a}")
            .replace("{x}", &q(file)),
        Effect::Program(None) => small_program(name, lang, place),
        Effect::Download(host) => match host.clone() {
            Some(h) => t("{n} veut télécharger depuis {h}", "{n} wants to download from {h}",
                         "{n} möchte von {h} herunterladen", "{n} quiere descargar desde {h}")
                .replace("{h}", &h),
            None => t("{n} veut télécharger un fichier depuis Internet", "{n} wants to download a file from the Internet",
                      "{n} möchte eine Datei aus dem Internet herunterladen", "{n} quiere descargar un archivo de Internet"),
        },
        Effect::Clone(host) => match host.clone() {
            Some(h) => t("{n} veut copier un projet depuis {h}", "{n} wants to copy a project from {h}",
                         "{n} möchte ein Projekt von {h} kopieren", "{n} quiere copiar un proyecto desde {h}")
                .replace("{h}", &h),
            None => t("{n} veut copier un projet", "{n} wants to copy a project", "{n} möchte ein Projekt kopieren",
                      "{n} quiere copiar un proyecto"),
        },
        Effect::Pull => t("{n} veut récupérer les dernières modifications d’un projet",
                          "{n} wants to fetch a project’s latest changes",
                          "{n} möchte die neuesten Änderungen eines Projekts holen",
                          "{n} quiere traer los últimos cambios de un proyecto"),
        Effect::Commit => t("{n} veut enregistrer ses modifications dans un projet",
                            "{n} wants to save its changes in a project",
                            "{n} möchte seine Änderungen in einem Projekt speichern",
                            "{n} quiere guardar sus cambios en un proyecto"),
    }
}

/// « … veut supprimer 3 fichiers de ses dossiers » (the count when known; « de ses dossiers » in its own space).
fn deleted_files(name: &str, lang: &str, place: Place, count: Option<usize>) -> String {
    let own = place == Place::Own;
    let template = match (count, own) {
        (Some(_), true) => pick(lang, "{n} veut supprimer {k} fichiers de ses dossiers",
                                "{n} wants to delete {k} files from its folders",
                                "{n} möchte {k} Dateien aus seinen Ordnern löschen",
                                "{n} quiere borrar {k} archivos de sus carpetas"),
        (Some(_), false) => pick(lang, "{n} veut supprimer {k} fichiers", "{n} wants to delete {k} files",
                                 "{n} möchte {k} Dateien löschen", "{n} quiere borrar {k} archivos"),
        (None, true) => pick(lang, "{n} veut supprimer des fichiers de ses dossiers",
                             "{n} wants to delete files from its folders",
                             "{n} möchte Dateien aus seinen Ordnern löschen",
                             "{n} quiere borrar archivos de sus carpetas"),
        (None, false) => pick(lang, "{n} veut supprimer des fichiers", "{n} wants to delete files",
                              "{n} möchte Dateien löschen", "{n} quiere borrar archivos"),
    };
    template.replace("{n}", name).replace("{k}", &count.unwrap_or(0).to_string())
}

/// « … veut lancer un petit programme dans son espace » (or « sur ta machine, avec accès à tes fichiers »).
pub(crate) fn small_program(name: &str, lang: &str, place: Place) -> String {
    fill(pick(lang, "{n} veut lancer un petit programme{a}", "{n} wants to run a small program{a}",
              "{n} möchte{w} ein kleines Programm ausführen{a}", "{n} quiere ejecutar un pequeño programa{a}"),
         name, lang, place)
}

/// « … veut lancer une commande dans son espace »: nothing recognised.
pub(crate) fn a_command(name: &str, lang: &str, place: Place) -> String {
    fill(pick(lang, "{n} veut lancer une commande{a}", "{n} wants to run a command{a}",
              "{n} möchte{w} einen Befehl ausführen{a}", "{n} quiere ejecutar un comando{a}"),
         name, lang, place)
}

/// « … veut supprimer des données »: Hermes says the command deletes, the bridge can't say what.
pub(crate) fn deletes_something(name: &str, lang: &str, place: Place) -> String {
    fill(pick(lang, "{n} veut supprimer des fichiers ou des données{a}", "{n} wants to delete files or data{a}",
              "{n} möchte{w} Dateien oder Daten löschen{a}", "{n} quiere borrar archivos o datos{a}"),
         name, lang, place)
}

/// The command's effect in one sentence, None when no step is recognised (the caller says « une commande »).
pub fn describe(name: &str, lang: &str, command: &str, place: Place) -> Option<String> {
    let mut sudo = false;
    let effects = read(command, &mut sudo, 0);
    let acting: Vec<&Effect> = effects.iter().filter(|e| **e != Effect::Harmless).collect();
    let top = acting.iter().filter(|e| ***e != Effect::Unknown).max_by_key(|e| e.rank())?;
    let mut text = sentence(name, lang, place, top);
    if text.is_empty() {
        return None;
    }
    if sudo {
        text.push_str(pick(lang, ", avec les droits d’administrateur", ", with administrator rights",
                           " (mit Administratorrechten)", ", con permisos de administrador"));
    }
    let others = acting.iter().filter(|e| ***e != **top).count();
    if others > 0 {
        text.push_str(pick(lang, ", et d’autres actions", ", and other actions", " (und weitere Schritte)",
                           ", y otras acciones"));
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fr(command: &str) -> Option<String> {
        describe("Wellness", "fr", command, Place::Own)
    }

    #[test]
    fn deleting() {
        assert_eq!(fr("rm a.txt b.txt c.txt").as_deref(), Some("Wellness veut supprimer 3 fichiers de ses dossiers"));
        assert_eq!(fr("rm -rf /workspace/old").as_deref(),
                   Some("Wellness veut supprimer le dossier « old » et tout son contenu"));
        assert_eq!(fr("rm notes.md").as_deref(), Some("Wellness veut supprimer le fichier « notes.md »"));
        assert_eq!(fr("rm -f *.tmp").as_deref(), Some("Wellness veut supprimer des fichiers de ses dossiers"));
        assert_eq!(fr("rm -rf a b").as_deref(), Some("Wellness veut supprimer 2 dossiers ou fichiers, avec leur contenu"));
        assert_eq!(fr("sudo rm -rf /").as_deref(),
                   Some("Wellness veut supprimer tous les fichiers, avec les droits d’administrateur"));
        assert_eq!(fr("rm -rf ~").as_deref(), Some("Wellness veut supprimer tous les fichiers du dossier personnel"));
        assert_eq!(fr("find . -name '*.log' -delete").as_deref(), Some("Wellness veut supprimer des fichiers de ses dossiers"));
        assert_eq!(fr("find /tmp -name x | xargs rm -f").as_deref(),
                   Some("Wellness veut supprimer des fichiers de ses dossiers"));
        assert_eq!(describe("Wellness", "fr", "rm a b c", Place::Machine).as_deref(),
                   Some("Wellness veut supprimer 3 fichiers"));
        assert_eq!(describe("Wellness", "de", "rm a b c", Place::Own).as_deref(),
                   Some("Wellness möchte 3 Dateien aus seinen Ordnern löschen"));
        assert_eq!(describe("Wellness", "en", "rm -r build", Place::Own).as_deref(),
                   Some("Wellness wants to delete the folder “build” and everything in it"));
        assert_eq!(describe("Wellness", "es", "rmdir vieja", Place::Own).as_deref(),
                   Some("Wellness quiere borrar la carpeta «vieja» y todo su contenido"));
    }

    #[test]
    fn installing() {
        assert_eq!(fr("pip install pypdf").as_deref(), Some("Wellness veut installer un outil : pypdf, pour lire les PDF"));
        assert_eq!(fr("pip3 install --user 'pdfplumber>=0.10'").as_deref(),
                   Some("Wellness veut installer un outil : pdfplumber, pour lire les PDF"));
        assert_eq!(fr("uv pip install rich").as_deref(), Some("Wellness veut installer un outil : rich"));
        assert_eq!(fr("python3 -m pip install pandas openpyxl").as_deref(),
                   Some("Wellness veut installer 2 outils : pandas, openpyxl"));
        assert_eq!(fr("pip install -r requirements.txt").as_deref(),
                   Some("Wellness veut installer les outils dont un projet a besoin"));
        assert_eq!(fr("npm install").as_deref(), Some("Wellness veut installer les outils dont un projet a besoin"));
        assert_eq!(fr("pnpm add a b c d").as_deref(), Some("Wellness veut installer 4 outils : a, b, c, …"));
        assert_eq!(fr("sudo apt-get install -y ffmpeg").as_deref(),
                   Some("Wellness veut installer un outil : ffmpeg, pour les sons et les vidéos, avec les droits d’administrateur"));
        assert_eq!(fr("brew install tesseract").as_deref(),
                   Some("Wellness veut installer un outil : tesseract, pour lire le texte des images"));
        assert_eq!(fr("pip uninstall -y requests").as_deref(), Some("Wellness veut désinstaller un outil : requests"));
        assert_eq!(fr("apt update && apt upgrade -y").as_deref(), Some("Wellness veut mettre à jour les outils installés"));
        assert_eq!(describe("Wellness", "en", "yarn add yt-dlp", Place::Own).as_deref(),
                   Some("Wellness wants to install a tool: yt-dlp, to download videos"));
        assert_eq!(describe("Wellness", "de", "pip install pypdf", Place::Own).as_deref(),
                   Some("Wellness möchte ein Werkzeug installieren: pypdf, um PDFs zu lesen"));
    }

    #[test]
    fn downloading_and_sending() {
        assert_eq!(fr("curl -o page.html https://www.example.com/a").as_deref(),
                   Some("Wellness veut télécharger depuis example.com"));
        assert_eq!(fr("wget https://files.example.org/x.zip").as_deref(),
                   Some("Wellness veut télécharger depuis files.example.org"));
        assert_eq!(fr("curl -X POST -d @notes.json https://api.example.com/v1").as_deref(),
                   Some("Wellness veut envoyer des données à api.example.com"));
        assert_eq!(fr("curl -F file=@a.pdf https://upload.example.com").as_deref(),
                   Some("Wellness veut envoyer des données à upload.example.com"));
        assert_eq!(fr("curl --data-binary @x https://e.example").as_deref(), Some("Wellness veut envoyer des données à e.example"));
        assert_eq!(fr("wget --post-data 'a=1' http://example.net").as_deref(), Some("Wellness veut envoyer des données à example.net"));
        assert_eq!(fr("curl -fsSL https://get.example.sh | sh").as_deref(),
                   Some("Wellness veut télécharger un programme depuis get.example.sh et le lancer tout de suite"));
        assert_eq!(fr("curl -s https://x.example/i.sh | sudo bash -s --").as_deref(),
                   Some("Wellness veut télécharger un programme depuis x.example et le lancer tout de suite, avec les droits d’administrateur"));
        assert_eq!(fr("scp notes.txt alex@backup.example.com:/srv/").as_deref(),
                   Some("Wellness veut envoyer des données à backup.example.com"));
        assert_eq!(describe("Wellness", "es", "curl -d x=1 https://example.com", Place::Own).as_deref(),
                   Some("Wellness quiere enviar datos a example.com"));
    }

    #[test]
    fn git_programs_and_the_rest() {
        assert_eq!(fr("git clone https://github.com/a/b.git").as_deref(), Some("Wellness veut copier un projet depuis github.com"));
        assert_eq!(fr("git clone git@gitlab.com:a/b.git").as_deref(), Some("Wellness veut copier un projet depuis gitlab.com"));
        assert_eq!(fr("cd repo && git pull").as_deref(), Some("Wellness veut récupérer les dernières modifications d’un projet"));
        assert_eq!(fr("git push --force origin main").as_deref(),
                   Some("Wellness veut envoyer les modifications d’un projet en ligne, en remplaçant ce qui y est"));
        assert_eq!(fr("git add . && git commit -m 'x'").as_deref(), Some("Wellness veut enregistrer ses modifications dans un projet"));
        assert_eq!(fr("git reset --hard HEAD~1").as_deref(), Some("Wellness veut effacer les modifications en cours d’un projet"));
        assert_eq!(fr("python3 analyse.py").as_deref(), Some("Wellness veut lancer le programme « analyse.py » dans son espace"));
        assert_eq!(describe("Wellness", "fr", "python3 analyse.py", Place::Machine).as_deref(),
                   Some("Wellness veut lancer le programme « analyse.py » sur ta machine, avec accès à tes fichiers"));
        assert_eq!(describe("Wellness", "de", "./run.sh", Place::Own).as_deref(),
                   Some("Wellness möchte das Programm „run.sh“ in seinem Bereich ausführen"));
        assert_eq!(fr("node -e 'console.log(1)'").as_deref(), Some("Wellness veut lancer un petit programme dans son espace"));
        assert_eq!(fr("bash -c 'rm -rf /workspace/tmp'").as_deref(),
                   Some("Wellness veut supprimer le dossier « tmp » et tout son contenu"));
        assert_eq!(fr("chmod +x run.sh").as_deref(), Some("Wellness veut changer qui peut lire ou modifier « run.sh »"));
        assert_eq!(fr("chown -R alex /srv/data/*").as_deref(), Some("Wellness veut changer qui peut lire ou modifier des fichiers"));
        assert_eq!(fr("mv draft.md final.md").as_deref(), Some("Wellness veut déplacer ou renommer « draft.md »"));
        assert_eq!(fr("cp a b dest/").as_deref(), Some("Wellness veut copier 2 fichiers"));
        assert_eq!(fr("pkill -f chromium").as_deref(), Some("Wellness veut arrêter le programme « chromium »"));
        assert_eq!(fr("kill -9 1234").as_deref(), Some("Wellness veut arrêter un programme en cours"));
        assert_eq!(fr("systemctl --user restart hermes-gateway").as_deref(),
                   Some("Wellness veut redémarrer un programme qui tourne en fond"));
        assert_eq!(fr("crontab -r").as_deref(), Some("Wellness veut effacer toutes ses tâches programmées"));
        assert_eq!(fr("crontab /tmp/jobs").as_deref(), Some("Wellness veut changer ses tâches programmées"));
        // chained: the riskiest, and the others
        assert_eq!(fr("mkdir out && curl -o out/a.zip https://example.com/a.zip && rm -rf old").as_deref(),
                   Some("Wellness veut supprimer le dossier « old » et tout son contenu, et d’autres actions"));
        // heredoc bodies are not steps
        assert_eq!(fr("cat > notes.txt <<'EOF'\nrm -rf /\nEOF\nrm notes.old").as_deref(),
                   Some("Wellness veut supprimer le fichier « notes.old »"));
    }

    #[test]
    fn nothing_claimed_when_not_read() {
        assert_eq!(fr("dd if=/dev/zero of=/dev/sda"), None);
        assert_eq!(fr("ssh host 'ls'"), None);
        assert_eq!(fr("ls -la"), None);
        assert_eq!(fr("echo 'unclosed"), None);
        // an unknown step beside a known one: the known one, and « d'autres actions »
        assert_eq!(fr("mkfs.ext4 /dev/sdb && rm a").as_deref(),
                   Some("Wellness veut supprimer le fichier « a », et d’autres actions"));
        assert_eq!(a_command("Wellness", "fr", Place::Own), "Wellness veut lancer une commande dans son espace");
        assert_eq!(a_command("Wellness", "de", Place::Machine),
                   "Wellness möchte auf deinem Gerät einen Befehl ausführen, mit Zugriff auf deine Dateien");
        assert_eq!(a_command("Wellness", "en", Place::Hosted), "Wellness wants to run a command, with access to your files");
    }
}
