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

/// Tools that do no work for the person: a turn that called only these did nothing it could announce (`todo`: the task
/// list's other name, steps.rs `is_todo_call`).
pub const TALK_TOOLS: [&str; 11] = [
    "clarify",
    "todo_list",
    "todo",
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

/// Work announced for later, in the five languages of the agents (lower case; apostrophes made straight first): the
/// agent's own verb in the first person, never a time alone (« prêt dans 20 minutes », « in a few minutes » of a recipe)
/// nor a polite closing line (« I'm looking forward… », « Ik kijk ernaar uit… »).
static PROMISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)",
        // French
        r"\bje (?:te |t'|vous )?(?:pr[ée]pare|cherche|regarde|v[ée]rifie|lance|compare|fouille|d[ée]gote|d[ée]niche|envoie|reviens|m'en occupe|m'y mets|m'en charge)\b",
        r"|\bj'envoie\b|\bje (?:te |vous )fais (?:une|un|la|le|les|ta|ton|tes)\b",
        r"|\bje vais (?:te |t'|vous )?(?:chercher|regarder|trouver|comparer|voir|v[ée]rifier|fouiller|lancer|pr[ée]parer|d[ée]nicher|envoyer|m'en occuper)\b",
        r"|\bje vais (?:te |t'|vous )?faire (?:quelques|des|une|un|ta|ton|tes|la|le|les) (?:petites? )?(?:recherches?|s[ée]lection|liste|comparatif|tour)\b",
        r"|\bje (?:te |vous )?(?:tiens au courant|recontacte|reviens)\b",
        r"|\blaisse-moi (?:chercher|regarder|v[ée]rifier)\b",
        r"|\b(?:je|j'y) jette un (?:petit )?(?:coup d'(?:œ|oe)il|(?:œ|oe)il)|\bje (?:te |vous )fais (?:ça|signe)\b|\bje m'occupe de\b",
        r"|\bdonne-moi (?:un instant|une minute|une seconde|deux secondes|(?:quelques|deux|trois|cinq|\d+) minutes)\b",
        // English
        r"|\bi(?:'ll| will) (?:look (?:for|into|up|around)|search|check|find|get back|come back|be back|get [^.!?\n]{0,30}ready|prepare|put together|pull together|send|dig|compare|research|report back|hunt)\b",
        r"|\bi'm (?:on it|looking (?:for|into|up)|searching|checking|preparing|putting together|digging)\b",
        r"|\blet me (?:look|search|check|find|dig|take a look|put together|pull together|prepare|get (?:you|that|this|it))\b",
        r"|\bgive me (?:a (?:moment|sec|second|minute)|(?:a few|a couple of|\d+) minutes)\b",
        r"|\bi(?:'ll| will) (?:(?:take|have) a (?:quick )?look|let you know|keep you (?:posted|updated))\b",
        r"|\bi'm (?:going to|gonna) (?:look|search|check|find|prepare|put together|pull together|send|compare|dig)\b",
        // Spanish
        r"|\b(?:te )?(?:preparo|busco)\b|\bvuelvo (?:con|en)\b|\bvoy a (?:buscar|mirar|preparar|comparar)\b",
        // German
        r"|\bich (?:suche|schaue|bereite|melde mich|k[üu]mmere mich)\b",
        // Dutch
        r"|\bik (?:zoek|kijk (?:even|het na|rond|voor je)|ga .{0,30}zoeken|kom .{0,30}terug)\b",
    ))
    .expect("valid regex")
});

/// A link, whose `?` asks nothing.
static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://\S+").expect("valid regex"));

/// Words that are not the agent's own to its person: the hidden spoken version (`<!--bip:say …-->`), a block (a
/// ```draft to send, code).
static HIDDEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->|```.*?(?:```|$)").expect("valid regex"));

/// Quoted words (an example, a line of a story, a quoted message): someone else's « je cherche ».
static QUOTED: LazyLock<Regex> = LazyLock::new(|| {
    // a straight quote opens after a space, a bracket or the line's start, and closes before a non-letter (never
    // inches: « la TV 55" … la 65" »)
    Regex::new(r#"(?m)«[^»]*»|“[^”]*”|(?:^|[\s(\[])"[^"\n]*"(?:$|[^\w])|^[ \t]*>.*$"#).expect("valid regex")
});

/// A letter's closing line (a reply drafted for the person to send): what comes before it is the draft's.
static SIGNED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?mi)^[ \t]*(?:bien (?:cordialement|à (?:vous|toi))|cordialement|bien amicalement|amicalement|amitiés|bisous|bises",
        r"|best(?: regards| wishes)?|kind regards|warm regards|regards|sincerely|yours|saludos(?: cordiales)?|atentamente",
        r"|un (?:abrazo|saludo)|mit freundlichen grüßen|viele grüße|liebe grüße|beste grüße|met vriendelijke groet(?:en)?",
        r"|hartelijke groet(?:en)?|groetjes)[ \t]*[,.]?[ \t]*$",
        r"|(?mi)^[ \t]*(?:merci(?: encore| beaucoup)?|thanks|thank you|many thanks|cheers|à bientôt|à plus|bonne (?:journée|soirée)",
        r"|see you|gracias|danke|bedankt)[ \t]*,[ \t]*$",
    ))
    .expect("valid regex")
});

