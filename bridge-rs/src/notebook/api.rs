//! The Carnet for the apps (the bridge key, like the other app routes; mounted under /v1). The private health part
//! (`private=1` / `"private": true`) is the person's: the home leaves it out (it is reached only through its own page,
//! behind the app's lock).
//!
//!     GET    /v1/notebook                      home: sections and counts, « Nouveau » (recent changes), upcoming tasks,
//!                                              the trips within 30 days or under way, the records, the open proposals
//!     GET    /v1/notebook/pages?kind=          a section's pages (kind: person, subject, trip, area, other), with what their card
//!                                              shows (close, next_date, status, facts)
//!     GET    /v1/notebook/search?q=&lang=      the facts that answer, ready to show ({label, answer, page, path, date,
//!                                              when}: tools::answers, the agents' search too), then the pages
//!     GET    /v1/notebook/offline              the copy the phone answers from offline (offline.rs; ETag, 304)
//!     GET    /v1/notebook/known?agent=&lang=   « Ce que tes agents savent de toi » (known.rs)
//!     POST   /v1/notebook/forget/preview       {texts? (≤ 5), page?, private?, card?}   « Oublie ça »: where it is, and a
//!                                              token for 10 minutes, once (forget.rs)
//!     POST   /v1/notebook/forget/confirm       {token}   erased everywhere (never the conversations)
//!     POST   /v1/notebook/forget/cancel        {card}    the card an agent put in the conversation, dismissed
//!     POST   /v1/notebook/ask/{id}             {choice} · {skip} · {never} · {undo}   the question of the day answered
//!                                              (ask.rs: the fact written in the person's name, no agent runs)
//!     POST   /v1/notebook/follow/{id}          {answer: done|tomorrow|drop|never}   a follow-up answered (followups.rs)
//!     GET    /v1/notebook/review?lang=         « Ta semaine dans le Carnet », or 204 (review.rs); POST …/review/done {off?}
//!     POST   /v1/notebook/paper?lang=          {text, image? (base64)}   a paper photographed, read on the phone
//!                                              (papers.rs: no number kept, two reminders); GET …/papers/{id}/photo
//!     GET    /v1/notebook/page?path=           a page: frontmatter apart, « En bref », facts with attribution, links,
//!                                              its `rev` (the app writes over what it read), `emails`, `phones`
//!     POST   /v1/notebook/page                 {kind, title, private?}   a new page, its Markdown written by the bridge
//!                                              (201 {path, rev}; 409 page_exists: the page of that name, the app opens it)
//!     PUT    /v1/notebook/section              {path, section (its id), text, rev, private?}   « Modifier » on one
//!                                              section (409 page_changed: an agent wrote since `rev`; the section as
//!                                              it is now comes with it, the app keeps what the person typed)
//!     DELETE /v1/notebook/page?path=
//!     GET    /v1/notebook/targets?path=&private=&text=   « Déplacer vers… »: the pages proposed first (where such
//!                                              points went before, the page their label names, « Travail » for a work
//!                                              agent's), then every page, each with the section its words point to
//!     POST   /v1/notebook/move                 {path, private?, rev?, texts? | section, to: {path? | title+kind,
//!                                              private?}, to_section?}   points or a whole section moved, their lines
//!                                              kept (moves.rs) → {page, to, move, toast, learnt, undo}
//!     POST   /v1/notebook/move/undo            {move}   « Annuler » → {page}
//!     POST   /v1/notebook/misplaced            {action: move|leave, ids?}   the misplaced points' card answered
//!                                              (misplaced.rs) → {moved, toast, card}
//!     GET    /v1/notebook/history?path=        changes (a page's, or all), the newest first
//!     POST   /v1/notebook/restore              {change}   the page as it was after that change
//!     GET    /v1/notebook/tasks                open and recent tasks
//!     POST   /v1/notebook/tasks                {title, due?, page?, private?}
//!     POST   /v1/notebook/tasks/{id}           {done}
//!     GET    /v1/notebook/dates?days=&all=&standard=   « Dates à venir »: dated facts (close people's by default), not
//!                                              tasks, and the standard dates (dates/: `computed: true`; standard=0: not)
//!     POST   /v1/notebook/close                {path, close}   a person marked close (« proche ») or not
//!     POST   /v1/notebook/icon                 {path, icon}    a subject's or area's icon, the person's choice ("": automatic)
//!     GET    /v1/notebook/proposals            the agents' task proposals, the kinds declined
//!     POST   /v1/notebook/proposals/{id}       {accept, never?}   the person's answer to a proposal (yes: its task added;
//!                                              a split, kind `split`: the page's trips made, trip_split.rs)
//!     GET    /v1/notebook/records              the records (purchases, subscriptions, contracts, papers, bookings), by
//!                                              type, the soonest next date first, their archived ones apart (records.rs)
//!     POST   /v1/notebook/records/archive      {type, id, archived}   a record archived, or brought back (archive.rs)
//!     POST   /v1/notebook/check                {path, item, done}   a trip's thing to pack ticked (trip.rs)
//!     GET    /v1/notebook/documents?private=   the files the agents made or received and their web pages, by agent,
//!                                              with their subjects (the private part's pages only with private=1)
//!     GET    /v1/notebook/settings             {mentions, nightly, language, cap…}
//!     PATCH  /v1/notebook/settings             {mentions?, nightly?, questions?, quiet? {from, to}, language? (a Carnet
//!                                              with pages: its folders and pages moved to the new names, relanguage.rs)}
//!     GET    /v1/notebook/export?health=       a zip of the vault, for Obsidian
//!     POST   /v1/notebook/tidy                 the nightly pass now; GET /v1/notebook/runs its runs and this month's cost
//!     GET    /v1/notebook/fill                 « Remplir mon Carnet »: where it is; POST starts it {agents?}, DELETE stops
//!                                              it (each agent's task in turn, fill.rs)

use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::http::HeaderValue;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{Part, Who};
use crate::app::AppState;
use crate::config::Config;
use crate::tools_knowledge as tk;
use crate::web::{self, check_max_len, ApiResult, FieldError, HttpError, Json, OptionalJson, QueryMap, Validate};
use crate::Dict;

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> ApiResult<T> {
    match tokio::task::spawn_blocking(work).await.map_err(anyhow::Error::from)? {
        Ok(v) => Ok(v),
        Err(err) => match err.downcast_ref::<super::NotebookError>() {
            Some(message) => Err(HttpError::new(400, message.0.clone())),
            None => Err(err.into()),
        },
    }
}

/// The language the app shows the Carnet in (`?lang=`, the app's; apps from 2026-11-02 send it), else the Carnet's
/// own: what the app shows (the « Toi » card, the sections' names, the standard dates) follows the app, the stored
/// language only the pages' text the agents write (2026-11-02: an English app on a place claimed in French showed
/// « Toi »).
fn shown_language(config: &Config, q: &QueryMap) -> String {
    let wanted = q.get("lang").map(|l| crate::souls::agent_language(Some(l)));
    match wanted {
        Some(lang) if super::WORDS.iter().any(|w| w.lang == lang) => lang,
        Some(lang) if !lang.is_empty() => "en".into(), // another language (Dutch…): English, like the other names
        _ => super::language(config),
    }
}

fn part_of(q: &QueryMap) -> Part {
    if matches!(q.get("private"), Some("1" | "true" | "yes")) {
        Part::Health
    } else {
        Part::Main
    }
}

fn change_view(config: &Config, entry: &Dict, shown: &str) -> Value {
    let mut out = entry.clone();
    // Always strings for the apps: the person's own changes and the nightly tidy's have no agent (null in the log).
    for key in ["id", "at", "by", "name", "agent", "part", "path", "title", "action", "summary"] {
        let text = tk::str_or_empty(entry.get(key));
        out.insert(key.into(), json!(text));
    }
    // the person's own page by its name in the app's language (« You », not « Toi »)
    let path = tk::str_or_empty(entry.get("path"));
    if super::WORDS.iter().any(|w| path == w.self_path()) {
        out.insert("title".into(), json!(super::words(shown).me));
    }
    let by = tk::str_or_empty(entry.get("by"));
    out.insert("by_you".into(), json!(by == "you"));
    out.insert("nightly".into(), json!(by == "nightly"));
    let agent = tk::str_or_empty(entry.get("agent"));
    out.insert("agent_color".into(), json!(config.agent(Some(&agent)).and_then(|a| a.color.clone())));
    Value::Object(out)
}

fn section_of(w: &super::Words, page: &super::Page, path: &str) -> String {
    let kind = page.kind();
    match kind.as_str() {
        "person" | "subject" | "area" | "self" | "tasks" | "records" | "trip" => kind,
        _ if path == w.inbox_path() => "inbox".into(),
        _ => "other".into(),
    }
}

