//! A turn that ends on a promise gets one nudge to do the work (Alex 13/11, his « Vie »: « Je te prépare 2-3
//! trouvailles pour 5 € », the turn over with no search; then, after « vas-y », a plan ending « Je reviens dans quelques
//! minutes avec le lien et le meilleur prix », over again with no tool call). An answer without a tool call ends a
//! Hermes turn: nothing runs after it until the person writes again, so such a promise is never kept.
//!
//! `souls::ACK_RULE` tells the agent so; this is the bridge's safety net for what the rule misses (runs.rs `go_on`):
//! a person's conversation turn that called no working tool (only questions, its task list, memory, tool search…) and
//! whose last words announce work to come gets, at its end, ONE more turn of the same agent in the same
//! conversation, told out of sight to do it now or to start a background task (`input`). Its events follow in the
//! turn's stream after a `loop_rescue_started` notice (the apps adopt its run, as for the brake's rescue turn), and its
//! end is the turn's end. Never for a continuation itself, a background task, a routine or any turn nobody started from
//! an app, a group discussion, a relative's or a guest's turn, a turn the brake stopped or that handed work over; a
//! Live call's turns are not runs (live_turns.rs). An answer whose end asks the person anything (« Je te prépare le
//! panier ? », « Dis-moi lequel et je m'y mets ») waits for them: no nudge. Bench: internal/notes/promise-stop.md.

use std::sync::LazyLock;

use regex::Regex;

/// The continuation's input starts with this (the timeline hides the row: threads/timeline.rs `BRIDGE_MARK`).
pub const MARK: &str = "[bipagents:go_on]";

/// Tools that do no work for the person: a turn that called only these did nothing it could announce.
pub const TALK_TOOLS: [&str; 10] = [
    "clarify",
    "todo_list",
    "memory",
    "skill_view",
    "skills_list",
    "tool_search",
    "tool_describe",
    "session_search",
    "notebook_search",
    "read_file",
];

/// The last characters of a reply where a promise counts: the end of what it says.
pub const TAIL_CHARS: usize = 400;

/// The continuation followed in the turn's stream this long at most (past ~3 minutes the long-work net hands it to a
/// background task anyway: runs.rs `handoff`); after that it goes on alone, its run adopted by the apps.
pub const MAX_SECONDS: f64 = 1800.0;

/// Work announced for later, in the five languages of the agents (lower case; apostrophes made straight first).
static PROMISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)",
        // French
        r"\bje (?:te |t'|vous )?(?:pr[ée]pare|cherche|regarde|lance|compare|fouille|reviens|m'en occupe|m'y mets|m'en charge)\b",
        r"|\bje vais (?:te |t'|vous )?(?:chercher|regarder|trouver|comparer|voir|fouiller|lancer|pr[ée]parer|d[ée]nicher|faire une recherche|m'en occuper)\b",
        r"|\bje (?:te |vous )?(?:tiens au courant|recontacte|reviens)\b",
        r"|\bdans (?:quelques|deux|trois|cinq|dix|\d+) (?:minutes|instants)\b|\bdans un instant\b|\blaisse-moi (?:chercher|regarder|v[ée]rifier)\b",
        // English
        r"|\bi(?:'ll| will) (?:look|search|check|find|get back|come back|be back|prepare|put together|dig|compare|report back|hunt)\b",
        r"|\bi'm (?:on it|looking|searching|checking|preparing|putting together|digging)\b|\blet me (?:look|search|check|find|dig)\b",
        r"|\b(?:give me|in) (?:a few|a couple of|\d+) minutes\b",
        // Spanish
        r"|\b(?:te )?(?:preparo|busco|vuelvo)\b|\bvoy a (?:buscar|mirar|preparar|comparar)\b|\ben unos minutos\b",
        // German
        r"|\bich (?:suche|schaue|bereite|melde mich|k[üu]mmere mich)\b|\bin (?:ein paar|wenigen) minuten\b",
        // Dutch
        r"|\bik (?:zoek|kijk|ga .{0,30}zoeken|kom .{0,30}terug)\b|\bover een paar minuten\b",
    ))
    .expect("valid regex")
});

/// The person is asked to say or choose something (lower case; apostrophes made straight first): the work waits for
/// their answer, it was offered, not promised. Bench 13/11 (DeepSeek): « Dis-moi lequel et je m'y mets tout de suite ».
static ASKS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\b(?:dis-moi|dites-moi|tu me dis|choisis|pr[ée]cise-moi|r[ée]ponds-moi|confirme-moi|si tu veux|si tu pr[ée]f[èe]res",
        r"|tell me|let me know|pick one|choose|if you(?:'d)? (?:like|want|prefer)",
        r"|dime|av[íi]same|sag mir|lass mich wissen|laat me weten|zeg maar)\b",
    ))
    .expect("valid regex")
});