/// The agent's own words to its person: without its hidden notes and blocks, a drafted letter (up to its signature),
/// quoted words (unless the whole reply is quoted) and links.
fn own_words(reply: &str) -> String {
    let text = reply.replace(['\u{2019}', '\u{02BC}'], "'");
    let mut text = HIDDEN.replace_all(&text, "").to_string();
    if let Some(signed) = SIGNED.find_iter(&text).last() {
        let mut rest = &text[signed.end()..];
        // the signature's name line (short, no sentence) belongs to the draft too
        if let Some(line) = rest.lines().find(|l| !l.trim().is_empty()) {
            let line = line.trim();
            if line.chars().count() <= 40 && !line.contains(['.', '!', '?']) {
                rest = rest.split_once(line).map_or(rest, |(_, after)| after);
            }
        }
        text = rest.to_string();
    }
    let unquoted = QUOTED.replace_all(&text, "");
    if unquoted.chars().any(char::is_alphabetic) {
        text = unquoted.to_string();
    }
    LINK.replace_all(&text, "").to_string()
}

/// What follows « : » gives an answer: a computation, a yes or no, a sentence with its verb (« le 14 juillet tombe un
/// mercredi », « it costs 4,99 € »).
static ANSWERED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)^\s*(?:oui|non|yes|no)\b|=",
        r"|\b(?:est|sont|c'est|tombe|tombent|co[uû]te|co[uû]tent|vaut|ouvre|ferme|is|are|falls|costs|opens|closes|was|were)\b",
    ))
    .expect("valid regex")
});

/// A promise followed on its line by `:` and the answer it gives (« Let me check the math: 3 × 4.99 € = 14.97 € »,
/// a check's « Je vérifie : le 14 juillet 2027 tombe un mercredi »): done. What only describes the work to come (« Je regarde
/// trois enseignes : Sephora, Marionnaud et Nocibé ») is not.
fn delivered(said: &str, after: &str) -> bool {
    let line = after.lines().next().unwrap_or("");
    let checks = CHECKS.is_match(said);
    let Some((head, given)) = line.split_once(':') else { return false };
    let checks = checks && (!said.to_lowercase().ends_with("compare") || head.trim().is_empty());
    if given.contains('=') || (checks && ANSWERED.is_match(given)) {
        return true;
    }
    // something made, given on the lines its colon opens (« Je te fais un petit récap :\n\n- Budget : 5 € … »); a
    // search's list is its plan (« Je regarde Sephora et Nocibé :\n- miniatures… »)
    given.trim().is_empty()
        && MAKES.is_match(said)
        && HOLDS.is_match(head)
        && after.lines().skip(1).any(|l| !l.trim().is_empty())
}

/// Work that makes something the reply can hold (a recap, a list, a programme), not a search.
static MAKES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)pr[ée]pare|fais|put together|pull together|make|draft|write").expect("valid regex")
});

/// A check announced, whose answer may follow (« Je vérifie : … tombe un mercredi »); any other promise followed by
/// « : c'est parfait pour un achat test » still announces work.
static CHECKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)v[ée]rifie|check|compare").expect("valid regex"));

/// « je cherche (juste) à comprendre », « je reviens sur ta question »: the words, not work to come.
static SEEKS_TO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:(?:juste|simplement|seulement|surtout|vraiment)\s+)?à\b").expect("valid regex")
});
static ON_TOPIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*sur\b").expect("valid regex"));

/// What one sends or gives out of care (« je t'envoie plein de courage », « je te fais un gros câlin »): no work.
static CARE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)^\s*(?:(?:un|une|des|plein de|toute?s?|tout|mes|ma|mon|de la|du|de|mille|beaucoup de)\s+)*",
        r"(?:(?:gros|grosse|grand|grande|petit|petite|énorme|immense|bonnes?|bons?|douces?|tendres?)\s+)*",
        r"(?:c[âa]lins?|bisous?|bises?|courage|force|ondes|pensées|amitiés|amour|tendresse|soutien|énergie|accolade",
        r"|sourires?|clin d'(?:œ|oe)il|vibes?|hugs?|love)\b",
    ))
    .expect("valid regex")
});

fn figurative(said: &str, after: &str) -> bool {
    let said = said.to_lowercase();
    (said.ends_with("cherche") && SEEKS_TO.is_match(after))
        || (said.ends_with("reviens") && ON_TOPIC.is_match(after))
        || ((said.ends_with("envoie") || said.contains(" fais ")) && CARE.is_match(after))
}