async fn home(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_language(&config, &q);
    let view = blocking(move || {
        let lang = super::language(&config);
        // the paths in the Carnet's language (its folders), the names shown in the app's
        let stored = super::words(&lang);
        let w = super::words(&shown);
        let pages = super::all_pages(&config, Part::Main);
        // the person's own page first (« Toi »: one page, its own card), then the sections
        let mine = pages.iter().find(|(_, p)| p.kind() == "self").map(|(path, _)| path.clone());
        let mut sections = vec![json!({"kind": "self", "title": w.me, "count": usize::from(mine.is_some()),
                                       "path": mine.unwrap_or_else(|| stored.self_path())})];
        for (kind, folder) in [("person", w.people), ("subject", w.subjects), ("area", w.areas)] {
            let count = pages.iter().filter(|(_, p)| p.kind() == kind).count();
            sections.push(json!({"kind": kind, "title": folder, "count": count}));
            // the trips apart from the subjects (2026-10-23), the past ones in the archives
            if kind == "subject" {
                let trips: Vec<&String> =
                    pages.iter().filter(|(_, p)| p.kind() == "trip").map(|(path, _)| path).collect();
                let past = trips.iter().filter(|path| stored.archived(path)).count();
                sections.push(json!({"kind": "trip", "title": w.trips, "count": trips.len() - past, "archived": past}));
            }
        }
        // the open tasks that count: archived ones left out, the person's own first
        let open = super::open_tasks(&config, Part::Main);
        sections.push(json!({"kind": "tasks", "title": w.tasks, "count": open.len()}));
        let documents = super::documents(&config, Part::Main);
        sections.push(json!({"kind": "documents", "title": w.documents, "count": documents.len()}));
        let (_, inbox) = super::inbox(&config);
        let recent: Vec<Value> =
            super::recent_changes(&config, Part::Main, 20).iter().map(|e| change_view(&config, e, &shown)).collect();
        let mut upcoming: Vec<&super::Task> = open.iter().filter(|t| !t.due.is_empty()).collect();
        upcoming.sort_by(|a, b| a.due.cmp(&b.due));
        let upcoming: Vec<Value> = upcoming.iter().take(10).map(|t| t.view()).collect();
        // « Dates à venir »: close people's dated facts in the next 30 days (facts, not tasks)
        let today = chrono::Local::now().date_naive();
        let mut dates = super::upcoming_dates(&config, 30, false, today);
        // and the standard dates (holidays, school holidays, clock changes): computed for the person, not saved
        dates.extend(crate::dates::notebook_entries(&config, 30, Some(&shown)));
        dates.sort_by_key(|d| d.get("in_days").and_then(Value::as_i64).unwrap_or(0));
        // « Mode voyage »: the trips within 30 days or under way; the records by type; the proposals to answer
        let trips = super::trip::upcoming(&config, today);
        let records = super::records::home(&config, today);
        let proposals = super::open_proposals(&config);
        let archives = super::archive::counts(&config);
        Ok(json!({"language": lang, "shown_language": shown, "sections": sections, "recent": recent, "upcoming": upcoming, "archives": archives,
                  "dates": dates.into_iter().take(10).collect::<Vec<_>>(),
                  "inbox": inbox.len(), "settings": settings_view(&config), "trips": trips, "records": records,
                  "proposals": proposals, "known_tile": super::known::tile(&config, &shown, crate::py::time_time()),
                  "review": super::review::view(&config, &shown, crate::py::time_time())?.map(|r| r["card"].clone()),
                  "misplaced": super::misplaced::card(&config, &shown)}))
    })
    .await?;
    Ok(web::ok(view))
}

fn page_summary(path: &str, part: Part, page: &super::Page, brief: &str) -> Value {
    let archived = super::WORDS.iter().any(|w| w.archived(path));
    json!({"path": path, "part": part.as_str(), "title": page.title, "kind": page.kind(), "aliases": page.aliases(),
           "archived": archived,
           "contact": if page.contact().is_empty() { Value::Null } else { json!(page.contact()) },
           "updated": page.front.get("updated").cloned().unwrap_or(Value::Null), "icon": page.icon(),
           "summary": super::summary_of(&page.body, brief)})
}

async fn list_pages(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let part = part_of(&q);
    let kind = q.get("kind").unwrap_or("").to_string();
    let view = blocking(move || {
        let w = super::words(&super::language(&config));
        // what a section's card shows: a person's next date and whether they are close, a subject's « Où on en
        // est », an area's number of facts
        let dates = match part {
            Part::Main => super::upcoming_dates(&config, 366, true, chrono::Local::now().date_naive()),
            Part::Health => Vec::new(),
        };
        let mut listed = super::all_pages(&config, part);
        listed.retain(|(path, p)| kind.is_empty() || section_of(w, p, path) == kind);
        // the trips: under way and upcoming first (the soonest first), then those without dates, then the past ones
        // (the latest first)
        let today = chrono::Local::now().date_naive();
        let order = |path: &String, p: &super::Page| {
            if p.kind() != "trip" {
                return (0, 0);
            }
            let (start, end) = super::trip::shown_dates(p);
            let past = w.archived(path) || end.is_some_and(|e| e < today);
            match (past, start) {
                (false, Some(s)) => (1, (s - today).num_days()),
                (false, None) => (2, 0),
                (true, s) => (3, s.map(|s| (today - s).num_days()).unwrap_or(i64::MAX)),
            }
        };
        if kind == "trip" {
            listed.sort_by_key(|(path, p)| order(path, p));
        }
        let pages: Vec<Value> = listed
            .iter()
            .map(|(path, p)| {
                let mut view = page_summary(path, part, p, w.brief);
                let shaped = super::Page { body: super::in_outline(&config, p), ..p.clone() };
                let sections = shaped.sections();
                let status = super::template::title(&p.kind(), "status", &super::language(&config));
                view["close"] = json!(p.close());
                view["next_date"] = dates
                    .iter()
                    .find(|d| d.get("path").and_then(Value::as_str) == Some(path.as_str()))
                    .map(|d| json!({"date": d["date"], "in_days": d["in_days"], "fact": d["fact"], "yearly": d["yearly"]}))
                    .unwrap_or(Value::Null);
                // where it stands now: the latest line (the lines are added at the end, a day's last one after)
                view["status"] = sections
                    .iter()
                    .find(|(name, _)| p.kind() == "subject" && *name == status)
                    .and_then(|(_, facts)| facts.iter().enumerate().max_by_key(|(i, f)| (f.date.clone(), *i)))
                    .map(|(_, f)| json!(f.text))
                    .unwrap_or(Value::Null);
                view["facts"] = json!(sections
                    .iter()
                    .filter(|(name, _)| super::fold(name) != super::fold(w.brief))
                    .map(|(_, facts)| facts.len())
                    .sum::<usize>());
                if p.kind() == "trip" {
                    let (start, end) = super::trip::shown_dates(p);
                    view["start"] = json!(start.map(|d| d.format("%Y-%m-%d").to_string()));
                    view["end"] = json!(end.map(|d| d.format("%Y-%m-%d").to_string()));
                    view["destination"] = p.front.get("destination").cloned().unwrap_or(Value::Null);
                }
                view
            })
            .collect();
        Ok(json!({"pages": pages}))
    })
    .await?;
    Ok(web::ok(view))
}

async fn search(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let part = part_of(&q);
    let shown = shown_language(&config, &q);
    let words = web::query_str(&q, "q", true, Some(200))?.unwrap_or_default();
    let view = blocking(move || {
        let w = super::words(&super::language(&config));
        // the answers by the one rule the agents' search and the offline copy follow (offline.rs): split by the
        // bridge, the apps show them as they come
        let answers: Vec<Value> = super::tools::answers(&config, &[part], &words)
            .into_iter()
            .map(|a| {
                json!({"label": a.label, "answer": a.answer, "page": a.page, "path": a.path, "date": a.date,
                       "when": super::offline::when(&a.date, &shown), "part": a.part.as_str()})
            })
            .collect();
        let found = super::search(&config, &[part], &words, "");
        let pages: Vec<Value> = found
            .iter()
            .take(50)
            .map(|(part, path, page, lines)| {
                let mut v = page_summary(path, *part, page, w.brief);
                v["lines"] = json!(lines);
                v
            })
            .collect();
        let (wanted, _) = super::lexicon::query(&words);
        let tasks: Vec<Value> = super::tasks(&config, part)
            .iter()
            .filter(|t| !wanted.is_empty() && wanted.iter().all(|x| super::fold(&t.title).contains(x.as_str())))
            .map(|t| t.view())
            .collect();
        Ok(json!({"answers": answers, "pages": pages, "tasks": tasks}))
    })
    .await?;
    Ok(web::ok(view))
}

