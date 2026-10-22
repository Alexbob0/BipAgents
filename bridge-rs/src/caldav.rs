//! The calendars of a connected mailbox, by CalDAV (decided 2026-10-21): they work when the phone is off. For each
//! mailbox (mailbox.rs) the person may switch « Utiliser aussi son agenda » on: the bridge finds the CalDAV server
//! from the address (a small table of known providers, else RFC 6764's `https://<domain>/.well-known/caldav`, or an
//! address the person gives), logs in with the mailbox's own credentials (an iCloud app-specific password works for
//! both), and keeps the calendars it found in the mailbox's encrypted entry. The mailbox's agents then read those
//! calendars through the same `calendar_list` tool as the phone's (each event says its source: « phone », the
//! mailbox's name), and `calendar_add` writes to one of them (a PUT of an iCalendar VEVENT). `calendar_delete` works
//! on their events too; `calendar_update` does not (the person changes them in their calendar app). As for the
//! phone, an agent's write needs no approval of its own: the person chose the agents of that mailbox.
//!
//! Not possible this way: Google Agenda (its CalDAV needs Google's own sign-in, a « sensitive » scope Google must
//! verify) and Outlook / Microsoft 365 (no CalDAV). For those, the person shares their phone's calendar.
//! Not done: DNS SRV discovery (`_caldavs._tcp`, RFC 6764 §3): the well-known address covers the providers seen.
//!
//! The credentials never leave the mailbox's entry (Fernet-encrypted); no agent sees them. The XML of WebDAV and the
//! iCalendar text are read and written by the small parsers below (no new crate).
//!
//!     PATCH /v1/mail/accounts/{id}   {calendar: bool, calendar_url?}   (mailbox.rs: found and tried first)
//!
//! No Python counterpart (owner group: tools-knowledge).

use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde_json::{json, Value};

use crate::calendar::Moment;
use crate::config::Config;
use crate::tools_knowledge::{self as tk, tool_text};
use crate::{py, Dict};

pub const TIMEOUT: u64 = 20;
/// What the app says for Gmail and Outlook (in its own words, translated there).
pub const NOT_POSSIBLE: &str = "Google and Microsoft calendars can't be reached this way: share the phone's calendar \
instead (with the account added in the phone's settings).";

/// A calendar found on a mailbox's CalDAV server.
#[derive(Debug, Clone, PartialEq)]
pub struct DavCalendar {
    pub href: String,
    pub title: String,
    pub color: Option<String>,
}

/// A mailbox whose calendars an agent may use.
#[derive(Debug, Clone)]
pub struct Source {
    pub account: String,
    pub label: String,
    pub username: String,
    pub password: String,
    pub calendars: Vec<DavCalendar>,
}

impl Source {
    fn calendar_id(&self, calendar: &DavCalendar) -> String {
        format!("mail:{}:{}", self.account, calendar.href)
    }
}