/// Words to the person (a question with « tu », « you »…): a `?` there asks them something; a bare echo (« 5 € ? »)
/// does not.
static TO_THEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:tu|te|t'|toi|ton|ta|tes|vous|votre|vos|you|your|dir|dich|dein\w*|jij|jou|jouw|tú|ti|usted)\b|\bt'",
    )
    .expect("valid regex")
});

/// What acknowledges an echo of the person's words (« 5 € ? Parfait, … »).
static ACKNOWLEDGED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)^\s*(?:parfait|top|super|g[ée]nial|tr[èe]s bien|(?:bonne|tr[èe]s bonne|excellente) id[ée]e|ok|okay|d'accord",
        r"|entendu|not[ée]|c'est not[ée]|ça marche|avec plaisir|pas de (?:souci|soucis|probl[èe]me)|bien s[ûu]r|volontiers",
        r"|perfect|great|sure|of course|absolutely|certainly|no problem|got it|noted|sounds good|happy to",
        r"|perfecto|vale|claro|alles klar|gerne|prima)\b",
    ))
    .expect("valid regex")
});

/// A question to the person: after the promise starts (« Je te prépare le panier sur Carrefour ? »), before it in words
/// to them (« Which one do you like? I'll prepare the cart then ») or right before its sentence (« Taille M ou L ? Je
/// lance la commande »), but an echo of their words the agent acknowledges (« 5 € ? Parfait, je te prépare… ») and a
/// question it answers itself (« Pourquoi ces trois enseignes ? Ce sont… »).
fn asks_a_question(tail: &str, at: usize) -> bool {
    tail.match_indices('?').any(|(i, _)| {
        if i >= at {
            return true;
        }
        let start = tail[..i].rfind(['.', '!', '?', '\n']).map_or(0, |s| s + 1);
        // right before the promise's sentence (« Taille M ou L ? Je lance la commande »), unless acknowledged
        let next = !tail[i + 1..at].trim_start().contains(['.', '!', '?', '\n']);
        TO_THEM.is_match(&tail[start..i]) || (next && !ACKNOWLEDGED.is_match(&tail[i + 1..]))
    })
}

/// The person is asked to say or choose something (lower case; apostrophes made straight first): the work waits for
/// their answer, it was offered, not promised. Bench 13/11 (DeepSeek): « Dis-moi lequel et je m'y mets tout de suite ».
/// Followed by « si / if / ob / of » it is optional (« Dis-moi si tu as une enseigne préférée »): the work does not wait.
static ASKS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\b(?:dis-moi|dites-moi|tu me dis|choisis|pr[ée]cise-moi|r[ée]ponds-moi|confirme-moi",
        r"|envoie-moi|montre-moi|partage-moi|transmets-moi",
        r"|donne-moi (?:ton|ta|tes|les|l'|la (?:liste|taille|r[ée]f[ée]rence|date|couleur)|le (?:lien|nom|budget|num[ée]ro|code|prix|mod[èe]le|montant))",
        r"|tu (?:me )?(?:confirmes|valides)|give me the (?:green light|go-ahead)|the go-ahead|say (?:yes|the word)",
        r"|d[èe]s que tu (?:me |m')?(?:confirmes|valides|envoies|donnes|dis|r[ée]ponds)",
        r"|tell me|let me know|pick one|choose|send me|share (?:it|the|your)|once you|as soon as you",
        r"|dime|av[íi]same|sag mir|lass mich wissen|laat me weten|zeg maar)\b",
    ))
    .expect("valid regex")
});