/// The copy the phone answers from offline (contract.md §6): 304 when the app has it already.
async fn offline(State(state): State<AppState>, q: QueryMap, headers: axum::http::HeaderMap) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_language(&config, &q);
    let (body, etag) = blocking(move || Ok(super::offline::snapshot(&config, &shown, crate::py::time_time()))).await?;
    let unchanged = headers.get("if-none-match").and_then(|v| v.to_str().ok()).is_some_and(|v| v.trim() == etag);
    let mut response = if unchanged {
        let mut r = Response::new(Body::empty());
        *r.status_mut() = axum::http::StatusCode::NOT_MODIFIED;
        r
    } else {
        web::ok(body)
    };
    if let Ok(v) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert("etag", v);
    }
    response.headers_mut().insert("cache-control", HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn known(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_language(&config, &q);
    let agent = web::query_str(&q, "agent", false, Some(80))?.unwrap_or_default();
    let view = blocking(move || Ok(super::known::view(&config, &agent, &shown, crate::py::time_time()))).await?;
    Ok(web::ok(view))
}

#[derive(Debug, Deserialize)]
pub struct ForgetPreview {
    #[serde(default)]
    pub texts: Vec<String>,
    #[serde(default)]
    pub page: Option<String>,
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub private: bool,
    /// The card an agent put in the conversation (`bip-forget-…`): its request, confirmed here.
    #[serde(default)]
    pub card: Option<String>,
}

impl Validate for ForgetPreview {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        web::check_max_items(errors, "texts", Some(&self.texts), super::forget::TEXTS_MAX);
        for text in &self.texts {
            check_max_len(errors, "texts", Some(text), 500);
        }
        check_max_len(errors, "page", self.page.as_deref(), 200);
        check_max_len(errors, "card", self.card.as_deref(), 40);
    }
}

fn refused(refusal: super::forget::Refused, lang: &str) -> HttpError {
    use super::forget::Refused;
    match refusal {
        Refused::PageGone => carnet_error(404, "page_gone", lang, json!({})),
        Refused::Several(names) => {
            let text = match lang {
                "fr" => format!("Plusieurs pages : {}.", names.join(", ")),
                "de" => format!("Mehrere Seiten: {}.", names.join(", ")),
                "es" => format!("Varias páginas: {}.", names.join(", ")),
                _ => format!("Several pages: {}.", names.join(", ")),
            };
            HttpError::json(409, json!({"code": "several_pages", "text": text, "pages": names}))
        }
        Refused::Nothing => {
            HttpError::json(404, json!({"code": "nothing_found", "text": super::forget::texts(lang).nothing}))
        }
    }
}

/// « Oublie ça »: what would be erased, and the token that erases it (10 minutes, once). The only route that gives a
/// token: an agent's `notebook_write {forget}` gets a card, never one.
async fn forget_preview(State(state): State<AppState>, q: QueryMap, Json(body): Json<ForgetPreview>) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_language(&config, &q);
    let person = state.households.name();
    let lang = shown.clone();
    let out = blocking(move || {
        let request = match body.card.as_deref() {
            Some(card) => match super::forget::card_request(&config, card) {
                Some(r) => r,
                None => return Ok(Err(super::forget::Refused::Nothing)),
            },
            None => super::forget::Request { texts: body.texts, page: body.page, private: body.private },
        };
        Ok(super::forget::preview(&config, &request, &lang, &person, body.card))
    })
    .await?;
    match out {
        Ok(v) => Ok(web::ok(v)),
        Err(r) => Err(refused(r, &shown)),
    }
}

#[derive(Debug, Deserialize)]
pub struct ForgetConfirm {
    pub token: String,
}

impl Validate for ForgetConfirm {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "token", Some(&self.token), 64);
    }
}

async fn forget_confirm(State(state): State<AppState>, q: QueryMap, Json(body): Json<ForgetConfirm>) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_language(&config, &q);
    let person = state.households.name();
    let done = blocking(move || super::forget::confirm(&config, body.token.trim(), &person)).await?;
    match done {
        Some(text) => Ok(web::ok(json!({"done": true, "text": text}))),
        None => Err(carnet_error(410, "token_expired", &shown, json!({}))),
    }
}

#[derive(Debug, Deserialize)]
pub struct ForgetCancel {
    pub card: String,
}

impl Validate for ForgetCancel {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "card", Some(&self.card), 40);
    }
}

async fn forget_cancel(State(state): State<AppState>, Json(body): Json<ForgetCancel>) -> ApiResult<Response> {
    let config = state.config();
    let found = blocking(move || Ok(super::forget::set_card(&config, &body.card, "cancelled")?)).await?;
    if found {
        Ok(web::no_content())
    } else {
        Err(HttpError::not_found())
    }
}

// -- what the Carnet says to the person (ask.rs, followups.rs, review.rs, papers.rs) --------------------------------

#[derive(Debug, Deserialize)]
pub struct AskAnswer {
    #[serde(default)]
    pub choice: Option<String>,
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub skip: bool,
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub never: bool,
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub undo: bool,
}

impl Validate for AskAnswer {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "choice", self.choice.as_deref(), 80);
    }
}

fn card_refused(r: super::ask::Refused) -> HttpError {
    match r {
        super::ask::Refused::Missing => HttpError::not_found(),
        super::ask::Refused::Gone(card) => HttpError::json(410, json!({"code": "card_gone", "card": card})),
    }
}

/// The question of the day answered: the fact written in the person's name (no agent runs, nothing pushed).
async fn ask_answer(State(state): State<AppState>, UrlPath(id): UrlPath<String>, q: QueryMap, Json(body): Json<AskAnswer>) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_language(&config, &q);
    use super::ask::Answer;
    let answer = if body.undo {
        Answer::Undo
    } else if body.never {
        Answer::Never
    } else if body.skip {
        Answer::Skip
    } else {
        match body.choice.filter(|c| !c.trim().is_empty()) {
            Some(c) => Answer::Choice(c),
            None => return Err(HttpError::new(422, "choice, skip, never or undo")),
        }
    };
    let out = blocking(move || super::ask::answer(&config, &id, answer, &shown, crate::py::time_time())).await?;
    out.map(web::ok).map_err(card_refused)
}

#[derive(Debug, Deserialize)]
pub struct FollowAnswer {
    pub answer: String,
}

impl Validate for FollowAnswer {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if !super::followups::ANSWERS.contains(&self.answer.as_str()) {
            errors.push(FieldError::new("value_error", web::body_loc("answer"), "Value error, done, tomorrow, drop or never", json!(self.answer)));
        }
    }
}

/// A follow-up answered: « C'est fait », « Demain », « Laisse tomber », « Ne plus me relancer ».
async fn follow_answer(State(state): State<AppState>, UrlPath(id): UrlPath<String>, q: QueryMap, Json(body): Json<FollowAnswer>) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_language(&config, &q);
    let out = blocking(move || super::followups::answer(&config, &id, &body.answer, &shown)).await?;
    out.map(web::ok).map_err(card_refused)
}

/// « Ta semaine dans le Carnet », when it is due (else 204).
async fn review(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_language(&config, &q);
    let view = blocking(move || super::review::view(&config, &shown, crate::py::time_time())).await?;
    Ok(view.map(web::ok).unwrap_or_else(web::no_content))
}

#[derive(Debug, Default, Deserialize)]
pub struct ReviewDone {
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub off: bool,
}

impl Validate for ReviewDone {}

async fn review_done(State(state): State<AppState>, OptionalJson(body): OptionalJson<ReviewDone>) -> ApiResult<Response> {
    let config = state.config();
    let off = body.map(|b| b.off).unwrap_or(false);
    blocking(move || Ok(super::review::done(&config, off, crate::py::time_time())?)).await?;
    Ok(web::no_content())
}

#[derive(Debug, Deserialize)]
pub struct Paper {
    pub text: String,
    /// The photo, base64 (JPEG or PNG): kept apart, never given to the model.
    #[serde(default)]
    pub image: Option<String>,
}

impl Validate for Paper {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "text", Some(&self.text), super::papers::TEXT_MAX * 2);
    }
}

/// A paper photographed: the text the phone read filed (no number kept), its photo kept apart, two reminders.
async fn paper(State(state): State<AppState>, q: QueryMap, Json(body): Json<Paper>) -> ApiResult<Response> {
    use base64::Engine;
    let config = state.config();
    let shown = shown_language(&config, &q);
    let image = match body.image.as_deref().map(str::trim).filter(|i| !i.is_empty()) {
        Some(b64) => {
            let b64 = b64.split_once("base64,").map(|(_, b)| b).unwrap_or(b64);
            match base64::engine::general_purpose::STANDARD.decode(b64) {
                Ok(bytes) if bytes.len() <= super::papers::PHOTO_MAX => Some(bytes),
                _ => return Err(HttpError::new(422, "image: a JPEG or a PNG, in base64")),
            }
        }
        None => None,
    };
    let lang = shown.clone();
    let out = blocking(move || {
        super::papers::file(&config, &body.text, image.as_deref(), &lang, chrono::Local::now().date_naive())
    })
    .await?;
    match out {
        Ok(view) => Ok(web::ok(view)),
        Err(_) => Err(carnet_error(422, "paper_unreadable", &shown, json!({}))),
    }
}