/// A tool's bare name (`mcp__bipagents__start_task`, `mcp_stealth_navigate` → `start_task`, `navigate`).
fn bare(tool: &str) -> &str {
    let tool = crate::loop_brake::bare(tool);
    tool.rsplit("__").next().unwrap_or(tool)
}

/// The turn did no work: every tool it called is a talking one (none at all included).
pub fn did_nothing<'a>(tools: impl IntoIterator<Item = &'a str>) -> bool {
    tools.into_iter().all(|t| TALK_TOOLS.contains(&bare(t)))
}

/// The reply's end announces work to come, and does not wait for the person (a question, an offer).
pub fn promises(reply: &str) -> bool {
    let text = reply.trim();
    if text.is_empty() {
        return false;
    }
    let chars: Vec<char> = text.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(TAIL_CHARS)..].iter().collect();
    let tail = tail.replace(['\u{2019}', '\u{02BC}'], "'");
    // it asks the person something (« Tu veux que je… ? », « Dis-moi lequel et je m'y mets »): they answer first
    if tail.contains('?') || ASKS.is_match(&tail) {
        return false;
    }
    PROMISE.is_match(&tail)
}

/// The continuation's input (in English, like the brake's: the agent answers in its person's language).
pub fn input(reply: &str) -> String {
    let said: String = reply.trim().chars().rev().take(160).collect::<Vec<_>>().into_iter().rev().collect();
    format!(
        "{MARK}\n\n(A note from the app, not from your person.) Your last answer announced work (« {said} ») but called \
no tool, so your turn ended there: nothing happens after an answer until your person writes again. Do it now, in this \
answer, with your tools (search, browse, pages…), or start it as a background task with start_task and say so in one \
sentence. Don't apologise and don't mention this note. If something is really missing to do it, ask your person one \
precise question."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alex_s_two_replies_promise() {
        // 13/11, Vie, the two turns that ended with no tool call
        assert!(promises("Je te prépare 2-3 trouvailles pour 5 €."));
        assert!(promises(
            "Je regarde Sephora, Marionnaud et Nocibé, et je te dis où le panier tombe à ~5 €.\n\n- Sephora : miniatures\n\
             - Marionnaud : échantillons\n\nJe reviens dans quelques minutes avec le lien et le meilleur prix."
        ));
        assert!(promises("I'll look for two or three ideas around 5 € and get back to you."));
        assert!(promises("Voy a buscar dos o tres ideas."));
        assert!(promises("Ich suche dir zwei, drei Ideen heraus."));
        assert!(promises("Ik zoek een paar ideeën voor je."));
    }

    #[test]
    fn a_question_an_answer_or_small_talk_is_no_promise() {
        assert!(!promises("Je te prépare le panier sur Carrefour ?"));
        assert!(!promises("Tu veux que je cherche d'autres idées ?**"));
        assert!(!promises("Voici trois idées à 5 € : un carnet, des stylos, un livre de poche. Bon test !"));
        assert!(!promises("Avec plaisir, bonne soirée !"));
        assert!(!promises(""));
        // « regarde » of the person, not the agent's
        assert!(!promises("Regarde la carte cadeau Fnac à 5 €, elle est parfaite pour un test."));
        // the bench's offers (13/11, DeepSeek V4.1 Flash): the work waits for the person's choice
        assert!(!promises(
            "Dis-moi lequel et je m'y mets tout de suite :\n\nUn article de maison | Un livre de poche | Autre chose"
        ));
        assert!(!promises("Tu me dis l'envie et le montant max, et je te trouve deux ou trois options concrètes."));
        assert!(!promises("Si tu veux, je te prépare le panier jusqu'à la page de paiement."));
        assert!(!promises("Which one do you like? I'll prepare the cart then."));
    }

    #[test]
    fn only_the_end_counts() {
        let long = format!("Je cherche souvent ce genre de choses. {}Voilà la liste complète.", "Une idée. ".repeat(60));
        assert!(!promises(&long));
    }

    #[test]
    fn a_turn_did_nothing_with_only_talking_tools() {
        assert!(did_nothing([]));
        assert!(did_nothing(["clarify", "todo_list", "mcp__bipagents__notebook_search"]));
        assert!(!did_nothing(["clarify", "web_search"]));
        assert!(!did_nothing(["mcp__bipagents__start_task"]));
        assert!(!did_nothing(["mcp_stealth_navigate"]));
        assert!(!did_nothing(["mcp__stealth__navigate"]));
    }

    #[test]
    fn the_input_is_hidden_and_quotes_the_promise() {
        let text = input("Je reviens dans quelques minutes avec le lien.");
        assert!(text.starts_with(MARK));
        assert!(text.contains("« Je reviens dans quelques minutes avec le lien. »"));
        assert!(text.contains("start_task"));
    }
}