fn text(account: &Dict, key: &str) -> String {
    account.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The `calendar` entry kept in a mailbox (`{url, calendars: [{href, title, color}]}`), as calendars.
pub fn calendars_of(account: &Dict) -> Option<Vec<DavCalendar>> {
    let entry = account.get("calendar")?.as_object()?;
    Some(
        entry
            .get("calendars")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|c| {
                        Some(DavCalendar {
                            href: c.get("href")?.as_str()?.to_string(),
                            title: c.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
                            color: c.get("color").and_then(Value::as_str).map(str::to_string),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// The entry kept for calendars found at `url`.
pub fn entry(url: &str, calendars: &[DavCalendar]) -> Value {
    let listed: Vec<Value> =
        calendars.iter().map(|c| json!({"href": c.href, "title": c.title, "color": c.color})).collect();
    json!({"url": url, "calendars": listed})
}

/// The mailboxes of `agent` with their calendar on.
pub fn sources(config: &Config, agent: &str) -> Vec<Source> {
    let Ok(accounts) = crate::mailbox::of(config).of_agent(agent) else { return Vec::new() };
    accounts
        .iter()
        .filter_map(|a| {
            let calendars = calendars_of(a)?;
            Some(Source {
                account: text(a, "id"),
                label: text(a, "label"),
                username: text(a, "username"),
                password: text(a, "password"),
                calendars,
            })
        })
        .collect()
}

/// Whether `agent` reads a mailbox's calendar.
pub fn shared_with(config: &Config, agent: &str) -> bool {
    !sources(config, agent).is_empty()
}

// -- WebDAV's XML ------------------------------------------------------------------------------------------------------

/// An XML element: its local name (the namespace prefix dropped), its attributes, children and text.
#[derive(Debug, Default, Clone)]
pub struct Node {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
    pub text: String,
}

impl Node {
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Every descendant named `name`, depth first.
    pub fn all<'a>(&'a self, name: &str, out: &mut Vec<&'a Node>) {
        for c in &self.children {
            if c.name == name {
                out.push(c);
            }
            c.all(name, out);
        }
    }

    pub fn find(&self, name: &str) -> Option<&Node> {
        let mut found = Vec::new();
        self.all(name, &mut found);
        found.into_iter().next()
    }

    /// The text of the first `name` below, trimmed.
    pub fn text_of(&self, name: &str) -> Option<String> {
        self.find(name).map(|n| n.text.trim().to_string()).filter(|t| !t.is_empty())
    }
}

pub fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") || e.starts_with("#X") => {
                u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32)
            }
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn local_name(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_string()
}

/// A tolerant reader for WebDAV answers: elements, attributes, text and CDATA; comments and declarations skipped.
pub fn parse_xml(input: &str) -> Node {
    let mut stack: Vec<Node> = vec![Node { name: "#root".into(), ..Node::default() }];
    let mut rest = input;
    while !rest.is_empty() {
        let Some(at) = rest.find('<') else {
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&unescape(rest));
            }
            break;
        };
        if at > 0 {
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&unescape(&rest[..at]));
            }
        }
        rest = &rest[at..];
        if let Some(body) = rest.strip_prefix("<![CDATA[") {
            let end = body.find("]]>").unwrap_or(body.len());
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&body[..end]);
            }
            rest = body.get(end + 3..).unwrap_or("");
            continue;
        }
        if rest.starts_with("<!--") {
            rest = rest.find("-->").map(|e| &rest[e + 3..]).unwrap_or("");
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            rest = rest.find('>').map(|e| &rest[e + 1..]).unwrap_or("");
            continue;
        }
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        if let Some(closing) = tag.strip_prefix('/') {
            let name = local_name(closing.trim());
            // Close up to the matching element (a stray closing tag is ignored).
            if let Some(pos) = stack.iter().rposition(|n| n.name == name) {
                while stack.len() > pos.max(1) {
                    let node = stack.pop().unwrap_or_default();
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(node);
                    }
                }
            }
            continue;
        }
        let self_closing = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let mut parts = tag.splitn(2, char::is_whitespace);
        let name = local_name(parts.next().unwrap_or(""));
        let attrs = parse_attrs(parts.next().unwrap_or(""));
        let node = Node { name, attrs, ..Node::default() };
        if self_closing {
            if let Some(top) = stack.last_mut() {
                top.children.push(node);
            }
        } else {
            stack.push(node);
        }
    }
    while stack.len() > 1 {
        let node = stack.pop().unwrap_or_default();
        if let Some(parent) = stack.last_mut() {
            parent.children.push(node);
        }
    }
    stack.pop().unwrap_or_default()
}

fn parse_attrs(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = text.trim();
    while let Some(eq) = rest.find('=') {
        let name = local_name(rest[..eq].trim());
        let after = rest[eq + 1..].trim_start();
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else { break };
        let body = &after[1..];
        let end = body.find(quote).unwrap_or(body.len());
        out.push((name, unescape(&body[..end])));
        rest = body.get(end + 1..).unwrap_or("").trim_start();
    }
    out
}

// -- iCalendar ---------------------------------------------------------------------------------------------------------

/// One VEVENT's properties: (name, parameters, value), lines unfolded.
fn vevents(ics: &str) -> Vec<Vec<(String, String, String)>> {
    let unfolded = ics.replace("\r\n", "\n").replace("\n ", "").replace("\n\t", "");
    let mut events = Vec::new();
    let mut current: Option<Vec<(String, String, String)>> = None;
    let mut depth = 0;
    for line in unfolded.lines() {
        let upper = line.trim().to_ascii_uppercase();
        if upper == "BEGIN:VEVENT" {
            current = Some(Vec::new());
            depth = 0;
            continue;
        }
        if upper == "END:VEVENT" {
            if let Some(event) = current.take() {
                events.push(event);
            }
            continue;
        }
        let Some(event) = current.as_mut() else { continue };
        // A VALARM inside the event: not the event's own properties.
        if upper.starts_with("BEGIN:") {
            depth += 1;
            continue;
        }
        if upper.starts_with("END:") {
            depth -= 1;
            continue;
        }
        if depth > 0 {
            continue;
        }
        let Some(colon) = value_colon(line) else { continue };
        let (head, value) = (&line[..colon], &line[colon + 1..]);
        let (name, params) = match head.find(';') {
            Some(semi) => (&head[..semi], &head[semi + 1..]),
            None => (head, ""),
        };
        event.push((name.to_ascii_uppercase(), params.to_string(), value.to_string()));
    }
    events
}