/// A paper's photo (the app shows it after Face ID or the fingerprint).
async fn paper_photo(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let found = blocking(move || Ok(super::papers::photo(&config, &id))).await?;
    let Some((bytes, kind)) = found else { return Err(HttpError::not_found()) };
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert("content-type", HeaderValue::from_static(kind));
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
    Ok(response)
}

/// A page for the app: its frontmatter apart, its « En bref », its sections with each fact's attribution, its
/// links resolved to pages (or not found), the pages that link to it.
pub fn page_view(config: &Config, part: Part, path: &str) -> anyhow::Result<Value> {
    page_view_in(config, part, path, &super::language(config))
}

/// [`page_view`] with the sections' names, the outline's and the person's own page's title in `shown` (the app's
/// language); the page's text stays as it is written.
pub fn page_view_in(config: &Config, part: Part, path: &str, shown: &str) -> anyhow::Result<Value> {
    super::checked(path)?;
    let Some(text) = super::read_page_text(config, part, path) else {
        return Err(super::NotebookError("no such page".into()).into());
    };
    let w = super::words(&super::language(config));
    let stem = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut page = super::Page::parse(&text, &stem);
    // shown in its outline even before the re-filing reached it (on disk at the next write or the re-filing)
    page.body = super::in_outline(config, &page);
    let kind = page.kind();
    let lang = super::language(config);
    let all = super::all_pages(config, part);
    let resolve = |name: &str| -> Value {
        // a link to a document of an agent's archive (`[[archive:<ref>|voir le mail]]`, archives/): opened by the app
        if let Some(reference) = name.strip_prefix("archive:") {
            return json!({"title": name, "path": null, "archive": reference.trim()});
        }
        match all.iter().find(|(p, pg)| {
            pg.named(name) || std::path::Path::new(p).file_stem().map(|s| s.to_string_lossy() == name).unwrap_or(false)
        }) {
            Some((p, pg)) => json!({"title": name, "path": p, "kind": pg.kind()}),
            None => json!({"title": name, "path": null}),
        }
    };
    let sections: Vec<Value> = page
        .sections()
        .into_iter()
        .filter(|(name, _)| super::fold(name) != super::fold(w.brief))
        .map(|(name, facts)| {
            let facts: Vec<Value> = facts
                .iter()
                .map(|f| {
                    let links: Vec<Value> = super::links_in(&f.text).iter().map(|l| resolve(l)).collect();
                    let (label, answer) = super::lexicon::split_label(&f.text).unwrap_or_default();
                    json!({"text": f.text, "label": label, "answer": answer, "when": super::offline::when(&f.date, shown),
                           "fresh": fresh(&f.date), "noted": noted(&f.who, &f.date, shown),
                           "by": if f.who.is_empty() { Value::Null } else { json!(f.who) },
                           "date": if f.date.is_empty() { Value::Null } else { json!(f.date) }, "item": f.item, "links": links})
                })
                .collect();
            let id = super::template::resolve(&kind, &name)
                .or_else(|| (super::fold(&name) == super::fold(&super::template::title(&kind, "other", &lang))).then_some("other"));
            let title = match id {
                Some(id) if super::template::outline(&kind).iter().any(|s| s.id == id) => {
                    super::template::title(&kind, id, shown)
                }
                _ => name,
            };
            json!({"id": id, "title": title, "facts": facts})
        })
        .collect();
    // the kind's outline, for « Modifier » on a section and adding to an empty one
    let outline: Vec<Value> = super::template::outline(&kind)
        .iter()
        .map(|s| json!({"id": s.id, "title": super::template::title(&kind, s.id, shown)}))
        .collect();
    let links: Vec<Value> =
        super::links_in(&page.body).iter().filter(|l| !l.starts_with("archive:")).map(|l| resolve(l)).collect();
    let mut names: Vec<String> = vec![page.title.clone()];
    names.extend(page.aliases());
    let backlinks: Vec<Value> = all
        .iter()
        .filter(|(p, _)| p != path)
        .filter(|(_, pg)| {
            // a trip split from it points back to it (trip_split.rs: no line per trip on the page)
            let from = super::fold(&crate::tools_knowledge::str_or_empty(pg.front.get("split_from")));
            super::links_in(&pg.body).iter().any(|l| names.iter().any(|n| super::fold(n) == super::fold(l)))
                || (pg.kind() == "trip" && !from.is_empty() && names.iter().any(|n| super::fold(n) == from))
        })
        .map(|(p, pg)| json!({"title": pg.title, "path": p, "kind": pg.kind()}))
        .collect();
    let tasks: Vec<Value> = super::tasks(config, part)
        .iter()
        .filter(|t| t.links.iter().any(|l| names.iter().any(|n| super::fold(n) == super::fold(l))))
        .map(|t| t.view())
        .collect();
    // the web pages the agents made about it (linked to it by `create_page`), shown in its « Documents »
    let documents: Vec<Value> = super::documents(config, part)
        .into_iter()
        .filter(|d| d.get("linked").and_then(|l| l.get("path")).and_then(Value::as_str) == Some(path))
        .collect();
    // a trip's layout, a records page's table (null for the other pages)
    let trip = if kind == "trip" { super::trip::view(config, part, path, &page) } else { Value::Null };
    let records = super::records::page_view(config, &page, chrono::Local::now().date_naive()).unwrap_or(Value::Null);
    // a page holding several journeys: its split proposal, answered on the page too
    let split = if part == Part::Main { super::trip_split::open_for(config, path, &page) } else { None };
    let title = if kind == "self" { super::words(shown).me.to_string() } else { page.title.clone() };
    Ok(json!({"path": path, "part": part.as_str(), "title": title, "kind": page.kind(), "trip": trip,
              "records": records, "split": split, "archived": w.archived(path),
              "frontmatter": page.front, "aliases": page.aliases(),
              "contact": if page.contact().is_empty() { Value::Null } else { json!(page.contact()) },
              "updated": page.front.get("updated").cloned().unwrap_or(Value::Null),
              "summary": super::summary_of(&page.body, w.brief), "sections": sections, "outline": outline, "links": links,
              "backlinks": backlinks, "tasks": tasks, "documents": documents, "id": page.id(), "close": page.close(),
              "icon": page.icon(), "shared_with": super::shares_of(config, part, path, &page),
              "emails": page.emails(), "phones": page.phones(), "rev": super::rev(&text), "text": text}))
}

async fn read_page(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let part = part_of(&q);
    let path = web::query_str(&q, "path", true, Some(300))?.unwrap_or_default();
    let shown = shown_language(&config, &q);
    let view = tokio::task::spawn_blocking(move || page_view_in(&config, part, &path, &shown))
        .await
        .map_err(anyhow::Error::from)?;
    match view {
        Ok(v) => Ok(web::ok(v)),
        Err(err) => match err.downcast_ref::<super::NotebookError>() {
            Some(e) => Err(HttpError::new(404, e.0.clone())),
            None => Err(err.into()),
        },
    }
}

#[derive(Debug, Deserialize)]
pub struct NewPage {
    pub kind: String,
    pub title: String,
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub private: bool,
}

impl Validate for NewPage {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "kind", Some(&self.kind), 20);
        check_max_len(errors, "title", Some(&self.title), 200);
    }
}

/// The texts of the Carnet's errors for the app (contract.md §5), in the app's language.
fn error_text(code: &str, lang: &str) -> &'static str {
    match (code, lang) {
        ("page_changed", "fr") => "Un agent vient d'écrire ici. Vérifie avant d'enregistrer.",
        ("page_changed", "de") => "Ein Agent hat hier gerade geschrieben. Prüfe es vor dem Speichern.",
        ("page_changed", "es") => "Un agente acaba de escribir aquí. Revísalo antes de guardar.",
        ("page_changed", _) => "An agent just wrote here. Check before saving.",
        ("page_exists", "fr") => "Cette page existe déjà.",
        ("page_exists", "de") => "Diese Seite gibt es schon.",
        ("page_exists", "es") => "Esta página ya existe.",
        ("page_exists", _) => "This page already exists.",
        ("number_refused", "fr") => "Les numéros ne sont jamais gardés.",
        ("number_refused", "de") => "Nummern werden nie gespeichert.",
        ("number_refused", "es") => "Los números nunca se guardan.",
        ("number_refused", _) => "Numbers are never kept.",
        ("token_expired", "fr") => "C'est trop ancien. Recommence.",
        ("token_expired", "de") => "Das ist zu alt. Fang neu an.",
        ("token_expired", "es") => "Es demasiado antiguo. Empieza de nuevo.",
        ("token_expired", _) => "That's too old. Start again.",
        ("page_gone", "fr") => "Page oubliée",
        ("page_gone", "de") => "Vergessene Seite",
        ("page_gone", "es") => "Página olvidada",
        ("page_gone", _) => "Forgotten page",
        ("paper_unreadable", "fr") => "Je n'ai pas pu lire ce papier. Corrige à la main.",
        ("paper_unreadable", "de") => "Ich konnte dieses Dokument nicht lesen. Korrigiere es von Hand.",
        ("paper_unreadable", "es") => "No pude leer este documento. Corrígelo a mano.",
        ("paper_unreadable", _) => "I couldn't read this paper. Fix it by hand.",
        ("health_stays", "fr") => "Les infos de santé restent dans Santé.",
        ("health_stays", "de") => "Gesundheitsangaben bleiben in Gesundheit.",
        ("health_stays", "es") => "La información de salud se queda en Salud.",
        ("health_stays", _) => "Health information stays in Health.",
        ("nothing_to_move", "fr") => "Ce point n'est plus là.",
        ("nothing_to_move", "de") => "Dieser Punkt ist nicht mehr da.",
        ("nothing_to_move", "es") => "Este punto ya no está.",
        ("nothing_to_move", _) => "This point is no longer there.",
        ("no_such_page", "fr") => "Cette page n'existe plus.",
        ("no_such_page", "de") => "Diese Seite gibt es nicht mehr.",
        ("no_such_page", "es") => "Esta página ya no existe.",
        ("no_such_page", _) => "This page no longer exists.",
        ("move_gone", "fr") => "Ce déplacement ne peut plus être annulé.",
        ("move_gone", "de") => "Das lässt sich nicht mehr rückgängig machen.",
        ("move_gone", "es") => "Este movimiento ya no se puede deshacer.",
        ("move_gone", _) => "This move can no longer be undone.",
        _ => "",
    }
}

