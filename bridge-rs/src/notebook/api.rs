//! The Carnet for the apps (the bridge key, like the other app routes; mounted under /v1). The private health part
//! (`private=1` / `"private": true`) is the person's: the home leaves it out (it is reached only through its own page,
//! behind the app's lock).
//!
//!     GET    /v1/notebook                      home: sections and counts, « Nouveau » (recent changes), upcoming tasks
//!     GET    /v1/notebook/pages?kind=          a section's pages (kind: person, subject, area, other), with what their card
//!                                              shows (close, next_date, status, facts)
//!     GET    /v1/notebook/search?q=            pages, and the facts that answer (for a short answer on top)
//!     GET    /v1/notebook/page?path=           a page: frontmatter apart, « En bref », facts with attribution, links
//!     PUT    /v1/notebook/page                 {path, text, private?}   the person's edit (« toi » in its history)
//!     PUT    /v1/notebook/section              {path, section (its id), text, private?}   « Modifier » on one section
//!     DELETE /v1/notebook/page?path=
//!     GET    /v1/notebook/history?path=        changes (a page's, or all), the newest first
//!     POST   /v1/notebook/restore              {change}   the page as it was after that change
//!     GET    /v1/notebook/tasks                open and recent tasks
//!     POST   /v1/notebook/tasks                {title, due?, page?, private?}
//!     POST   /v1/notebook/tasks/{id}           {done}
//!     GET    /v1/notebook/dates?days=&all=     « Dates à venir »: dated facts (close people's by default), not tasks
//!     POST   /v1/notebook/close                {path, close}   a person marked close (« proche ») or not
//!     POST   /v1/notebook/icon                 {path, icon}    a subject's or area's icon, the person's choice ("": automatic)
//!     GET    /v1/notebook/proposals            the agents' task proposals, the kinds declined
//!     GET    /v1/notebook/documents?private=   the files the agents made or received and their web pages, by agent,
//!                                              with their subjects (the private part's pages only with private=1)
//!     GET    /v1/notebook/settings             {mentions, nightly, language, cap…}
//!     PATCH  /v1/notebook/settings             {mentions?, nightly?, language? (only while the Carnet is empty)}
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

fn part_of(q: &QueryMap) -> Part {
    if matches!(q.get("private"), Some("1" | "true" | "yes")) {
        Part::Health
    } else {
        Part::Main
    }
}

fn change_view(config: &Config, entry: &Dict) -> Value {
    let mut out = entry.clone();
    // Always strings for the apps: the person's own changes and the nightly tidy's have no agent (null in the log).
    for key in ["id", "at", "by", "name", "agent", "part", "path", "title", "action", "summary"] {
        let text = tk::str_or_empty(entry.get(key));
        out.insert(key.into(), json!(text));
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
        "person" | "subject" | "area" | "self" | "tasks" => kind,
        _ if path == w.inbox_path() => "inbox".into(),
        _ => "other".into(),
    }
}

async fn home(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let view = blocking(move || {
        let lang = super::language(&config);
        let w = super::words(&lang);
        let pages = super::all_pages(&config, Part::Main);
        // the person's own page first (« Toi »: one page, its own card), then the sections
        let mine = pages.iter().find(|(_, p)| p.kind() == "self").map(|(path, _)| path.clone());
        let mut sections = vec![json!({"kind": "self", "title": w.me, "count": usize::from(mine.is_some()),
                                       "path": mine.unwrap_or_else(|| w.self_path())})];
        for (kind, folder) in [("person", w.people), ("subject", w.subjects), ("area", w.areas)] {
            let count = pages.iter().filter(|(_, p)| p.kind() == kind).count();
            sections.push(json!({"kind": kind, "title": folder, "count": count}));
        }
        // the open tasks that count: archived ones left out, the person's own first
        let open = super::open_tasks(&config, Part::Main);
        sections.push(json!({"kind": "tasks", "title": w.tasks, "count": open.len()}));
        let documents = super::documents(&config, Part::Main);
        sections.push(json!({"kind": "documents", "title": w.documents, "count": documents.len()}));
        let (_, inbox) = super::inbox(&config);
        let recent: Vec<Value> =
            super::history(&config, Some(Part::Main), None, 20).iter().map(|e| change_view(&config, e)).collect();
        let mut upcoming: Vec<&super::Task> = open.iter().filter(|t| !t.due.is_empty()).collect();
        upcoming.sort_by(|a, b| a.due.cmp(&b.due));
        let upcoming: Vec<Value> = upcoming.iter().take(10).map(|t| t.view()).collect();
        // « Dates à venir »: close people's dated facts in the next 30 days (facts, not tasks)
        let dates = super::upcoming_dates(&config, 30, false, chrono::Local::now().date_naive());
        Ok(json!({"language": lang, "sections": sections, "recent": recent, "upcoming": upcoming,
                  "dates": dates.into_iter().take(10).collect::<Vec<_>>(),
                  "inbox": inbox.len(), "settings": settings_view(&config)}))
    })
    .await?;
    Ok(web::ok(view))
}