/// The colon that ends a property's name and parameters (a quoted parameter may hold one).
fn value_colon(line: &str) -> Option<usize> {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => return Some(i),
            _ => {}
        }
    }
    None
}

fn ics_unescape(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn ics_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace("\r\n", "\\n").replace('\n', "\\n")
}

/// An iCalendar moment as the calendar copy writes them: a day, a UTC time in the bridge's local offset, or a local
/// (floating or TZID) time as written (taken as the person's own time).
fn ics_moment(params: &str, value: &str) -> Option<(String, bool)> {
    let value = value.trim();
    if params.to_ascii_uppercase().contains("VALUE=DATE") && !params.to_ascii_uppercase().contains("DATE-TIME")
        || value.len() == 8
    {
        let day = NaiveDate::parse_from_str(value, "%Y%m%d").ok()?;
        return Some((day.format("%Y-%m-%d").to_string(), true));
    }
    if let Some(utc) = value.strip_suffix('Z') {
        let naive = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        let local = Utc.from_utc_datetime(&naive).with_timezone(&Local);
        return Some((local.fixed_offset().to_rfc3339(), false));
    }
    let naive = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()?;
    Some((naive.format("%Y-%m-%dT%H:%M:%S").to_string(), false))
}

/// `P1D`, `PT1H30M`, `P2W`: minutes.
fn duration_minutes(value: &str) -> Option<i64> {
    let value = value.trim().trim_start_matches('+');
    let body = value.strip_prefix('P')?;
    let mut minutes = 0i64;
    let mut number = String::new();
    let mut in_time = false;
    for c in body.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => number.push(c),
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                minutes += match (unit, in_time) {
                    ('W', _) => n * 7 * 1440,
                    ('D', _) => n * 1440,
                    ('H', true) => n * 60,
                    ('M', true) => n,
                    ('S', true) => n / 60,
                    _ => return None,
                };
            }
        }
    }
    Some(minutes)
}

/// The events of an iCalendar text, in the calendar copy's shape (`calendar` and `id` left to the caller).
pub fn read_ics(ics: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for props in vevents(ics) {
        let get = |name: &str| props.iter().find(|(n, _, _)| n == name);
        let Some((start, all_day)) = get("DTSTART").and_then(|(_, p, v)| ics_moment(p, v)) else { continue };
        let mut end = get("DTEND").and_then(|(_, p, v)| ics_moment(p, v)).map(|(e, _)| e);
        if end.is_none() {
            if let (Some(minutes), Some(moment)) =
                (get("DURATION").and_then(|(_, _, v)| duration_minutes(v)), Moment::parse(&start))
            {
                end = Some(moment.plus(minutes).write());
            }
        }
        let mut end = end.unwrap_or_else(|| start.clone());
        if all_day {
            // iCalendar's end of an all-day event is the day after its last one.
            if let (Ok(first), Ok(next)) =
                (NaiveDate::parse_from_str(&start, "%Y-%m-%d"), NaiveDate::parse_from_str(&end, "%Y-%m-%d"))
            {
                end = (next - chrono::Duration::days(1)).max(first).format("%Y-%m-%d").to_string();
            }
        }
        let field = |name: &str| get(name).map(|(_, _, v)| ics_unescape(v)).unwrap_or_default();
        let mut event = json!({"uid": field("UID"), "title": tk::head(&field("SUMMARY"), 1000), "start": start, "end": end,
                               "all_day": all_day});
        let location = field("LOCATION");
        if !location.is_empty() {
            event["location"] = json!(tk::head(&location, 500));
        }
        let notes = field("DESCRIPTION");
        if !notes.is_empty() {
            event["notes"] = json!(tk::head(&notes, 4000));
        }
        if get("RRULE").is_some() || get("RECURRENCE-ID").is_some() {
            event["recurring"] = json!(true);
        }
        out.push(event);
    }
    out
}