fn carnet_error(status: u16, code: &str, lang: &str, extra: Value) -> HttpError {
    let mut detail = json!({"code": code, "text": error_text(code, lang)});
    if let (Value::Object(d), Value::Object(e)) = (&mut detail, extra) {
        d.extend(e);
    }
    HttpError::json(status, json!(detail))
}

/// A new page from the app: the bridge writes its Markdown (the apps never do).
async fn new_page(State(state): State<AppState>, q: QueryMap, Json(body): Json<NewPage>) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_language(&config, &q);
    let part = if body.private { Part::Health } else { Part::Main };
    let made = blocking(move || {
        let made = super::new_page(&config, part, body.kind.trim(), &body.title, &Who::you(&config))?;
        Ok(made.map(|path| {
            let rev = super::read_page_text(&config, part, &path).map(|t| super::rev(&t)).unwrap_or_default();
            (path, rev)
        }))
    })
    .await?;
    match made {
        Ok((path, rev)) => Ok(web::json_response(201, &json!({"path": path, "rev": rev}))),
        Err(path) => Err(carnet_error(409, "page_exists", &shown, json!({"path": path}))),
    }
}

#[derive(Debug, Deserialize)]
pub struct SectionEdit {
    pub path: String,
    /// The section's id in the page's outline (`dates`, `decisions`…).
    pub section: String,
    /// Its new lines (« - … »; empty: the section is emptied). A line unchanged keeps its attribution.
    pub text: String,
    /// The page's revision the app read (`GET page`).
    pub rev: String,
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub private: bool,
}

impl Validate for SectionEdit {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "path", Some(&self.path), 300);
        check_max_len(errors, "section", Some(&self.section), 80);
        check_max_len(errors, "text", Some(&self.text), 50_000);
        check_max_len(errors, "rev", Some(&self.rev), 16);
    }
}

/// The person's « Modifier » on one section of a page, over the revision they read: the page as it is now, its new
/// `rev` and the change's id. An agent wrote since: 409 `page_changed` with the section as it is now.
async fn write_section(State(state): State<AppState>, q: QueryMap, Json(body): Json<SectionEdit>) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_language(&config, &q);
    let part = if body.private { Part::Health } else { Part::Main };
    if super::lexicon::id_number(&body.text) {
        return Err(carnet_error(422, "number_refused", &shown, json!({})));
    }
    let lang = shown.clone();
    let done = tokio::task::spawn_blocking(move || -> anyhow::Result<Result<Value, (String, Value)>> {
        super::checked(&body.path)?;
        let who = Who::you(&config);
        let edited =
            super::edit(&config, &[part], &body.path, "", "", &body.section, Some(&body.text), "", Some(&body.rev), &who);
        match edited {
            Ok((written, _)) => {
                let mut view = page_view_in(&config, part, &written.path, &lang)?;
                let change = super::history(&config, Some(part), Some(&written.path), 1);
                view["change_id"] = json!(change.first().map(|c| tk::str_or_empty(c.get("id"))));
                Ok(Ok(view))
            }
            Err(err) => match err.downcast_ref::<super::PageChanged>() {
                Some(changed) => {
                    let view = page_view_in(&config, part, &body.path, &lang)?;
                    let section = view["sections"]
                        .as_array()
                        .and_then(|list| list.iter().find(|s| s["id"] == json!(body.section)).cloned())
                        .unwrap_or_else(|| json!({"id": body.section, "facts": []}));
                    Ok(Err((changed.0.clone(), section)))
                }
                None => Err(err),
            },
        }
    })
    .await
    .map_err(anyhow::Error::from)?;
    match done {
        Ok(Ok(view)) => Ok(web::ok(view)),
        Ok(Err((rev, section))) => {
            Err(carnet_error(409, "page_changed", &shown, json!({"rev": rev, "section": section})))
        }
        Err(err) => match err.downcast_ref::<super::NotebookError>() {
            Some(message) => Err(HttpError::new(400, message.0.clone())),
            None => Err(err.into()),
        },
    }
}

async fn delete_page(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let part = part_of(&q);
    let path = web::query_str(&q, "path", true, Some(300))?.unwrap_or_default();
    let entry = blocking(move || super::delete_page(&config, part, &path, &Who::you(&config))).await?;
    Ok(web::ok(json!({"change": entry})))
}

async fn history(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_language(&config, &q);
    let part = part_of(&q);
    let path = q.get("path").map(str::to_string);
    let view = blocking(move || {
        // a page's own history keeps everything; all the changes (« Nouveau ») only the pages still there
        let entries = match path.as_deref() {
            Some(p) => super::history(&config, Some(part), Some(p), 200),
            None => super::recent_changes(&config, part, 200),
        };
        let changes: Vec<Value> = entries.iter().map(|e| change_view(&config, e, &shown)).collect();
        Ok(json!({"changes": changes}))
    })
    .await?;
    Ok(web::ok(view))
}

#[derive(Debug, Deserialize)]
pub struct Restore {
    pub change: String,
}

impl Validate for Restore {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "change", Some(&self.change), 40);
    }
}

async fn restore(State(state): State<AppState>, Json(body): Json<Restore>) -> ApiResult<Response> {
    let config = state.config();
    let entry = blocking(move || super::restore(&config, &body.change, &Who::you(&config))).await?;
    Ok(web::ok(json!({"change": entry})))
}

async fn list_tasks(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let part = part_of(&q);
    let view = blocking(move || {
        let all = super::tasks(&config, part);
        let open: Vec<Value> = super::open_tasks(&config, part).iter().map(|t| t.view()).collect();
        let done: Vec<Value> = all.iter().rev().filter(|t| t.done).take(50).map(|t| t.view()).collect();
        let archived: Vec<Value> = all.iter().filter(|t| t.archived && !t.done).map(|t| t.view()).collect();
        Ok(json!({"open": open, "done": done, "archived": archived}))
    })
    .await?;
    Ok(web::ok(view))
}

#[derive(Debug, Deserialize)]
pub struct NewTask {
    pub title: String,
    #[serde(default)]
    pub due: Option<String>,
    #[serde(default)]
    pub page: Option<String>,
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub private: bool,
}

impl Validate for NewTask {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "title", Some(&self.title), 300);
        check_max_len(errors, "due", self.due.as_deref(), 40);
        check_max_len(errors, "page", self.page.as_deref(), 200);
    }
}

async fn add_task(State(state): State<AppState>, Json(body): Json<NewTask>) -> ApiResult<Response> {
    let config = state.config();
    let part = if body.private { Part::Health } else { Part::Main };
    let push = state.push.clone();
    let added = blocking(move || {
        super::add_task(
            &config,
            part,
            &body.title,
            body.due.as_deref().unwrap_or(""),
            body.page.as_deref().unwrap_or(""),
            "asked",
            "",
            &Who::you(&config),
        )
    })
    .await?;
    if added.reminder {
        wake(push, &state.config());
    }
    Ok(web::ok(json!({"task": added.task.view(), "reminder": added.reminder, "existing": added.existing})))
}

/// The phones woken to apply the reminders' queue (a silent push), as for an agent's reminders_add.
fn wake(push: std::sync::Arc<crate::push::PushService>, config: &Config) {
    let agents = crate::reminders::read(config).get("agents").cloned().unwrap_or(json!([]));
    let Some(agent) = agents.get(0).and_then(Value::as_str).map(str::to_string) else { return };
    tokio::spawn(async move {
        push.notify_silent(&agent, "reminders", Dict::new()).await;
    });
}

#[derive(Debug, Deserialize)]
pub struct TaskDone {
    #[serde(deserialize_with = "web::lax::bool")]
    pub done: bool,
}

impl Validate for TaskDone {}