fn page_summary(path: &str, part: Part, page: &super::Page, brief: &str) -> Value {
    json!({"path": path, "part": part.as_str(), "title": page.title, "kind": page.kind(), "aliases": page.aliases(),
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
        let pages: Vec<Value> = super::all_pages(&config, part)
            .iter()
            .filter(|(path, p)| kind.is_empty() || section_of(w, p, path) == kind)
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
                view["status"] = sections
                    .iter()
                    .find(|(name, _)| p.kind() == "subject" && *name == status)
                    .and_then(|(_, facts)| facts.first())
                    .map(|f| json!(f.text))
                    .unwrap_or(Value::Null);
                view["facts"] = json!(sections
                    .iter()
                    .filter(|(name, _)| super::fold(name) != super::fold(w.brief))
                    .map(|(_, facts)| facts.len())
                    .sum::<usize>());
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
    let words = web::query_str(&q, "q", true, Some(200))?.unwrap_or_default();
    let view = blocking(move || {
        let w = super::words(&super::language(&config));
        let found = super::search(&config, &[part], &words, "");
        let wanted: Vec<String> = super::fold(&words).split(' ').filter(|x| x.len() > 1).map(str::to_string).collect();
        // the facts that answer: the words not in the page's name found in the fact (« anniversaire marc » → Marc's
        // birthday line), for a short answer on top
        let mut answers: Vec<Value> = Vec::new();
        for (part, path, page, _) in &found {
            let names = super::fold(&format!("{} {}", page.title, page.aliases().join(" ")));
            let rest: Vec<&String> = wanted.iter().filter(|x| !names.contains(x.as_str())).collect();
            if rest.is_empty() {
                continue;
            }
            for (section, facts) in page.sections() {
                if super::fold(&section) == super::fold(w.brief) {
                    continue;
                }
                for fact in facts {
                    let folded = super::fold(&format!("{section} {}", fact.text));
                    if rest.iter().all(|x| folded.contains(x.as_str())) {
                        let short =
                            fact.text.split_once(':').map(|(_, v)| v.trim().to_string()).filter(|v| !v.is_empty());
                        answers.push(
                            json!({"page": page.title, "path": path, "part": part.as_str(), "section": section,
                                            "fact": fact.text, "answer": short.unwrap_or_else(|| fact.text.clone()),
                                            "by": fact.who, "date": fact.date}),
                        );
                    }
                }
            }
        }
        answers.truncate(5);
        let pages: Vec<Value> = found
            .iter()
            .take(50)
            .map(|(part, path, page, lines)| {
                let mut v = page_summary(path, *part, page, w.brief);
                v["lines"] = json!(lines);
                v
            })
            .collect();
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

/// A page for the app: its frontmatter apart, its « En bref », its sections with each fact's attribution, its
/// links resolved to pages (or not found), the pages that link to it.
pub fn page_view(config: &Config, part: Part, path: &str) -> anyhow::Result<Value> {
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
                    json!({"text": f.text, "by": if f.who.is_empty() { Value::Null } else { json!(f.who) },
                           "date": if f.date.is_empty() { Value::Null } else { json!(f.date) }, "item": f.item, "links": links})
                })
                .collect();
            let id = super::template::resolve(&kind, &name)
                .or_else(|| (super::fold(&name) == super::fold(&super::template::title(&kind, "other", &lang))).then_some("other"));
            json!({"id": id, "title": name, "facts": facts})
        })
        .collect();
    // the kind's outline, for « Modifier » on a section and adding to an empty one
    let outline: Vec<Value> = super::template::outline(&kind)
        .iter()
        .map(|s| json!({"id": s.id, "title": super::template::title(&kind, s.id, &lang)}))
        .collect();
    let links: Vec<Value> =
        super::links_in(&page.body).iter().filter(|l| !l.starts_with("archive:")).map(|l| resolve(l)).collect();
    let mut names: Vec<String> = vec![page.title.clone()];
    names.extend(page.aliases());
    let backlinks: Vec<Value> = all
        .iter()
        .filter(|(p, _)| p != path)
        .filter(|(_, pg)| {
            super::links_in(&pg.body).iter().any(|l| names.iter().any(|n| super::fold(n) == super::fold(l)))
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
    Ok(json!({"path": path, "part": part.as_str(), "title": page.title, "kind": page.kind(),
              "frontmatter": page.front, "aliases": page.aliases(),
              "contact": if page.contact().is_empty() { Value::Null } else { json!(page.contact()) },
              "updated": page.front.get("updated").cloned().unwrap_or(Value::Null),
              "summary": super::summary_of(&page.body, w.brief), "sections": sections, "outline": outline, "links": links,
              "backlinks": backlinks, "tasks": tasks, "documents": documents, "id": page.id(), "close": page.close(),
              "icon": page.icon(), "shared_with": super::shares_of(config, part, path, &page), "text": text}))
}

async fn read_page(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let part = part_of(&q);
    let path = web::query_str(&q, "path", true, Some(300))?.unwrap_or_default();
    let view =
        tokio::task::spawn_blocking(move || page_view(&config, part, &path)).await.map_err(anyhow::Error::from)?;
    match view {
        Ok(v) => Ok(web::ok(v)),
        Err(err) => match err.downcast_ref::<super::NotebookError>() {
            Some(e) => Err(HttpError::new(404, e.0.clone())),
            None => Err(err.into()),
        },
    }
}

#[derive(Debug, Deserialize)]
pub struct PageEdit {
    pub path: String,
    pub text: String,
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub private: bool,
}

impl Validate for PageEdit {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "path", Some(&self.path), 300);
        check_max_len(errors, "text", Some(&self.text), 200_000);
    }
}

async fn write_page(State(state): State<AppState>, Json(body): Json<PageEdit>) -> ApiResult<Response> {
    let config = state.config();
    let part = if body.private { Part::Health } else { Part::Main };
    let entry = blocking(move || super::write_page(&config, part, &body.path, &body.text, &Who::you(&config))).await?;
    Ok(web::ok(json!({"change": entry})))
}

#[derive(Debug, Deserialize)]
pub struct SectionEdit {
    pub path: String,
    /// The section's id in the page's outline (`dates`, `decisions`…).
    pub section: String,
    /// Its new lines (« - … »; empty: the section is emptied). A line unchanged keeps its attribution.
    pub text: String,
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub private: bool,
}

impl Validate for SectionEdit {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "path", Some(&self.path), 300);
        check_max_len(errors, "section", Some(&self.section), 80);
        check_max_len(errors, "text", Some(&self.text), 50_000);
    }
}