fn ics_time(moment: Moment) -> (String, String) {
    match moment {
        Moment::Day(day) => (";VALUE=DATE".into(), day.format("%Y%m%d").to_string()),
        Moment::Local(local) => {
            let at = Local.from_local_datetime(&local).earliest().map(|l| l.with_timezone(&Utc));
            match at {
                Some(utc) => (String::new(), utc.format("%Y%m%dT%H%M%SZ").to_string()),
                None => (String::new(), local.format("%Y%m%dT%H%M%S").to_string()),
            }
        }
        Moment::Zoned(zoned) => (String::new(), zoned.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ").to_string()),
    }
}

/// A new event (calendar::new_event's arguments) as an iCalendar text.
pub fn write_ics(uid: &str, event: &Value, now: DateTime<Utc>) -> Option<String> {
    let get = |k: &str| event.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let start = Moment::parse(&get("start"))?;
    let mut end = Moment::parse(&get("end")).unwrap_or(start);
    if let (Moment::Day(_), Moment::Day(_)) = (start, end) {
        end = end.plus(1440); // the day after the last one
    }
    let (start_params, start_value) = ics_time(start);
    let (end_params, end_value) = ics_time(end);
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".into(),
        "PRODID:-//BipAgents//bridge//EN".into(),
        "BEGIN:VEVENT".into(),
        format!("UID:{uid}"),
        format!("DTSTAMP:{}", now.format("%Y%m%dT%H%M%SZ")),
        format!("DTSTART{start_params}:{start_value}"),
        format!("DTEND{end_params}:{end_value}"),
        format!("SUMMARY:{}", ics_escape(&get("title"))),
    ];
    if !get("location").is_empty() {
        lines.push(format!("LOCATION:{}", ics_escape(&get("location"))));
    }
    if !get("notes").is_empty() {
        lines.push(format!("DESCRIPTION:{}", ics_escape(&get("notes"))));
    }
    lines.extend(["END:VEVENT".to_string(), "END:VCALENDAR".into()]);
    Some(lines.iter().map(|l| fold(l)).collect::<Vec<_>>().join("\r\n") + "\r\n")
}

/// Lines longer than 75 octets folded (RFC 5545 §3.1), on character boundaries.
fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut width = 0;
    for c in line.chars() {
        if width + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += c.len_utf8();
    }
    out
}

// -- HTTP --------------------------------------------------------------------------------------------------------------

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(TIMEOUT))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default()
}

/// Plain http only to this machine (a CalDAV server of one's own, the tests); https elsewhere.
pub fn allowed_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else { return false };
    match parsed.scheme() {
        "https" => true,
        "http" => matches!(parsed.host_str(), Some("127.0.0.1") | Some("localhost") | Some("[::1]") | Some("::1")),
        _ => false,
    }
}

struct Answer {
    status: u16,
    body: String,
    url: String,
}