async fn set_task(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<TaskDone>,
) -> ApiResult<Response> {
    let config = state.config();
    let push = state.push.clone();
    let task = blocking(move || {
        super::set_task_done(&config, &[Part::Main, Part::Health], &id, body.done, &Who::you(&config), false)
    })
    .await?;
    let Some(task) = task else { return Err(HttpError::new(404, "no such task")) };
    if task.done {
        wake(push, &state.config());
    }
    Ok(web::ok(json!({"task": task.view()})))
}

/// « Dates à venir »: dated facts (birthdays, anniversaries, deadlines of others), never tasks. `days` (default 60,
/// at most 366); `all=1`: every page, not only the close people's.
#[derive(Debug, Deserialize)]
pub struct Close {
    pub path: String,
    #[serde(deserialize_with = "web::lax::bool")]
    pub close: bool,
}

impl Validate for Close {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "path", Some(&self.path), 300);
    }
}

/// A person marked close (« proche ») or not, by the person.
async fn set_close(State(state): State<AppState>, Json(body): Json<Close>) -> ApiResult<Response> {
    let config = state.config();
    let path = blocking(move || super::set_close(&config, &body.path, body.close, &Who::you(&config))).await?;
    match path {
        Some(path) => Ok(web::ok(json!({"path": path}))),
        None => Err(HttpError::new(404, "no such page")),
    }
}

#[derive(Debug, Deserialize)]
pub struct IconChoice {
    pub path: String,
    #[serde(default)]
    pub icon: String,
}

impl Validate for IconChoice {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "path", Some(&self.path), 300);
        check_max_len(errors, "icon", Some(&self.icon), 40);
    }
}

/// A page's icon chosen by the person in the app ("": back to the one its words give).
async fn set_icon(State(state): State<AppState>, Json(body): Json<IconChoice>) -> ApiResult<Response> {
    let config = state.config();
    let path = blocking(move || super::set_icon(&config, &body.path, &body.icon, &Who::you(&config))).await?;
    match path {
        Some(path) => Ok(web::ok(json!({"path": path}))),
        None => Err(HttpError::new(404, "no such page")),
    }
}

/// The agents' task proposals (open, accepted, declined) and the kinds the person declined.
async fn proposals(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let view = blocking(move || {
        let mut all = super::proposals(&config);
        all.reverse();
        Ok(json!({"proposals": all, "declined": super::declined(&config), "per_week": super::PROPOSALS_PER_WEEK}))
    })
    .await?;
    Ok(web::ok(view))
}

#[derive(Debug, Deserialize)]
pub struct Answer {
    #[serde(deserialize_with = "web::lax::bool")]
    pub accept: bool,
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub never: bool,
}

impl Validate for Answer {}

/// The person's answer to a proposal in the app: yes adds its task (with its date: a phone reminder), no closes it
/// (`never`: that kind is not proposed again).
async fn answer_proposal(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<Answer>,
) -> ApiResult<Response> {
    let config = state.config();
    let push = state.push.clone();
    // « Séparer » on a page holding several journeys: its trips made (no task)
    if body.accept && super::trip_split::is_split(&config, &id) {
        let split = blocking(move || super::trip_split::accept(&config, &id, &Who::you(&config))).await?;
        return Ok(web::ok(json!({"split": split})));
    }
    if body.accept {
        let (task, reminder) = blocking(move || super::accept(&config, &id, &Who::you(&config))).await?;
        if reminder {
            wake(push, &state.config());
        }
        return Ok(web::ok(json!({"task": task.view(), "reminder": reminder})));
    }
    let fact = blocking(move || super::decline(&config, &id, body.never, &Who::you(&config))).await?;
    Ok(web::ok(json!({"declined": true, "preference": fact})))
}

async fn records(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let view = blocking(move || Ok(super::records::view(&config, chrono::Local::now().date_naive()))).await?;
    Ok(web::ok(view))
}

#[derive(Debug, Deserialize)]
pub struct Check {
    pub path: String,
    pub item: String,
    #[serde(deserialize_with = "web::lax::bool")]
    pub done: bool,
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub private: bool,
}

impl Validate for Check {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "path", Some(&self.path), 300);
        check_max_len(errors, "item", Some(&self.item), 600);
    }
}

#[derive(Deserialize)]
pub struct RecordArchive {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    #[serde(deserialize_with = "web::lax::bool")]
    pub archived: bool,
}

impl Validate for RecordArchive {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "type", Some(&self.kind), 40);
        check_max_len(errors, "id", Some(&self.id), 40);
    }
}

/// A record archived, or brought back from the archives (then kept live), by the person.
async fn archive_record(State(state): State<AppState>, Json(body): Json<RecordArchive>) -> ApiResult<Response> {
    let config = state.config();
    let Some(kind) = super::records::kind(&body.kind) else {
        return Err(HttpError::new(400, "type is purchase, subscription, contract, paper or booking"));
    };
    let record = blocking(move || {
        super::archive::set_record_archived(&config, kind, &body.id, body.archived, &Who::you(&config))
    })
    .await?;
    match record {
        Some(r) => Ok(web::ok(json!({"record": r}))),
        None => Err(HttpError::new(404, "no such record")),
    }
}

/// A trip's thing to pack ticked (or not) by the person.
async fn check(State(state): State<AppState>, Json(body): Json<Check>) -> ApiResult<Response> {
    let config = state.config();
    let part = if body.private { Part::Health } else { Part::Main };
    let found =
        blocking(move || super::trip::tick(&config, part, &body.path, &body.item, body.done, &Who::you(&config)))
            .await?;
    if !found {
        return Err(HttpError::new(404, "no such thing to pack on this page"));
    }
    Ok(web::ok(json!({"done": true})))
}

async fn documents(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let part = part_of(&q);
    let view = blocking(move || Ok(json!({"documents": super::documents(&config, part)}))).await?;
    Ok(web::ok(view))
}

pub fn settings_view(config: &Config) -> Value {
    let spend = super::tidy::spending(config);
    let state = super::read_state(config);
    let [from, to] = super::ask::quiet_hours(&state);
    json!({"mentions": super::mentions(config), "nightly": super::nightly(config), "nightly_choices": super::NIGHTLY,
           "questions": super::ask::questions_on(&state), "quiet": {"from": from, "to": to},
           "language": super::language(config),
           "monthly_cap": config.notebook.monthly_cap, "spent_this_month": spend.get("eur").cloned().unwrap_or(json!(0.0)),
           "model": config.notebook.model})
}

async fn get_settings(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let view = blocking(move || Ok(settings_view(&config))).await?;
    Ok(web::ok(view))
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    #[serde(default, deserialize_with = "web::lax::opt_bool")]
    pub mentions: Option<bool>,
    #[serde(default)]
    pub nightly: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    /// « Questions pour le Carnet » (ask.rs).
    #[serde(default, deserialize_with = "web::lax::opt_bool")]
    pub questions: Option<bool>,
    /// The place's quiet hours, « HH:MM » (the Carnet's cards, the review, the tips).
    #[serde(default)]
    pub quiet: Option<Quiet>,
}

#[derive(Debug, Deserialize)]
pub struct Quiet {
    pub from: String,
    pub to: String,
}

fn hhmm(text: &str) -> bool {
    text.split_once(':').is_some_and(|(h, m)| {
        h.len() == 2 && m.len() == 2 && h.parse::<u32>().is_ok_and(|h| h < 24) && m.parse::<u32>().is_ok_and(|m| m < 60)
    })
}

impl Validate for Settings {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if let Some(q) = &self.quiet {
            for (k, v) in [("from", &q.from), ("to", &q.to)] {
                if !hhmm(v) {
                    errors.push(FieldError::new("value_error", web::body_loc(k), "Value error, HH:MM", json!(v)));
                }
            }
        }
        if let Some(n) = &self.nightly {
            if !super::NIGHTLY.contains(&n.as_str()) {
                errors.push(FieldError::new(
                    "value_error",
                    web::body_loc("nightly"),
                    format!("Value error, one of {}", super::NIGHTLY.join(", ")),
                    json!(n),
                ));
            }
        }
        if let Some(l) = &self.language {
            if !super::WORDS.iter().any(|w| w.lang == l) {
                errors.push(FieldError::new(
                    "value_error",
                    web::body_loc("language"),
                    "Value error, one of fr, en, de, es",
                    json!(l),
                ));
            }
        }
    }
}

async fn set_settings(State(state): State<AppState>, Json(body): Json<Settings>) -> ApiResult<Response> {
    let config = state.config();
    let view = blocking(move || {
        let _guard = super::lock();
        // a Carnet with pages: its folders and pages moved to the new language's names (relanguage.rs)
        if let Some(l) = &body.language {
            super::relanguage::change_locked(&config, l, &Who::you(&config))?;
        }
        super::update_state(&config, |s| {
            if let Some(m) = body.mentions {
                s.insert("mentions".into(), json!(m));
            }
            if let Some(n) = &body.nightly {
                s.insert("nightly".into(), json!(n));
            }
            if let Some(q) = body.questions {
                s.insert("questions".into(), json!(q));
            }
            if let Some(q) = &body.quiet {
                s.insert("quiet".into(), json!({"from": q.from, "to": q.to}));
            }
        })?;
        Ok(settings_view(&config))
    })
    .await?;
    Ok(web::ok(view))
}