/// An optional request (« dis-moi si… », « let me know if… »).
static OPTIONAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[\s,]*(?:si|s'|if|whether|ob|of)\b").expect("valid regex"));

/// The work offered on the person's condition (« Si tu veux, je te prépare le panier », « Je te prépare la version
/// imprimable si tu veux ») before the promise or in its sentence; in a later sentence (« … Si tu préfères un autre
/// site, dis-le-moi »), the promise stands.
static OFFER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\b(?:si tu (?:veux|pr[ée]f[èe]res|le souhaites|es d'accord|valides|confirmes)|si vous (?:voulez|pr[ée]f[ée]rez|[êe]tes d'accord)",
        r"|si (?:ça|cela|c'est bon pour) (?:te|vous|toi)|si (?:ça|cela) te (?:va|convient)|si c'est bon pour (?:toi|vous)",
        r"|d[èe]s (?:ton|votre) (?:feu vert|accord|ok)|quand tu (?:veux|voudras|me (?:le )?dis)|avec ton (?:accord|feu vert)",
        r"|if you(?:'d)? (?:like|want|prefer)|if you(?:'re| are) (?:ok|okay|happy|fine)|if you agree",
        r"|if (?:it|this|that) (?:looks good|works|suits)|once you(?:'re| are) happy|whenever you(?:'re| are) ready",
        r"|si quieres|wenn du (?:willst|m[öo]chtest)|als je (?:wilt|wil))\b",
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
    let text = own_words(reply);
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    let chars: Vec<char> = text.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(TAIL_CHARS)..].iter().collect();
    // the last words announcing work (not an answer given after « : », not « je cherche à comprendre »)
    let Some(last) = PROMISE
        .find_iter(&tail)
        .filter(|m| !delivered(m.as_str(), &tail[m.end()..]) && !figurative(m.as_str(), &tail[m.end()..]))
        .last()
    else {
        return false;
    };
    let at = last.start();
    // a question to the person (« Tu veux que je… ? »): they answer first
    if asks_a_question(&tail, at) {
        return false;
    }
    // a request they answer first (« Dis-moi lequel et je m'y mets », « Envoie-moi le lien et je regarde », « Dis-moi si
    // ça te va et je lance la commande »), but an optional line after the promise (« Dis-moi si tu as une enseigne
    // préférée ») and a statement (« tu choisis celle qui te plaît », « so you can choose »)
    let request = |m: &regex::Match| {
        let before: Vec<String> = tail[..m.start()].split_whitespace().rev().take(2).map(str::to_lowercase).collect();
        let said = before.first().map(String::as_str).unwrap_or("");
        let help = before.len() == 2 && before[1] == "help" && said == "you";
        // « so you can choose », « to help you choose », « pour t'aider à choisir »: no request
        if help || (m.start() >= at && ["can", "could", "to", "peux", "pourras"].contains(&said)) {
            return false;
        }
        if m.start() < at {
            return true; // before the work: it waits for them (« Tu choisis et je lance la commande »)
        }
        // after it, a choice among what it brings (« tu choisis celle qui te plaît », « tu valides celle… »)
        let statement = m.as_str().to_lowercase().starts_with("tu ");
        let choice = said == "tu" || said == "you" || (statement && CHOICE.is_match(&tail[m.end()..]));
        let end = tail[m.end()..].find(['.', '!', '?', '\n']).map_or(tail.len(), |e| m.end() + e);
        let sentence = &tail[m.end()..end];
        // optional (« Dis-moi si tu as une enseigne préférée », « Envoie-moi un message si tu as besoin… »), unless it
        // asks for their yes (« Dis-moi juste si c'est bon pour toi »)
        let optional = (OPTIONAL.is_match(sentence) || NEED.is_match(sentence) || IF.is_match(sentence))
            && !YES.is_match(sentence);
        !choice && !optional
    };
    if ASKS.find_iter(&tail).any(|m| request(&m)) {
        return false;
    }
    // the work offered on their condition in the promise's sentence (« Si tu veux, je te prépare le panier », « Je te
    // prépare une version imprimable si tu veux ! », « If that works for you, I'll check out »): it waits for their yes
    !OFFER.find_iter(&tail).any(|m| {
        let (from, to) = if m.start() < at { (m.end(), at) } else { (at, m.start()) };
        !tail[from..to].contains(['.', '!', '?', '\n'])
    })
}

/// A request after the promise made optional by its own condition (« Envoie-moi un message si tu as besoin d'autre
/// chose », « Send me your address if you want delivery »).
static NEED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:si (?:tu as besoin|besoin|tu veux|tu en veux|tu pr[ée]f[èe]res)|if you (?:need|want|prefer|'d like))\b")
        .expect("valid regex")
});

/// Their yes asked (« si c'est bon pour toi », « if this works for you »): no optional line.
static YES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\bsi (?:c'est bon|ça te va|ça te convient|cela te convient|tu es d'accord|tu valides|tu confirmes|ça marche)",
        r"|\bif (?:it|this|that) (?:looks good|works|suits)|\bif you(?:'re| are) (?:ok|okay|happy|fine)|\bif you agree",
    ))
    .expect("valid regex")
});

/// A request after the promise with its own condition (« Dis-moi aussi si tu as une enseigne préférée »): optional.
static IF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(?:si|s'il|if)\b").expect("valid regex"));

/// What a reply can hold, given under its colon (« Je te fais un petit récap : »), unlike the finds of a search
/// (« Je te prépare 2-3 trouvailles pour 5 € :\n- Sephora… », its plan).
static HOLDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:r[ée]cap\w*|r[ée]sum[ée]|liste|programme|planning|menu|tableau|plan|recette|summary|list|recap|rundown|overview|schedule|recipe)\b").expect("valid regex")
});

/// After the promise, the person picks among what it brings (« tu valides celle qui te plaît »).
static CHOICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:celle|celui|ceux|celles|laquelle|lequel|lesquel(?:le)?s|one|which)\b").expect("valid regex")
});