/// A WebDAV request with the mailbox's credentials, its redirects followed (same method, at most 5).
async fn dav(
    method: &str,
    url: &str,
    user: &str,
    password: &str,
    depth: Option<&str>,
    body: Option<String>,
    headers: &[(&str, &str)],
) -> Result<Answer, String> {
    let http = client();
    let mut url = url.to_string();
    for _ in 0..6 {
        if !allowed_url(&url) {
            return Err(format!("{url}: not an https address"));
        }
        let verb = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
        let mut request = http.request(verb, &url).basic_auth(user, Some(password));
        if let Some(depth) = depth {
            request = request.header("Depth", depth);
        }
        for (k, v) in headers {
            request = request.header(*k, *v);
        }
        if let Some(body) = &body {
            let kind = if method == "PUT" { "text/calendar; charset=utf-8" } else { "application/xml; charset=utf-8" };
            request = request.header("Content-Type", kind).body(body.clone());
        }
        let answer = request.send().await.map_err(|e| {
            let host = url::Url::parse(&url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
            if e.is_timeout() {
                format!("{host} doesn't answer (timeout)")
            } else {
                format!("{host} doesn't answer")
            }
        })?;
        let status = answer.status().as_u16();
        if (300..400).contains(&status) {
            let Some(next) = answer.headers().get("location").and_then(|l| l.to_str().ok()) else {
                return Err("a redirect without an address".into());
            };
            url = join(&url, next);
            continue;
        }
        let body = answer.text().await.unwrap_or_default();
        return Ok(Answer { status, body, url });
    }
    Err("too many redirects".into())
}

fn join(base: &str, href: &str) -> String {
    url::Url::parse(base).and_then(|b| b.join(href)).map(|u| u.to_string()).unwrap_or_else(|_| href.to_string())
}

fn refused(status: u16) -> Option<String> {
    match status {
        401 | 403 => Some("login refused (iCloud and others: an app-specific password, the same as the mail's)".into()),
        _ => None,
    }
}

// -- discovery ---------------------------------------------------------------------------------------------------------

/// Where to start for an address (and the mailbox's IMAP host): known providers, then the well-known addresses.
/// Err: a provider whose calendar can't be reached this way.
pub fn candidates(username: &str, host: &str) -> Result<Vec<String>, String> {
    let domain = username.rsplit('@').next().unwrap_or("").trim().to_ascii_lowercase();
    let host = host.trim().to_ascii_lowercase();
    let google = ["gmail.com", "googlemail.com"];
    let microsoft = ["outlook.com", "hotmail.com", "hotmail.fr", "live.com", "live.fr", "msn.com", "outlook.fr"];
    if google.contains(&domain.as_str())
        || microsoft.contains(&domain.as_str())
        || host.ends_with("gmail.com")
        || host.ends_with("office365.com")
        || host.ends_with("outlook.com")
    {
        return Err(NOT_POSSIBLE.into());
    }
    let known: &[(&[&str], &str)] = &[
        (&["icloud.com", "me.com", "mac.com", "imap.mail.me.com"], "https://caldav.icloud.com/"),
        (&["fastmail.com", "fastmail.fm", "imap.fastmail.com"], "https://caldav.fastmail.com/"),
        (&["ik.me", "etik.com", "ikmail.com", "mail.infomaniak.com"], "https://sync.infomaniak.com/"),
        (&["zoho.eu", "zohomail.eu", "imap.zoho.eu"], "https://calendar.zoho.eu/"),
        (&["zoho.com", "zohomail.com", "imap.zoho.com"], "https://calendar.zoho.com/"),
        (
            &["posteo.de", "posteo.net", "posteo.eu", "posteo.org", "posteo.at", "posteo.ch", "posteo.be"],
            "https://posteo.de:8443/",
        ),
        (&["mailbox.org", "imap.mailbox.org"], "https://dav.mailbox.org/"),
    ];
    for (names, url) in known {
        if names.contains(&domain.as_str()) || names.contains(&host.as_str()) {
            return Ok(vec![url.to_string()]);
        }
    }
    let mut out = Vec::new();
    let mut add_domain = |d: &str| {
        if d.contains('.') && !["localhost", "127.0.0.1"].contains(&d) {
            for url in [format!("https://{d}/.well-known/caldav"), format!("https://{d}/remote.php/dav/")] {
                if !out.contains(&url) {
                    out.push(url);
                }
            }
        }
    };
    add_domain(&domain);
    // The IMAP host's own domain (imap.example.org → example.org), for a provider that hosts the mail elsewhere.
    let host_domain =
        host.split_once('.').filter(|(first, _)| ["imap", "mail", "ssl0"].contains(first)).map(|(_, d)| d);
    if let Some(d) = host_domain {
        add_domain(d);
    }
    Ok(out)
}

const PRINCIPAL: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:current-user-principal/></d:prop></d:propfind>"#;
const HOME: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><c:calendar-home-set/></d:prop></d:propfind>"#;
const LIST: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:a="http://apple.com/ns/ical/"><d:prop><d:displayname/><d:resourcetype/><a:calendar-color/><c:supported-calendar-component-set/><d:current-user-privilege-set/></d:prop></d:propfind>"#;

/// The href inside `name` (current-user-principal, calendar-home-set), made absolute.
fn href_in(answer: &Answer, name: &str) -> Option<String> {
    let tree = parse_xml(&answer.body);
    let node = tree.find(name)?;
    let href = node.text_of("href")?;
    Some(join(&answer.url, &href))
}

/// The calendars at a calendar home (those that hold events).
fn calendars_in(answer: &Answer) -> Vec<DavCalendar> {
    let tree = parse_xml(&answer.body);
    let mut responses = Vec::new();
    tree.all("response", &mut responses);
    responses
        .into_iter()
        .filter_map(|r| {
            let href = r.child("href")?.text.trim().to_string();
            let kind = r.find("resourcetype")?;
            kind.child("calendar")?;
            if let Some(set) = r.find("supported-calendar-component-set") {
                let events = set.children.iter().any(|c| {
                    c.name == "comp" && c.attrs.iter().any(|(k, v)| k == "name" && v.eq_ignore_ascii_case("VEVENT"))
                });
                if !set.children.is_empty() && !events {
                    return None;
                }
            }
            let title = r
                .text_of("displayname")
                .unwrap_or_else(|| href.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string());
            let color = r.text_of("calendar-color").map(|c| c.chars().take(7).collect());
            Some(DavCalendar { href: join(&answer.url, &href), title: tk::head(&title, 200).to_string(), color })
        })
        .collect()
}

/// The calendar home and its calendars for a mailbox's credentials, from `hint` (an address the person gave) or
/// the candidates. Err: why not, in words safe to show.
pub async fn discover(
    username: &str,
    host: &str,
    password: &str,
    hint: Option<&str>,
) -> Result<(String, Vec<DavCalendar>), String> {
    let starts = match hint.map(str::trim).filter(|h| !h.is_empty()) {
        Some(hint) if allowed_url(hint) => vec![hint.to_string()],
        Some(_) => return Err("the calendar's address must start with https://".into()),
        None => candidates(username, host)?,
    };
    if starts.is_empty() {
        return Err("no calendar server found for this address: give its CalDAV address".into());
    }
    let mut last = "no calendar server found for this address: give its CalDAV address".to_string();
    for start in starts {
        let principal = match dav("PROPFIND", &start, username, password, Some("0"), Some(PRINCIPAL.into()), &[]).await
        {
            Ok(answer) if answer.status == 207 => href_in(&answer, "current-user-principal").unwrap_or(answer.url),
            Ok(answer) => {
                if let Some(why) = refused(answer.status) {
                    return Err(why);
                }
                continue;
            }
            Err(why) => {
                last = why;
                continue;
            }
        };
        let home = match dav("PROPFIND", &principal, username, password, Some("0"), Some(HOME.into()), &[]).await {
            Ok(answer) if answer.status == 207 => href_in(&answer, "calendar-home-set").unwrap_or(principal.clone()),
            Ok(answer) => {
                if let Some(why) = refused(answer.status) {
                    return Err(why);
                }
                principal.clone()
            }
            Err(why) => {
                last = why;
                continue;
            }
        };
        match dav("PROPFIND", &home, username, password, Some("1"), Some(LIST.into()), &[]).await {
            Ok(answer) if answer.status == 207 => {
                let found = calendars_in(&answer);
                if found.is_empty() {
                    last = "no calendar in this account".into();
                    continue;
                }
                return Ok((home, found));
            }
            Ok(answer) => {
                if let Some(why) = refused(answer.status) {
                    return Err(why);
                }
            }
            Err(why) => last = why,
        }
    }
    Err(last)
}

// -- the tools ---------------------------------------------------------------------------------------------------------

/// What the mailboxes' calendars add to `calendar_list` (calendar.rs): their calendars, their events of the period,
/// and the ones that didn't answer.
#[derive(Debug, Default, Clone)]
pub struct Remote {
    pub any: bool,
    pub calendars: Vec<Value>,
    pub events: Vec<Value>,
    pub failed: Vec<String>,
}

impl Remote {
    /// The calendars only (for the write tools: which names are the mailboxes').
    pub fn of(sources: &[Source]) -> Remote {
        let mut remote = Remote { any: !sources.is_empty(), ..Remote::default() };
        for source in sources {
            for calendar in &source.calendars {
                remote.calendars.push(json!({"id": source.calendar_id(calendar), "title": calendar.title,
                                             "source": source.label}));
            }
        }
        remote
    }
}

fn query(from: NaiveDate, to: NaiveDate) -> String {
    let stamp = |d: NaiveDate| d.format("%Y%m%dT000000Z").to_string();
    let (start, end) = (stamp(from - chrono::Duration::days(1)), stamp(to + chrono::Duration::days(2)));
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data><c:expand start="{start}" end="{end}"/></c:calendar-data></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="{start}" end="{end}"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#
    )
}