/// The person's « Modifier » on one section of a page.
async fn write_section(State(state): State<AppState>, Json(body): Json<SectionEdit>) -> ApiResult<Response> {
    let config = state.config();
    let part = if body.private { Part::Health } else { Part::Main };
    let view = blocking(move || {
        super::checked(&body.path)?;
        let who = Who::you(&config);
        let (written, _) =
            super::edit(&config, &[part], &body.path, "", "", &body.section, Some(&body.text), "", &who)?;
        page_view(&config, part, &written.path)
    })
    .await?;
    Ok(web::ok(view))
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
    let part = part_of(&q);
    let path = q.get("path").map(str::to_string);
    let view = blocking(move || {
        let changes: Vec<Value> =
            super::history(&config, Some(part), path.as_deref(), 200).iter().map(|e| change_view(&config, e)).collect();
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
async fn dates(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let days = q.get("days").and_then(|d| d.parse::<i64>().ok()).unwrap_or(60).clamp(1, 366);
    let all = matches!(q.get("all"), Some("1" | "true" | "yes"));
    let view = blocking(move || {
        Ok(json!({"dates": super::upcoming_dates(&config, days, all, chrono::Local::now().date_naive())}))
    })
    .await?;
    Ok(web::ok(view))
}

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

async fn documents(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let part = part_of(&q);
    let view = blocking(move || Ok(json!({"documents": super::documents(&config, part)}))).await?;
    Ok(web::ok(view))
}

pub fn settings_view(config: &Config) -> Value {
    let spend = super::tidy::spending(config);
    json!({"mentions": super::mentions(config), "nightly": super::nightly(config), "nightly_choices": super::NIGHTLY,
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
}

impl Validate for Settings {
    fn validate(&self, errors: &mut Vec<FieldError>) {
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
        if body.language.is_some()
            && (!super::all_pages(&config, Part::Main).is_empty()
                || !super::all_pages(&config, Part::Health).is_empty())
        {
            return Err(super::NotebookError("the Carnet's language is set once it has pages".into()).into());
        }
        super::update_state(&config, |s| {
            if let Some(m) = body.mentions {
                s.insert("mentions".into(), json!(m));
            }
            if let Some(n) = &body.nightly {
                s.insert("nightly".into(), json!(n));
            }
            if let Some(l) = &body.language {
                s.insert("language".into(), json!(l));
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

async fn runs(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let view = blocking(move || {
        Ok(json!({"runs": super::tidy::runs(&config), "spend": super::tidy::spending(&config),
                  "monthly_cap": config.notebook.monthly_cap}))
    })
    .await?;
    Ok(web::ok(view))
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
        .route("/notebook/page", get(read_page).put(write_page).delete(delete_page))
        .route("/notebook/section", axum::routing::put(write_section))
        .route("/notebook/history", get(history))
        .route("/notebook/restore", post(restore))
        .route("/notebook/tasks", get(list_tasks).post(add_task))
        .route("/notebook/tasks/{id}", post(set_task))
        .route("/notebook/documents", get(documents))
        .route("/notebook/dates", get(dates))
        .route("/notebook/close", post(set_close))
        .route("/notebook/icon", post(set_icon))
        .route("/notebook/proposals", get(proposals))
        .route("/notebook/settings", get(get_settings).patch(set_settings))
        .route("/notebook/export", get(export))
        .route("/notebook/tidy", post(tidy_now))
        .route("/notebook/runs", get(runs))
        .route("/notebook/fill", get(fill_view).post(fill_all).delete(fill_stop))
}