async fn export(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let with_health = part_of(&QueryMap {
        pairs: q.get("health").map(|h| vec![("private".to_string(), h.to_string())]).unwrap_or_default(),
    }) == Part::Health;
    let lang = super::language(&config);
    let data = blocking(move || super::export_zip(&config, with_health)).await?;
    let name = format!(
        "{}.zip",
        if lang == "fr" {
            "Carnet"
        } else if lang == "de" {
            "Notizbuch"
        } else if lang == "es" {
            "Cuaderno"
        } else {
            "Notebook"
        }
    );
    let mut response = Response::new(Body::from(data));
    response.headers_mut().insert("content-type", HeaderValue::from_static("application/zip"));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        response.headers_mut().insert("content-disposition", v);
    }
    response.headers_mut().insert("cache-control", HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn tidy_now(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let record = super::tidy::run(&config, &state.hermes, &state.http).await;
    Ok(web::ok(json!({"run": record})))
}

#[derive(Debug, Deserialize)]
pub struct FillAll {
    #[serde(default)]
    pub agents: Option<Vec<String>>,
}

impl Validate for FillAll {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if let Some(agents) = &self.agents {
            for id in agents {
                check_max_len(errors, "agents", Some(id), 80);
            }
        }
    }
}

/// « Remplir mon Carnet »: each agent's « Remplir le Carnet » task, one after another (fill.rs).
async fn fill_all(State(state): State<AppState>, OptionalJson(body): OptionalJson<FillAll>) -> ApiResult<Response> {
    let jobs = crate::jobs::jobs_of(&state);
    match super::fill::start_all(&jobs, body.and_then(|b| b.agents)) {
        Ok(view) => Ok(web::ok(Value::Object(view))),
        Err(err) if err.0 == "unknown agent" => Err(HttpError::new(404, err.0)),
        Err(err) => Err(HttpError::new(409, err.0)),
    }
}

async fn fill_view(State(state): State<AppState>) -> ApiResult<Response> {
    Ok(web::ok(Value::Object(super::fill::view(&state.config()))))
}

async fn fill_stop(State(state): State<AppState>) -> ApiResult<Response> {
    Ok(web::ok(Value::Object(super::fill::stop_all(&crate::jobs::jobs_of(&state)))))
}

/// `/v1/notebook…` (mounted under /v1 with the bridge key).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/notebook", get(home))
        .route("/notebook/pages", get(list_pages))
        .route("/notebook/search", get(search))
        .route("/notebook/offline", get(offline))
        .route("/notebook/known", get(known))
        .route("/notebook/forget/preview", post(forget_preview))
        .route("/notebook/forget/confirm", post(forget_confirm))
        .route("/notebook/forget/cancel", post(forget_cancel))
        .route("/notebook/ask/{id}", post(ask_answer))
        .route("/notebook/follow/{id}", post(follow_answer))
        .route("/notebook/review", get(review))
        .route("/notebook/review/done", post(review_done))
        .route("/notebook/paper", post(paper))
        .route("/notebook/papers/{id}/photo", get(paper_photo))
        .route("/notebook/page", get(read_page).post(new_page).delete(delete_page))
        .route("/notebook/section", axum::routing::put(write_section))
        .route("/notebook/targets", get(targets))
        .route("/notebook/move", post(move_points))
        .route("/notebook/move/undo", post(move_undo))
        .route("/notebook/misplaced", post(misplaced))
        .route("/notebook/history", get(history))
        .route("/notebook/restore", post(restore))
        .route("/notebook/tasks", get(list_tasks).post(add_task))
        .route("/notebook/tasks/{id}", post(set_task))
        .route("/notebook/documents", get(documents))
        .route("/notebook/close", post(set_close))
        .route("/notebook/icon", post(set_icon))
        .route("/notebook/proposals", get(proposals))
        .route("/notebook/proposals/{id}", post(answer_proposal))
        .route("/notebook/records", get(records))
        .route("/notebook/records/archive", post(archive_record))
        .route("/notebook/check", post(check))
        .route("/notebook/settings", get(get_settings).patch(set_settings))
        .route("/notebook/export", get(export))
        .route("/notebook/tidy", post(tidy_now))
        .route("/notebook/fill", get(fill_view).post(fill_all).delete(fill_stop))
}

// -- « Déplacer vers… » (moves.rs) and the misplaced points' card (misplaced.rs) ---------------------------------------

/// Days a point is « nouveau » on its page.
pub const FRESH_DAYS: i64 = 7;

/// Whether a fact noted on `date` (YYYY-MM-DD) is less than `FRESH_DAYS` old.
fn fresh(date: &str) -> bool {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|d| (chrono::Local::now().date_naive() - d).num_days() < FRESH_DAYS)
        .unwrap_or(false)
}

/// « Noté par Kerfy Studio le 25 oct. 2026 » (the long press), null without a date.
fn noted(who: &str, date: &str, lang: &str) -> Value {
    let Ok(d) = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") else { return Value::Null };
    use chrono::Datelike;
    let day = format!("{} {}", super::offline::day(d, lang), d.year());
    let text = match (lang, who.is_empty()) {
        ("fr", false) => format!("Noté par {who} le {day}"),
        ("fr", true) => format!("Noté le {day}"),
        ("de", false) => format!("Notiert von {who} am {day}"),
        ("de", true) => format!("Notiert am {day}"),
        ("es", false) => format!("Anotado por {who} el {day}"),
        ("es", true) => format!("Anotado el {day}"),
        (_, false) => format!("Noted by {who} on {day}"),
        (_, true) => format!("Noted on {day}"),
    };
    json!(text)
}

fn move_texts(lang: &str) -> (&'static str, &'static str, &'static str) {
    match lang {
        "fr" => ("Déplacé vers « {} ».", "{} range ça là désormais.", "Annuler"),
        "de" => ("Nach « {} » verschoben.", "{} legt so etwas künftig dort ab.", "Rückgängig"),
        "es" => ("Movido a « {} ».", "{} lo guardará ahí a partir de ahora.", "Deshacer"),
        _ => ("Moved to « {} ».", "{} will file such points there from now on.", "Undo"),
    }
}

fn move_refused(err: anyhow::Error, lang: &str) -> HttpError {
    use super::moves::Refused;
    match err.downcast_ref::<Refused>() {
        Some(Refused::NoPage) => carnet_error(404, "no_such_page", lang, json!({})),
        Some(Refused::Nothing) => carnet_error(404, "nothing_to_move", lang, json!({})),
        Some(Refused::Health) => carnet_error(422, "health_stays", lang, json!({})),
        Some(Refused::Gone) => carnet_error(410, "move_gone", lang, json!({})),
        None => match err.downcast_ref::<super::PageChanged>() {
            Some(changed) => carnet_error(409, "page_changed", lang, json!({"rev": changed.0})),
            None => match err.downcast_ref::<super::NotebookError>() {
                Some(message) => HttpError::new(400, message.0.clone()),
                None => err.into(),
            },
        },
    }
}

fn target_view(part: Part, path: &str, page: &super::Page, text: &str, shown: &str) -> Value {
    let kind = page.kind();
    let section = if text.is_empty() || super::template::outline(&kind).is_empty() {
        Value::Null
    } else {
        json!(super::template::place(&kind, "", text))
    };
    let title = if kind == "self" { super::words(shown).me.to_string() } else { page.title.clone() };
    json!({"path": path, "part": part.as_str(), "title": title, "kind": kind, "section": section})
}