/// The events of `from`..`to` in the mailboxes' calendars, read now.
pub async fn listed(sources: &[Source], from: NaiveDate, to: NaiveDate) -> Remote {
    let mut remote = Remote::of(sources);
    for source in sources {
        let mut failed = None;
        for calendar in &source.calendars {
            let answer = dav(
                "REPORT",
                &calendar.href,
                &source.username,
                &source.password,
                Some("1"),
                Some(query(from, to)),
                &[],
            )
            .await;
            let answer = match answer {
                Ok(a) if a.status == 207 => a,
                Ok(a) => {
                    failed = Some(refused(a.status).unwrap_or_else(|| format!("answered {}", a.status)));
                    continue;
                }
                Err(why) => {
                    failed = Some(why);
                    break;
                }
            };
            let tree = parse_xml(&answer.body);
            let mut responses = Vec::new();
            tree.all("response", &mut responses);
            for r in responses {
                let (Some(href), Some(data)) =
                    (r.child("href").map(|h| h.text.trim().to_string()), r.text_of("calendar-data"))
                else {
                    continue;
                };
                let href = join(&answer.url, &href);
                for mut event in read_ics(&data) {
                    event["id"] = json!(format!("mail:{}:{href}", source.account));
                    event["calendar"] = json!(source.calendar_id(calendar));
                    if let Some(map) = event.as_object_mut() {
                        map.remove("uid");
                    }
                    remote.events.push(event);
                }
            }
        }
        if let Some(why) = failed {
            remote.failed.push(format!("{}: {why}", source.label));
        }
    }
    remote
}

