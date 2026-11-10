//! Who may write where (Alex, 10/11: his work agent « Kerfy Studio » filed BipAgents' market facts on his « Famille »
//! page). A work agent — in the « Pro » group of the apps' home (agent_groups.rs, any language) or with the work Bip
//! (`category: work`) — never writes on the family's pages: the area « Famille », the group page « Famille élargie »
//! (groups.rs) or the family part of « Toi ». `note_locked` refuses it with a sentence that says where such facts go;
//! the nightly pass (tidy.rs) is told each agent's group so its model does not try; the misplaced points' card
//! (misplaced.rs) proposes moving what was filed there before.

use super::{fold, groups, template, Page, WORDS};
use crate::config::Config;

/// The group names (folded) that mean work, in the apps' languages.
const WORK_GROUPS: &[&str] = &["pro", "travail", "boulot", "work", "job", "business", "arbeit", "beruf", "trabajo"];

/// The family area's title in each language (folded).
const FAMILY_AREAS: &[&str] = &["famille", "family", "familie", "familia", "ma famille", "my family", "meine familie", "mi familia"];

/// The work area's title, where a work agent's facts go when no project page fits (fr, en, de, es).
pub const WORK_AREA: [&str; 4] = ["Travail", "Work", "Arbeit", "Trabajo"];

pub fn work_area(lang: &str) -> &'static str {
    WORK_AREA[super::lang_index(lang)]
}

/// Whether `agent` works for the person's job: its group says so, or its Bip is the work one.
pub fn is_work_agent(config: &Config, agent: &str) -> bool {
    let group = fold(&crate::agent_groups::group_of(config, agent));
    if WORK_GROUPS.contains(&group.as_str()) {
        return true;
    }
    config.agent(Some(agent)).and_then(|a| a.category.as_deref()) == Some("work")
}

/// The group a model is told an agent is in: « Pro », « Perso », « Famille »… (the person's word), else « work »
/// for the work Bip, else nothing.
pub fn group_label(config: &Config, agent: &str) -> String {
    let group = crate::agent_groups::group_of(config, agent);
    if !group.trim().is_empty() {
        return group;
    }
    if is_work_agent(config, agent) {
        "work".into()
    } else {
        String::new()
    }
}

/// Whether a page is one of the family's: the area « Famille » or the group page « Famille élargie ».
pub fn is_family_page(page: &Page) -> bool {
    let title = fold(&page.title);
    let kind = page.kind();
    ((kind == "area" || kind.is_empty()) && FAMILY_AREAS.contains(&title.as_str()))
        || WORDS.iter().any(|w| fold(groups::group_title("family", w.lang)) == title)
}

/// Whether a page's title names a family page, before the page exists (a new page « Famille »).
pub fn is_family_title(title: &str) -> bool {
    let title = fold(title);
    FAMILY_AREAS.contains(&title.as_str()) || WORDS.iter().any(|w| fold(groups::group_title("family", w.lang)) == title)
}

/// Whether facts written on `page` under `section` (each placed by its words when the section says nothing) land in
/// a family place: a family page, or the family section of « Toi ».
pub fn lands_in_family(page: &Page, section: &str, facts: &[String]) -> bool {
    if is_family_page(page) {
        return true;
    }
    page.kind() == "self" && facts.iter().any(|f| template::place("self", section, f) == "family")
}

/// The sentence a work agent gets back (it goes to the model: English, like the tools).
pub fn refused(lang: &str) -> String {
    format!(
        "Not filed: work facts never go on the family's pages. File them on the project's page (kind subject, \
         named after the project) or on the « {} » area.",
        work_area(lang)
    )
}

/// The agent a fact's attribution names (« Kerfy Studio » → its id), if one of this install's agents has that name.
pub fn agent_named(config: &Config, name: &str) -> Option<String> {
    let wanted = fold(name);
    if wanted.is_empty() {
        return None;
    }
    config
        .agents
        .values()
        .find(|a| fold(&a.display_name) == wanted || fold(&a.name) == wanted)
        .map(|a| a.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(kind: &str, title: &str) -> Page {
        Page::new(kind, title)
    }

    #[test]
    fn family_pages_and_sections() {
        assert!(is_family_page(&page("area", "Famille")));
        assert!(is_family_page(&page("area", "Famille élargie")));
        assert!(is_family_page(&page("area", "Extended family")));
        assert!(!is_family_page(&page("area", "Maison")));
        assert!(!is_family_page(&page("subject", "BipAgents")));
        assert!(is_family_title("famille"));
        assert!(!is_family_title("Famille Martin"));
        let me = page("self", "Toi");
        assert!(lands_in_family(&me, "family", &["Sœur : Léa".into()]));
        assert!(!lands_in_family(&me, "work", &["Fonde Kerfy Studio".into()]));
    }
}
