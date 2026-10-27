//! A turn that ended without an answer, in the person's words. Hermes ends such a turn with its own explanation
//! (« ⚠️ No reply: dgx-spark didn't produce a reply this time, even after retries. Send `continue` to try again, or
//! switch models with /model. »): English most of the time, a model id, a slash command the apps don't have. The
//! bridge swaps it for one plain line in the agent's language before the run's end reaches the apps (the thread, the
//! notification, a Live Activity): owner 2026-10-27.

/// `explainer.no_reply_prefix` in every language of Hermes 517b5e10f (`locales/*.yaml`), spaces trimmed: every
/// explanation of a turn without an answer starts with one of them.
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
    fn the_line_is_plain_in_each_language() {
        for lang in ["fr", "en", "de", "es", "nl"] {
            let line = plain("Wellness", lang);
            assert!(line.starts_with("Wellness "), "{lang}");
            assert!(!line.contains("/model") && !line.contains('`'), "{lang}");
        }
        assert_eq!(plain("Wellness", "fr"), "Wellness n'a pas réussi à répondre cette fois. Réessaie, ou écris « continue ».");
    }
}