/// The mailbox calendar `wanted` names (its title, or « title (mailbox) »), if one does.
fn target<'a>(sources: &'a [Source], wanted: &str) -> Option<(&'a Source, &'a DavCalendar)> {
    let wanted = wanted.trim().to_lowercase();
    sources.iter().find_map(|s| {
        s.calendars
            .iter()
            .find(|c| c.title.to_lowercase() == wanted || format!("{} ({})", c.title, s.label).to_lowercase() == wanted)
            .map(|c| (s, c))
    })
}

fn split_id(id: &str) -> Option<(&str, &str)> {
    let rest = id.strip_prefix("mail:")?;
    rest.split_once(':')
}

/// The tools' work on the mailboxes' calendars: an event added to one of them (its calendar named, or no phone copy
/// to ask), or one of their events deleted. None: not theirs (the phone's queue takes it).
pub async fn handle(name: &str, arguments: &Dict, sources: &[Source], phone: bool) -> Option<Dict> {
    if sources.is_empty() {
        return None;
    }
    match name {
        "calendar_add" => {
            let wanted = tk::str_or_empty(arguments.get("calendar"));
            let (source, calendar) = match target(sources, &wanted) {
                Some(found) => found,
                None if !phone => match sources.iter().find_map(|s| s.calendars.first().map(|c| (s, c))) {
                    Some(found) => found,
                    None => return Some(tool_text("no calendar to add to", true)),
                },
                None => return None,
            };
            let event = match crate::calendar::new_event(arguments) {
                Ok(event) => event,
                Err(why) => return Some(tool_text(&why, true)),
            };
            let uid = format!("{}@bipagents", uuid::Uuid::new_v4());
            let Some(ics) = write_ics(&uid, &event, Utc::now()) else {
                return Some(tool_text("start: an ISO date-time or a date", true));
            };
            let url = join(&format!("{}/", calendar.href.trim_end_matches('/')), &format!("{uid}.ics"));
            let put =
                dav("PUT", &url, &source.username, &source.password, None, Some(ics), &[("If-None-Match", "*")]).await;
            Some(match put {
                Ok(a) if (200..300).contains(&a.status) => tool_text(
                    &format!(
                        "Added to « {} » ({}): the person sees it in their calendar.",
                        calendar.title, source.label
                    ),
                    false,
                ),
                Ok(a) => tk::tool_failure(
                    &refused(a.status).unwrap_or_else(|| format!("not added ({})", a.status)),
                    &format!("I couldn't add it to your calendar « {} » just now.", calendar.title),
                ),
                Err(why) => tk::tool_failure(
                    &format!("not added: {why}"),
                    &format!("I couldn't add it to your calendar « {} » just now.", calendar.title),
                ),
            })
        }
        "calendar_update" | "calendar_delete" => {
            let id = tk::strip(&tk::str_or_empty(arguments.get("id"))).to_string();
            let (account, href) = split_id(&id)?;
            let Some(source) = sources.iter().find(|s| s.account == account) else {
                return Some(tool_text("unknown event id (see calendar_list)", true));
            };
            if name == "calendar_update" {
                return Some(tool_text(
                    "an event of a mailbox's calendar is changed in the person's calendar app (tell them), or deleted \
and added again",
                    true,
                ));
            }
            if !source.calendars.iter().any(|c| href.starts_with(c.href.as_str())) {
                return Some(tool_text("unknown event id (see calendar_list)", true));
            }
            let current = match dav("GET", href, &source.username, &source.password, None, None, &[]).await {
                Ok(a) if a.status == 200 => a.body,
                Ok(a) if a.status == 404 => return Some(tool_text("no such event any more", true)),
                Ok(a) => {
                    return Some(tool_text(
                        &refused(a.status).unwrap_or_else(|| format!("answered {}", a.status)),
                        true,
                    ))
                }
                Err(why) => return Some(tool_text(&why, true)),
            };
            if read_ics(&current).iter().any(|e| e.get("recurring").map(py::truthy).unwrap_or(false)) {
                return Some(tool_text(
                    "a repeating event: the person changes or deletes it in their calendar app (tell them)",
                    true,
                ));
            }
            Some(match dav("DELETE", href, &source.username, &source.password, None, None, &[]).await {
                Ok(a) if (200..300).contains(&a.status) => tool_text(&format!("Deleted from {}.", source.label), false),
                Ok(a) => tk::tool_failure(
                    &refused(a.status).unwrap_or_else(|| format!("not deleted ({})", a.status)),
                    "I couldn't remove it from your calendar just now.",
                ),
                Err(why) => tk::tool_failure(
                    &format!("not deleted: {why}"),
                    "I couldn't remove it from your calendar just now.",
                ),
            })
        }
        _ => None,
    }
}

