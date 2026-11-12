//! A turn that ended without an answer, in the person's words. Hermes ends such a turn with its own explanation
//! (« ⚠️ No reply: dgx-spark didn't produce a reply this time, even after retries. Send `continue` to try again, or
//! switch models with /model. »): English most of the time, a model id, a slash command the apps don't have. The
//! bridge swaps it for one plain line in the agent's language before the run's end reaches the apps (the thread, the
//! notification, a Live Activity): owner 2026-10-27.

/// `explainer.no_reply_prefix` in every language of Hermes 517b5e10f, v0.22.5 and v0.23.1 (`locales/*.yaml`, the same
/// in all three), spaces trimmed: every explanation of a turn without an answer starts with one of them.
const HERMES_PREFIXES: [&str; 17] = [
    "⚠️ No reply:",
    "⚠️ Pas de réponse :",
    "⚠️ Keine Antwort:",
    "⚠️ Sin respuesta:",
    "⚠️ Nessuna risposta:",
    "⚠️ Sem resposta:",
    "⚠️ Geen antwoord:",
    "⚠️ Gan freagra:",
    "⚠️ Nincs válasz:",
    "⚠️ Yanıt yok:",
    "⚠️ Немає відповіді:",
    "⚠️ Нет ответа:",
    "⚠️ لا رد:",
    "⚠️ 応答なし:",
    "⚠️ 无回复：",
    "⚠️ 沒有回覆：",
    "⚠️ 응답 없음:",
];

/// Whether `output` is Hermes' explanation of a turn without an answer (not an answer that happens to start with ⚠️).
pub fn is_hermes_no_reply(output: &str) -> bool {
    let text = output.trim_start();
    HERMES_PREFIXES.iter().any(|prefix| text.starts_with(prefix))
}

/// The words of an answer Hermes ended with its explanation (`_explain_abnormal_exit`: `words + "\n\n" +
/// explanation`); None when no explanation follows words. Only Hermes' two cases, so that an agent's own paragraph
/// opening on the same words stays its own: a fragment (24 characters at most, not ended like a sentence: Hermes'
/// `_is_partial_fragment`), or a stream that stopped early (its explanation names `continue`, in backquotes in every
/// language). An output that is only the explanation is `is_hermes_no_reply`.
pub fn words_before(output: &str) -> Option<&str> {
    const SENTENCE_END: [char; 8] = ['.', '!', '?', '。', '！', '？', '`', ')']; // Hermes' `_SENTENCE_END`
    let at = output
        .match_indices("\n\n")
        .map(|(i, _)| i + 2)
        .filter(|&i| HERMES_PREFIXES.iter().any(|prefix| output[i..].starts_with(prefix)))
        .last()?;
    let words = output[..at].trim();
    let fragment = words.chars().count() <= 24 && !words.ends_with(SENTENCE_END);
    (!words.is_empty() && (fragment || output[at..].contains('`'))).then(|| output[..at].trim_end())
}

/// What the person reads of an output that ends on Hermes' explanation: the plain line alone, or after the words that
/// came before it. None: an answer, kept as it is.
pub fn in_plain_words(output: &str, name: &str, lang: &str) -> Option<String> {
    if is_hermes_no_reply(output) {
        return Some(plain(name, lang));
    }
    words_before(output).map(|words| format!("{words}\n\n{}", plain(name, lang)))
}

/// The line the person reads instead, for agent `name`, in `lang` (fr, en, de, es; others read English).
pub fn plain(name: &str, lang: &str) -> String {
    match lang {
        "fr" => format!("{name} n'a pas réussi à répondre cette fois. Réessaie, ou écris « continue »."),
        "de" => format!("{name} konnte diesmal nicht antworten. Versuch es noch einmal oder schreib „weiter“."),
        "es" => format!("{name} no ha podido responder esta vez. Vuelve a intentarlo o escribe «continúa»."),
        _ => format!("{name} couldn't answer this time. Try again, or write “continue”."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hermes_explanations_are_recognised_and_answers_are_not() {
        assert!(is_hermes_no_reply(
            "⚠️ No reply: dgx-spark didn't produce a reply this time, even after retries. Send `continue` to try \
             again, or switch models with /model."
        ));
        assert!(is_hermes_no_reply("\n⚠️ Pas de réponse : le modèle n'a rien produit."));
        assert!(is_hermes_no_reply("⚠️ 无回复：模型没有回复。"));
        assert!(!is_hermes_no_reply("⚠️ Attention : il va pleuvoir demain, prends un parapluie."));
        assert!(!is_hermes_no_reply("No reply from the dentist yet."));
    }

    #[test]
    fn an_explanation_after_a_fragment_is_told_in_plain_words_too() {
        let partial = "Je regarde ta nuit et\n\n⚠️ No reply: streaming stopped early and only a partial response was \
                       recovered. Send `continue` to resume from where it stopped.";
        assert!(!is_hermes_no_reply(partial));
        assert_eq!(words_before(partial), Some("Je regarde ta nuit et"));
        assert_eq!(
            in_plain_words(partial, "Wellness", "fr").as_deref(),
            Some("Je regarde ta nuit et\n\nWellness n'a pas réussi à répondre cette fois. Réessaie, ou écris « continue ».")
        );
        assert_eq!(
            in_plain_words("⚠️ Pas de réponse : le modèle n'a rien produit.", "Vie", "en").as_deref(),
            Some("Vie couldn't answer this time. Try again, or write “continue”.")
        );
        // an answer, one that warns, one that quotes Hermes mid-line: as they are
        for said in ["Voilà.", "⚠️ Attention : il pleut.", "Hermes écrit « ⚠️ No reply: » quand rien ne vient."] {
            assert_eq!(in_plain_words(said, "Vie", "fr"), None, "{said}");
        }
        // an agent's own paragraph on the same words, after a whole report: its own
        let report = "Relances de la semaine : la banque a répondu.\n\n⚠️ Pas de réponse : Mme Dupont (relancée lundi).";
        assert_eq!(words_before(report), None);
        // a fragment ended like a sentence is an answer too
        assert_eq!(words_before("C'est fait.\n\n⚠️ No reply: the model provider didn't answer."), None);
    }

    #[test]
    fn the_line_is_plain_in_each_language() {
        for lang in ["fr", "en", "de", "es", "nl"] {
            let line = plain("Wellness", lang);
            assert!(line.starts_with("Wellness "), "{lang}");
            assert!(!line.contains("/model") && !line.contains('`'), "{lang}");
        }
        assert_eq!(plain("Wellness", "fr"), "Wellness n'a pas réussi à répondre cette fois. Réessaie, ou écris « continue ».");
    }
}