/// « Déplacer vers… »: the pages proposed first, then all of them (« Toi » first, then by kind and name), and each
/// kind's outline (for the section step).
async fn targets(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_language(&config, &q);
    let part = part_of(&q);
    let path = web::query_str(&q, "path", true, Some(300))?.unwrap_or_default();
    let text = web::query_str(&q, "text", false, Some(600))?.unwrap_or_default();
    let view = blocking(move || {
        super::checked(&path)?;
        let lang = super::language(&config);
        let w = super::words(&lang);
        let Some(source_text) = super::read_page_text(&config, part, &path) else {
            return Err(super::NotebookError("no such page".into()).into());
        };
        let source = super::Page::parse(&source_text, "");
        // who noted the point: where that agent's points went before, or where it writes
        let who = source
            .sections()
            .into_iter()
            .flat_map(|(_, facts)| facts)
            .find(|f| !text.is_empty() && super::fold(&f.text) == super::fold(&text))
            .map(|f| f.who)
            .unwrap_or_default();
        let agent = super::assign::agent_named(&config, &who);
        let mut pages: Vec<(Part, String, super::Page)> = Vec::new();
        for p in [Part::Main, Part::Health] {
            for (pp, page) in super::all_pages(&config, p) {
                let kind = page.kind();
                if !super::KINDS.contains(&kind.as_str()) || w.archived(&pp) || (p == part && pp == path) {
                    continue;
                }
                pages.push((p, pp, page));
            }
        }
        let order = |k: &str| ["self", "subject", "area", "person", "trip"].iter().position(|x| *x == k).unwrap_or(9);
        pages.sort_by(|a, b| {
            (a.0 == Part::Health, order(&a.2.kind()), super::fold(&a.2.title))
                .cmp(&(b.0 == Part::Health, order(&b.2.kind()), super::fold(&b.2.title)))
        });
        let mut suggested: Vec<Value> = Vec::new();
        let mut suggest = |v: Value, out: &mut Vec<Value>| {
            if out.len() < 3 && !out.iter().any(|o| o["path"] == v["path"] && o["title"] == v["title"]) {
                out.push(v);
            }
        };
        let state_now = super::read_state(&config);
        if let Some(agent) = agent.as_deref() {
            // where the person moved this agent's points from this page before
            for c in state_now.get("corrections").and_then(Value::as_array).into_iter().flatten().rev() {
                if tk::str_or_empty(c.get("agent")) == agent && tk::str_or_empty(c.get("from_path")) == path {
                    let to = tk::str_or_empty(c.get("to_path"));
                    if let Some((p, pp, page)) = pages.iter().find(|(_, pp, _)| *pp == to) {
                        suggest(target_view(*p, pp, page, &text, &shown), &mut suggested);
                    }
                }
            }
        }
        // the page the point's label names
        if let Some((label, _)) = super::lexicon::split_label(&text) {
            if let Some((p, pp, page)) = pages.iter().find(|(_, _, page)| super::fold(&page.title) == super::fold(&label)) {
                suggest(target_view(*p, pp, page, &text, &shown), &mut suggested);
            }
        }
        // out of the health part: the page of the same name
        if part == Part::Health {
            if let Some((p, pp, page)) = pages.iter().find(|(p, _, page)| *p == Part::Main && super::fold(&page.title) == super::fold(&source.title)) {
                suggest(target_view(*p, pp, page, &text, &shown), &mut suggested);
            }
        }
        // a work agent's: « Travail »
        if agent.as_deref().is_some_and(|a| super::assign::is_work_agent(&config, a)) {
            let area = super::assign::work_area(&lang);
            match pages.iter().find(|(p, _, page)| *p == Part::Main && super::fold(&page.title) == super::fold(area)) {
                Some((p, pp, page)) => suggest(target_view(*p, pp, page, &text, &shown), &mut suggested),
                None => suggest(json!({"path": null, "part": "main", "title": area, "kind": "area",
                                       "section": null}), &mut suggested),
            }
        }
        // the pages this agent wrote on lately
        if let Some(agent) = agent.as_deref() {
            let mut seen: Vec<String> = Vec::new();
            for entry in super::history(&config, None, None, 400) {
                if tk::str_or_empty(entry.get("agent")) != agent {
                    continue;
                }
                let pp = tk::str_or_empty(entry.get("path"));
                if seen.contains(&pp) || pp == path {
                    continue;
                }
                seen.push(pp.clone());
                if let Some((p, pp, page)) = pages.iter().find(|(_, x, page)| *x == pp && !super::assign::is_family_page(page)) {
                    suggest(target_view(*p, pp, page, &text, &shown), &mut suggested);
                }
            }
        }
        let all: Vec<Value> = pages.iter().map(|(p, pp, page)| target_view(*p, pp, page, &text, &shown)).collect();
        let outlines: serde_json::Map<String, Value> = super::KINDS
            .iter()
            .map(|k| {
                let items: Vec<Value> = super::template::outline(k)
                    .iter()
                    .map(|s| json!({"id": s.id, "title": super::template::title(k, s.id, &shown)}))
                    .collect();
                (k.to_string(), Value::Array(items))
            })
            .collect();
        let kinds: Vec<Value> = ["subject", "area", "person"]
            .iter()
            .map(|k| json!({"kind": k, "title": super::words(&shown).folder(k).unwrap_or(k)}))
            .collect();
        Ok(json!({"suggested": suggested, "pages": all, "outlines": outlines, "new_kinds": kinds}))
    })
    .await?;
    Ok(web::ok(view))
}

#[derive(Debug, Default, Deserialize)]
pub struct MoveTo {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub private: bool,
}

#[derive(Debug, Deserialize)]
pub struct MoveBody {
    pub path: String,
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub private: bool,
    #[serde(default)]
    pub rev: Option<String>,
    #[serde(default)]
    pub texts: Vec<String>,
    #[serde(default)]
    pub section: Option<String>,
    pub to: MoveTo,
    #[serde(default)]
    pub to_section: Option<String>,
}

impl Validate for MoveBody {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "path", Some(&self.path), 300);
        check_max_len(errors, "rev", self.rev.as_deref(), 16);
        web::check_max_items(errors, "texts", Some(&self.texts), super::moves::POINTS_MAX);
        for text in &self.texts {
            check_max_len(errors, "texts", Some(text), 700);
        }
        check_max_len(errors, "section", self.section.as_deref(), 120);
        check_max_len(errors, "to.path", self.to.path.as_deref(), 300);
        check_max_len(errors, "to.title", self.to.title.as_deref(), 200);
        check_max_len(errors, "to.kind", self.to.kind.as_deref(), 20);
        check_max_len(errors, "to_section", self.to_section.as_deref(), 80);
    }
}

/// « Déplacer vers… »: the page as it is now, where the points went, the toast and « Annuler ».
async fn move_points(State(state): State<AppState>, q: QueryMap, Json(body): Json<MoveBody>) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_language(&config, &q);
    let lang = shown.clone();
    let part = if body.private { Part::Health } else { Part::Main };
    let done = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let to = super::moves::Target {
            part: Some(if body.to.private { Part::Health } else { Part::Main }),
            path: body.to.path.clone().unwrap_or_default(),
            title: body.to.title.clone().unwrap_or_default(),
            kind: body.to.kind.clone().unwrap_or_default(),
        };
        let moved = super::moves::move_points(
            &config,
            part,
            &body.path,
            body.rev.as_deref(),
            &body.texts,
            body.section.as_deref(),
            &to,
            body.to_section.as_deref().unwrap_or(""),
            &Who::you(&config),
            true,
        )?;
        let page = page_view_in(&config, part, &body.path, &lang)?;
        let (toast, learnt, undo) = move_texts(&lang);
        let learnt = if moved.learnt.is_empty() { Value::Null } else { json!(learnt.replace("{}", &moved.learnt.join(", "))) };
        Ok(json!({"page": page, "moved": moved.moved, "move": moved.id, "undo": undo,
                  "to": {"path": moved.to_path, "part": moved.to_part.unwrap_or(part).as_str(), "title": moved.to_title},
                  "toast": toast.replace("{}", &moved.to_title), "learnt": learnt}))
    })
    .await
    .map_err(anyhow::Error::from)?;
    match done {
        Ok(view) => Ok(web::ok(view)),
        Err(err) => Err(move_refused(err, &shown)),
    }
}

#[derive(Debug, Deserialize)]
pub struct MoveUndo {
    #[serde(rename = "move")]
    pub id: String,
}

impl Validate for MoveUndo {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "move", Some(&self.id), 40);
    }
}

async fn move_undo(State(state): State<AppState>, q: QueryMap, Json(body): Json<MoveUndo>) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_language(&config, &q);
    let lang = shown.clone();
    let done = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let (part, path) = super::moves::undo(&config, &body.id, &Who::you(&config))?;
        Ok(json!({"page": page_view_in(&config, part, &path, &lang)?}))
    })
    .await
    .map_err(anyhow::Error::from)?;
    match done {
        Ok(view) => Ok(web::ok(view)),
        Err(err) => Err(move_refused(err, &shown)),
    }
}

#[derive(Debug, Deserialize)]
pub struct Misplaced {
    pub action: String,
    #[serde(default)]
    pub ids: Vec<String>,
}

impl Validate for Misplaced {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if self.action != "move" && self.action != "leave" {
            errors.push(FieldError::new("value_error", web::body_loc("action"), "Value error, move or leave", json!(self.action)));
        }
        web::check_max_items(errors, "ids", Some(&self.ids), super::misplaced::ITEMS_MAX);
        for id in &self.ids {
            check_max_len(errors, "ids", Some(id), 40);
        }
    }
}

async fn misplaced(State(state): State<AppState>, q: QueryMap, Json(body): Json<Misplaced>) -> ApiResult<Response> {
    let config = state.config();
    let shown = shown_language(&config, &q);
    let view = blocking(move || {
        let (moved, toast, card) = super::misplaced::answer(&config, &body.action, &body.ids, &shown)?;
        Ok(json!({"moved": moved, "toast": if toast.is_empty() { Value::Null } else { json!(toast) }, "card": card}))
    })
    .await?;
    Ok(web::ok(view))
}