/// `escape` for the tests' own answers.
pub fn xml_escape(text: &str) -> String {
    escape(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_as_servers_write_it() {
        let body = r#"<?xml version="1.0"?><multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
            <response><href>/cal/home/</href><propstat><prop><resourcetype><collection/><C:calendar/></resourcetype>
            <displayname>Perso &amp; famille</displayname><C:supported-calendar-component-set><C:comp name="VEVENT"/>
            </C:supported-calendar-component-set></prop></propstat></response>
            <response><href>/cal/tasks/</href><propstat><prop><resourcetype><collection/><C:calendar/></resourcetype>
            <C:supported-calendar-component-set><C:comp name="VTODO"/></C:supported-calendar-component-set></prop></propstat></response>
            <response><href>/cal/</href><propstat><prop><resourcetype><collection/></resourcetype></prop></propstat></response>
            </multistatus>"#;
        let answer = Answer { status: 207, body: body.into(), url: "https://dav.example/cal/".into() };
        assert_eq!(
            calendars_in(&answer),
            vec![DavCalendar {
                href: "https://dav.example/cal/home/".into(),
                title: "Perso & famille".into(),
                color: None
            }]
        );
        let cdata = parse_xml("<a><b><![CDATA[x<y]]></b></a>");
        assert_eq!(cdata.text_of("b").as_deref(), Some("x<y"));
    }

    #[test]
    fn icalendar_read_and_written() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART;VALUE=DATE:20261023\r\nDTEND;VALUE=DATE:20261026\r\n\
SUMMARY:Salon\\, Paris\r\nDESCRIPTION:Stand B4\\nAllée 2\r\nBEGIN:VALARM\r\nDESCRIPTION:alarm\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:2\r\nDTSTART;TZID=Europe/Paris:20261022T100000\r\nDURATION:PT1H30M\r\nSUMMARY:Dentiste avec un titre \r\n \
très long\r\nRRULE:FREQ=WEEKLY\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let events = read_ics(ics);
        assert_eq!(events[0]["start"], "2026-10-23");
        assert_eq!(events[0]["end"], "2026-10-25");
        assert_eq!(events[0]["title"], "Salon, Paris");
        assert_eq!(events[0]["notes"], "Stand B4\nAllée 2");
        assert_eq!(events[1]["start"], "2026-10-22T10:00:00");
        assert_eq!(events[1]["end"], "2026-10-22T11:30:00");
        assert_eq!(events[1]["title"], "Dentiste avec un titre très long");
        assert_eq!(events[1]["recurring"], true);
        let now = Utc.with_ymd_and_hms(2026, 10, 21, 8, 0, 0).unwrap();
        let written =
            write_ics("u1", &json!({"title": "Congés", "start": "2026-12-21", "end": "2026-12-24"}), now).unwrap();
        assert!(written.contains("DTSTART;VALUE=DATE:20261221\r\nDTEND;VALUE=DATE:20261225\r\n"), "{written}");
        let back = read_ics(&written);
        assert_eq!((back[0]["start"].clone(), back[0]["end"].clone()), (json!("2026-12-21"), json!("2026-12-24")));
        let timed = write_ics(
            "u2",
            &json!({"title": "a;b", "start": "2026-10-30T10:00:00+02:00",
                                            "end": "2026-10-30T11:00:00+02:00"}),
            now,
        )
        .unwrap();
        assert!(timed.contains("DTSTART:20261030T080000Z") && timed.contains("SUMMARY:a\\;b"), "{timed}");
        assert_eq!(duration_minutes("P1DT2H"), Some(1560));
    }

    #[test]
    fn where_to_look() {
        assert_eq!(candidates("alex@icloud.com", "imap.mail.me.com").unwrap(), vec!["https://caldav.icloud.com/"]);
        assert!(candidates("alex@gmail.com", "imap.gmail.com").is_err());
        assert!(candidates("alex@outlook.fr", "outlook.office365.com").is_err());
        assert_eq!(
            candidates("alex@exemple.org", "imap.exemple.org").unwrap(),
            vec!["https://exemple.org/.well-known/caldav", "https://exemple.org/remote.php/dav/"]
        );
        assert!(
            allowed_url("http://127.0.0.1:8080/dav/")
                && !allowed_url("http://dav.example/")
                && allowed_url("https://x.example/")
        );
    }
}