/// The continuation's input (in English, like the brake's: the agent answers in its person's language).
pub fn input(reply: &str) -> String {
    let said: String = reply.trim().chars().rev().take(160).collect::<Vec<_>>().into_iter().rev().collect();
    format!(
        "{MARK}\n\n(A note from the app, not from your person.) Your last answer announced work (« {said} ») but called \
no tool, so your turn ended there: nothing happens after an answer until your person writes again. Do it now, in this \
answer, with your tools (search, browse, pages…), or start it as a background task with start_task and say so in one \
sentence. Don't apologise and don't mention this note. If something is really missing to do it, ask your person one \
precise question. If your last answer was in fact waiting for your person (their choice, a detail, their go-ahead), \
don't act: say in one short sentence that you wait for them. Never send, order, book or pay because of this note."
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
    fn a_recipe_a_breath_or_a_closing_line_is_no_promise() {
        // review 13/11: a time alone, or a polite closing line, announces no work of the agent's
        let said = [
            "Ajoute la crème, couvre et laisse mijoter à feu doux : ce sera prêt dans 20 minutes. Bon appétit !",
            "Respire lentement par le ventre, tu vas te sentir plus calme dans un instant.",
            "Put it in the oven at 180 °C: it'll be ready in 25 minutes. Enjoy!",
            "What a lovely plan! I'm looking forward to hearing all about your trip.",
            "La pasta estará lista en unos minutos. ¡Buen provecho!",
            "Die Nudeln sind in ein paar Minuten fertig. Guten Appetit!",
            "Wat een leuk plan! Ik kijk ernaar uit om te horen hoe het was.",
            "Vuelvo a decirte que fue un placer. ¡Buen viaje!",
        ];
        let taken: Vec<&str> = said.iter().copied().filter(|s| promises(s)).collect();
        assert!(taken.is_empty(), "taken for a promise of work: {taken:#?}");
        // still promises: the agent's own verb
        assert!(promises("Donne-moi quelques minutes, je reviens avec le lien."));
        assert!(promises("I'm looking into it, give me a few minutes."));
        assert!(promises("Ich melde mich in ein paar Minuten mit dem Link."));
        assert!(promises("Ik kijk even rond en kom zo bij je terug."));
        assert!(promises("Vuelvo en unos minutos con el enlace."));
    }

    #[test]
    fn a_draft_a_story_or_an_example_is_no_promise() {
        // re-review 13/11: words the agent writes for someone else, quotes, or the work given at once after a colon
        let said = [
            "Voici une proposition de réponse :\n\nBonjour Madame Durand,\n\nMerci pour votre message. Je regarde \
             votre dossier ce week-end et je reviens vers vous lundi.\n\nBien cordialement,\nAlex",
            "Here's a reply you can send:\n\nHi Sarah,\n\nThanks for flagging this. I'll look into it and get back \
             to you by Friday.\n\nBest,\nAlex",
            "Le petit renard leva les yeux vers la lune et murmura : « Je cherche mon chemin. » La lune lui sourit, et \
             il s'endormit. Fin. Bonne nuit !",
            "En espagnol, « Je cherche un cadeau pas cher » se dit « Busco un regalo barato ». Bon courage pour ton cours !",
            "Let me check the math: 3 × 4.99 € = 14.97 €, so you stay under your 15 € budget.",
            "Je regarde : 3 × 4,99 € = 14,97 €, tu restes bien sous les 15 €.",
            "Voilà le texto :\n\n```draft\nSalut Léa, je regarde ça ce soir et je te redis demain !\n```",
            "C'est noté.\n<!--bip:say Je te prépare ça plus tard si tu veux.-->",
        ];
        let taken: Vec<&str> = said.iter().copied().filter(|s| promises(s)).collect();
        assert!(taken.is_empty(), "taken for a promise of work: {taken:#?}");
        // still promises: the agent's own words around a quote, after a draft, or a quoted whole reply
        assert!(promises("J'ai noté « carnet » et « stylo ». Je te prépare 2-3 trouvailles pour 5 €."));
        assert!(promises("« Je te prépare 2-3 trouvailles pour 5 €. »"));
        assert!(promises(
            "Voici le brouillon :\n\nBonjour,\nMerci.\n\nCordialement,\nAlex\n\nPendant ce temps je regarde les prix \
             sur la Fnac."
        ));
        assert!(promises(
            "Je regarde Sephora et Nocibé :\n\n- miniatures\n- échantillons\n\nJe reviens dans 5 minutes."
        ));
    }

    #[test]
    fn a_promise_followed_by_a_colon_inches_or_an_optional_line_is_still_one() {
        // third review 13/11: what follows « : » only describes the work; inches are no quote; « dis-moi si… » asks
        // nothing the work waits for
        let said = [
            "Je te prépare ça : 2-3 trouvailles autour de 5 € avec les liens.",
            "Je regarde trois enseignes : Sephora, Marionnaud et Nocibé.",
            "Je reviens dans quelques minutes : le temps de trouver le lien et le meilleur prix.",
            "Je lance la recherche : miniatures et échantillons autour de 5 €.",
            "I'll check three shops: Sephora, Boots and Superdrug.",
            "Pour la TV 55\", je te prépare un comparatif avec la 65\" de Boulanger.",
            "Je te prépare 2-3 trouvailles pour 5 €. Dis-moi si tu as une enseigne préférée.",
            "I'll look for two or three finds around €5 and get back to you. Let me know if you have a favourite shop.",
            "Je regarde Sephora et Nocibé et je reviens vers toi avec le lien. Si tu préfères un autre site, dis-le-moi.",
            "Je vérifie les stocks et les prix, je te dis ça dans quelques minutes.",
            "Donne-moi un instant, je te fais une petite sélection à 5 €.",
            "Give me a moment to pull together a few options under €5.",
            "I'll send you the link and the best price in a few minutes.",
        ];
        let missed: Vec<&str> = said.iter().copied().filter(|s| !promises(s)).collect();
        assert!(missed.is_empty(), "real promises not caught: {missed:#?}");
        // still no promise: an offer on their condition, a real request, a straight-quoted example
        assert!(!promises("Si tu veux, je te prépare le panier jusqu'à la page de paiement."));
        assert!(!promises("Dis-moi lequel et je m'y mets tout de suite."));
        assert!(!promises("En anglais, \"I'll look into it\" veut dire « je vais regarder ». Bonne soirée !"));
    }

    #[test]
    fn an_offer_a_yes_asked_first_or_what_they_must_send_waits_for_them() {
        // fourth review 13/11: the condition ending the sentence, a request for their yes before the work, what the
        // person must send or confirm first
        let said = [
            "Je te prépare une version imprimable si tu veux !",
            "Je te fais une petite liste de courses si tu veux.",
            "Je te prépare la version sans gluten si tu préfères.",
            "I'll put together a shopping list if you'd like.",
            "Dis-moi si ça te va et je lance la commande.",
            "Dis-moi si ça te convient et je te prépare la liste de courses.",
            "Let me know if this works for you and I'll send the invite.",
            "Envoie-moi le lien et je regarde.",
            "Donne-moi ton budget et je te prépare une sélection.",
            "Send me the link and I'll check it.",
            "Dès que tu me confirmes, je lance la commande.",
            "Je vérifie : le 14 juillet 2027 tombe un mercredi.",
            "Let me check: July 14, 2027 falls on a Wednesday, so you'd get a long weekend.",
            "Désolé si je t'ai vexé, je cherche juste à comprendre ce qui t'irait le mieux.",
            "Je reviens sur ta question : oui, c'est possible avec la carte Navigo.",
            "Voici un message pour Marc :\n\nSalut Marc, je regarde le dossier ce soir et je reviens vers toi demain.\n\n\
             Merci,\nAlex",
            "Here's a reply:\n\nHi Sarah,\n\nI'll look into it and get back to you by Friday.\n\nThanks,\nAlex",
        ];
        let taken: Vec<&str> = said.iter().copied().filter(|s| promises(s)).collect();
        assert!(taken.is_empty(), "taken for a promise of work: {taken:#?}");
        let said = [
            "5 € ? Parfait, je te prépare 2-3 trouvailles.",
            "Un achat test à 5 € ? Bonne idée ! Je te prépare 2-3 trouvailles.",
            "€5? No problem, I'll look for two or three finds and get back to you.",
            "Je te prépare 2-3 trouvailles, tu choisis celle qui te plaît.",
            "I'll pull together 3 options under €5 so you can choose.",
            "Je vais vérifier les disponibilités et les prix.",
            "Je vais t'envoyer le lien et le meilleur prix.",
            "Je vais faire quelques recherches pour toi.",
            "Let me take a look.",
            "Let me put together a few options for you.",
            "I'm going to look for a few options.",
        ];
        let missed: Vec<&str> = said.iter().copied().filter(|s| !promises(s)).collect();
        assert!(missed.is_empty(), "real promises not caught: {missed:#?}");
    }

    #[test]
    fn a_yes_or_a_choice_asked_in_other_words_waits_and_explained_promises_stand() {
        // fifth review 13/11
        let said = [
            "Voici trois options : A, B ou C. Tu choisis et je lance la commande.",
            "Dès que tu choisis, je lance la commande.",
            "OK pour la taille M ? Je lance la commande.",
            "Taille M ou L ? Je lance la commande juste après.",
            "Le panier est prêt : 3 articles, 4,85 € livraison comprise. Si tu es d'accord, je lance la commande.",
            "Le panier est prêt : 3 articles, 4,85 € livraison comprise. Je lance la commande si ça te va.",
            "Le panier est prêt : 3 articles, 4,85 € livraison comprise. Si c'est bon pour toi, je lance la commande.",
            "Le panier est prêt : 3 articles, 4,85 € livraison comprise. Tu me confirmes et je lance la commande.",
            "Le panier est prêt : 3 articles, 4,85 € livraison comprise. Je lance la commande dès ton feu vert.",
            "Le panier est prêt : 3 articles, 4,85 € livraison comprise. Je lance la commande quand tu veux.",
            "Voici le mail pour Marc :\n\nSalut Marc,\n\nOn se voit samedi à 18h ?\n\nAlex\n\nSi ça te va, j'envoie le \
             mail à Marc.",
            "Here's the draft. If it looks good to you, I'll send it right away.",
            "Here's the draft. Just give me the green light and I'll send it.",
            "The cart is ready: 3 items, €4.85 with delivery. If that works for you, I'll check out.",
        ];
        let taken: Vec<&str> = said.iter().copied().filter(|s| promises(s)).collect();
        assert!(taken.is_empty(), "taken for a promise of work (a hidden « do it now »): {taken:#?}");
        let said = [
            "Je te prépare 2-3 trouvailles pour 5 € : c'est parfait pour un achat test.",
            "Je regarde Sephora, Marionnaud et Nocibé : c'est là qu'on trouve le plus de miniatures.",
            "Je reviens dans quelques minutes avec le lien et le meilleur prix : le panier est presque prêt.",
            "Je reviens vers toi dans quelques minutes : la recherche est en cours.",
            "Donne-moi le temps de regarder les prix, je reviens vers toi avec le lien.",
            "Je reviens vers toi avec le lien. Envoie-moi un message si tu as besoin d'autre chose.",
            "I'll look for options and get back to you. Send me your address if you want delivery.",
            "Je regarde Sephora, Marionnaud et Nocibé, et je te dis où le panier tombe à ~5 €.\n\n- Sephora : \
             miniatures\n- Marionnaud : échantillons\n- Nocibé : coffrets, si tu préfères offrir\n\nJe reviens dans \
             quelques minutes avec le lien et le meilleur prix.",
            "I'll research this and get back to you.",
        ];
        let missed: Vec<&str> = said.iter().copied().filter(|s| !promises(s)).collect();
        assert!(missed.is_empty(), "real promises not caught: {missed:#?}");
    }

    #[test]
    fn care_a_recap_given_under_its_colon_and_the_fifth_fix_s_misses() {
        // sixth review 13/11
        let said = [
            "Je suis vraiment désolée que ta journée ait été si difficile. Prends soin de toi ce soir, je t'envoie \
             plein de courage pour demain 💛",
            "Bravo pour ta séance ! Je t'envoie plein de bonnes ondes pour la suite.",
            "Courage pour ton entretien demain, je t'envoie toute ma force !",
            "Ça a l'air d'être une période chargée. Je te fais un gros câlin virtuel 🤗",
            "Je te fais un petit récap :\n\n- Budget : 5 €\n- Enseigne : Sephora\n- Retrait en magasin samedi\n\nOn est \
             bons !",
            "Je te fais la liste de courses :\n\n- 1,2 kg de raclette\n- 1 kg de pommes de terre\n- charcuterie\n- \
             cornichons\n\nBonne soirée !",
            "Je te prépare un petit programme pour la semaine :\n\n- Lundi : 20 min de marche\n- Mercredi : yoga\n- \
             Vendredi : 20 min de marche\n\nTu vas voir, ça passe vite !",
            "Let me put together a quick summary:\n\n- Budget: €5\n- Shop: Sephora\n- Pick-up on Saturday\n\nAll set!",
            "Je compare : la Fnac est moins chère (4,99 € contre 5,49 €).",
            "Je lance la commande. Dis-moi juste si c'est bon pour toi.",
        ];
        let taken: Vec<&str> = said.iter().copied().filter(|s| promises(s)).collect();
        assert!(taken.is_empty(), "taken for a promise of work: {taken:#?}");
        let said = [
            "5 € ? Avec plaisir, je te prépare 2-3 trouvailles.",
            "5 € ? Pas de souci, je te prépare 2-3 trouvailles.",
            "5 € ? Bien sûr ! Je te prépare 2-3 trouvailles.",
            "Un achat test à 5 € ? Excellente idée ! Je te prépare 2-3 trouvailles.",
            "€5? Of course! I'll look for two or three finds.",
            "Pourquoi ces trois enseignes ? Ce sont celles qui ont le plus de miniatures.\n\nJe reviens dans quelques \
             minutes avec le lien et le meilleur prix.",
            "To help you choose, I'll compare three shops and get back to you with the best price.",
            "Je te prépare 2-3 trouvailles, tu valides celle qui te plaît.",
            "Je regarde Sephora, Marionnaud et Nocibé, et je te dis où le panier tombe à ~5 €.\n\n- Sephora : miniatures\n\
             - Marionnaud : échantillons\n\nJe reviens dans quelques minutes avec le lien et le meilleur prix.",
            "Je t'envoie le lien et le meilleur prix dans quelques minutes.",
        ];
        let missed: Vec<&str> = said.iter().copied().filter(|s| !promises(s)).collect();
        assert!(missed.is_empty(), "real promises not caught: {missed:#?}");
    }

    #[test]
    fn the_rules_own_english_promise_is_caught() {
        // souls.rs ENGLISH_EXAMPLES gives « I'll get that ready » for « je te prépare ça »
        assert!(promises("I'll get that ready for you."));
        assert!(promises("I'll get 2-3 finds ready for €5."));
        assert!(promises("Je te déniche ça."));
    }

    #[test]
    fn a_link_s_question_mark_asks_nothing() {
        assert!(promises("J'ai repéré https://www.fnac.com/a123?origin=search, je te prépare le panier dessus."));
        assert!(!promises("Celui-ci te va ? https://www.fnac.com/a123"));
    }

    #[test]
    fn only_the_end_counts() {
        let long =
            format!("Je cherche souvent ce genre de choses. {}Voilà la liste complète.", "Une idée. ".repeat(60));
        assert!(!promises(&long));
    }

    #[test]
    fn a_turn_did_nothing_with_only_talking_tools() {
        assert!(did_nothing([]));
        assert!(did_nothing(["clarify", "todo_list", "mcp__bipagents__notebook_search"]));
        // the task list under its other name (steps.rs `is_todo_call`)
        assert!(did_nothing(["todo", "clarify"]));
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
        // a reply that waited for the person is never acted on through this note
        assert!(text.contains("Never send, order, book or pay because of this note"));
    }
}

#[cfg(test)]
mod seventh_review {
    // seventh review 13/11: regressions of the sixth fix (each passed before it), and common announcements
    use super::promises;

    #[test]
    fn r7_a_question_before_the_order_still_waits_after_a_line_break() {
        // fifth review « OK pour la taille M ? Je lance la commande », « Taille M ou L ? Je lance la commande juste
        // après », with the paragraph break a chat answer puts after its question
        let said = [
            "OK pour la taille M ?\nJe lance la commande.",
            "OK pour la taille M ?\n\nJe lance la commande.",
            "Taille M ou L ?\n\nJe lance la commande juste après.",
        ];
        let taken: Vec<&str> = said.iter().copied().filter(|s| promises(s)).collect();
        assert!(taken.is_empty(), "taken for a promise of work (a hidden « do it now »): {taken:#?}");
    }

    #[test]
    fn r7_which_one_asked_after_the_work_still_waits() {
        // the bench's « Dis-moi lequel et je m'y mets » (DeepSeek, 13/11) the other way round, and the fourth review's
        // « Send me the link and I'll check it » said « You can send me… »
        let said = [
            "Je m'y mets dès que tu me dis lequel.",
            "Je te prépare le panier, dis-moi lequel.",
            "Voici trois options : A (4,99 €), B (5,20 €), C (4,50 €). Je lance la commande dès que tu me dis laquelle.",
            "I'll prepare the cart. Tell me which one.",
            "You can send me the link and I'll check it.",
            "You can tell me which one and I'll prepare the cart.",
        ];
        let taken: Vec<&str> = said.iter().copied().filter(|s| promises(s)).collect();
        assert!(taken.is_empty(), "taken for a promise of work (a hidden « do it now »): {taken:#?}");
    }

    #[test]
    fn r7_promises_the_sixth_fix_lost() {
        let said = [
            // third review « Je te prépare ça : 2-3 trouvailles autour de 5 € avec les liens. », its list on the
            // next lines
            "Je te prépare ça :\n2-3 trouvailles autour de 5 € avec les liens.",
            "Je te prépare ça :\n\n- 2-3 trouvailles autour de 5 €\n- les liens et le meilleur prix",
            // Alex's first words with the plan of his second reply
            "Je te prépare 2-3 trouvailles pour 5 € :\n\n- Sephora : miniatures\n- Marionnaud : échantillons\n- Nocibé : \
             coffrets",
            // fifth review « Je regarde Sephora, Marionnaud et Nocibé : c'est là qu'on trouve… », with « compare »
            "Je compare Sephora, Marionnaud et Nocibé : c'est là qu'on trouve le plus de miniatures.",
            // third review « Dis-moi si tu as une enseigne préférée » after the promise, a word before its « si »
            "Je te prépare 2-3 trouvailles pour 5 €. Dis-moi aussi si tu as une enseigne préférée.",
            "Je te prépare 2-3 trouvailles pour 5 €. Dis-moi juste si tu as une enseigne préférée.",
            "I'll look for two or three finds around €5 and get back to you. Let me know too if you have a favourite shop.",
        ];
        let missed: Vec<&str> = said.iter().copied().filter(|s| !promises(s)).collect();
        assert!(missed.is_empty(), "real promises not caught: {missed:#?}");
    }

    #[test]
    fn r7_common_announcements_are_caught() {
        // not a regression (missed since 2b06140a7): everyday ways to announce a search and stop
        let said = [
            "Je jette un œil aux prix et je te dis ça.",
            "Je te fais ça tout de suite !",
            "Je te fais signe dès que j'ai trouvé.",
            "Je m'occupe de te trouver 2-3 idées à 5 €.",
            "I'll take a look and let you know.",
            "I'll let you know what I find.",
        ];
        let missed: Vec<&str> = said.iter().copied().filter(|s| !promises(s)).collect();
        assert!(missed.is_empty(), "real promises not caught: {missed:#?}");
    }
}
