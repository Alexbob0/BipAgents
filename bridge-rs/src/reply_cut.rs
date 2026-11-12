//! A reply cut by the model's own length limit, said plainly (Alex 12/11: an agent writing a long plan into a file
//! said « La limite du pont est plus basse que prévu » and went on through the terminal; the bridge has no such limit).
//!
//! When the model's reply stops at its output limit in the middle of a tool call (`finish_reason: length`, the
//! arguments cut), Hermes retries the call four times with a bigger `max_tokens`, then ends the turn without running it
//! (`agent/turn_truncation.py`, v0.23.1): the run fails (`partial`) with its English copy (« …it hit its output length
//! limit… raise max_tokens for this model. »), and the transcript ends on its « [System: Your previous response was
//! truncated…] » nudge, with no word of what to do. Measured 2026-11-12 (Hermes v0.23.1, GLM-5.3 Flash at EUrouter,
//! a cap kept at 3 000 tokens): exactly that. The model, on its next turn, guesses: a « payload » too big, then the
//! bridge's limit.
//!
//! The bridge, at the run's end (runs.rs `dispatch`), recognises Hermes' copies ([`kind`]) and:
//! - shows the person one plain line in the agent's language instead ([`person_line`]), marked `bip_reply_cut`;
//! - gives the agent, with the person's next message in that conversation (live_brief.rs `message_notes`), a hidden
//!   note ([`take_note`]): the model's limit cut it, not BipAgents; write the file in parts (a first part, then the
//!   rest appended) or sum up.
//!
//! The copies of a stream the provider (or a proxy) closed mid tool-call are the same case for the agent (« in
//! smaller pieces »), told as cut on the way, at the model's provider.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// Why the reply was cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cut {
    /// The model's output length limit (Hermes' `truncated` and `truncated_unreported`).
    Length,
    /// The provider or a proxy closed the stream mid tool-call (`stream_closed_tool_call`, `stream_dropped_tool_call`).
    Stream,
}

/// Hermes v0.23.1 `agent/turn_failure_copy.py`: a part of each copy (English only, `site_copy`).
const LENGTH_COPIES: [&str; 2] = [
    "it hit its output length limit",            // "truncated"
    "action arrived cut off partway through",    // "truncated_unreported"
];
const STREAM_COPIES: [&str; 2] = [
    "before the model finished writing its action", // "stream_closed_tool_call"
    "while the model was writing a large action",   // "stream_dropped_tool_call"
];

/// Whether a failed run's `error` is one of Hermes' copies of a reply cut mid-way.
pub fn kind(error: &str) -> Option<Cut> {
    if LENGTH_COPIES.iter().any(|c| error.contains(c)) {
        Some(Cut::Length)
    } else if STREAM_COPIES.iter().any(|c| error.contains(c)) {
        Some(Cut::Stream)
    } else {
        None
    }
}

/// What the person reads instead of Hermes' copy.
pub fn person_line(cut: Cut, name: &str, lang: &str) -> String {
    match (lang, cut) {
        ("fr", Cut::Length) => format!(
            "La réponse de {name} a été coupée par la limite de longueur du modèle (pas par BipAgents) : rien n'a été \
             fait. Écris « continue » : la suite se fera en plusieurs parties."
        ),
        ("fr", Cut::Stream) => format!(
            "La réponse de {name} a été coupée en route chez le fournisseur du modèle (pas par BipAgents) : rien n'a \
             été fait. Écris « continue » : la suite se fera en plusieurs parties."
        ),
        ("de", Cut::Length) => format!(
            "Die Antwort von {name} wurde vom Längenlimit des Modells abgeschnitten (nicht von BipAgents): Nichts \
             wurde gemacht. Schreib „weiter“: Der Rest kommt in mehreren Teilen."
        ),
        ("de", Cut::Stream) => format!(
            "Die Antwort von {name} wurde unterwegs beim Anbieter des Modells abgeschnitten (nicht von BipAgents): \
             Nichts wurde gemacht. Schreib „weiter“: Der Rest kommt in mehreren Teilen."
        ),
        ("es", Cut::Length) => format!(
            "La respuesta de {name} se cortó por el límite de longitud del modelo (no por BipAgents): no se ha hecho \
             nada. Escribe «continúa»: lo demás irá en varias partes."
        ),
        ("es", Cut::Stream) => format!(
            "La respuesta de {name} se cortó por el camino, en el proveedor del modelo (no por BipAgents): no se ha \
             hecho nada. Escribe «continúa»: lo demás irá en varias partes."
        ),
        (_, Cut::Length) => format!(
            "{name}'s reply was cut by the model's length limit (not by BipAgents): nothing was done. Write \
             “continue”: the rest will come in several parts."
        ),
        (_, Cut::Stream) => format!(
            "{name}'s reply was cut on the way, at the model's provider (not by BipAgents): nothing was done. Write \
             “continue”: the rest will come in several parts."
        ),
    }
}

/// The agent's note (hidden by the apps), in its language: what cut it and what to do.
pub fn agent_note(cut: Cut, lang: &str) -> String {
    let text = match (lang, cut) {
        ("fr", Cut::Length) => "Ta réponse précédente a été coupée par la limite de longueur du modèle (pas par \
            BipAgents) : l'appel d'outil n'a pas été exécuté, rien n'a été écrit. Écris le fichier en plusieurs parties \
            (une première avec write_file, puis chaque suite ajoutée à la fin : append, ou `cat >> fichier <<'EOF'` \
            dans le terminal) ou résume.",
        ("fr", Cut::Stream) => "Ta réponse précédente a été coupée en route chez le fournisseur du modèle (pas par \
            BipAgents) : l'appel d'outil n'a pas été exécuté, rien n'a été écrit. Écris le fichier en plusieurs parties \
            (une première avec write_file, puis chaque suite ajoutée à la fin : append, ou `cat >> fichier <<'EOF'` \
            dans le terminal) ou résume.",
        ("de", Cut::Length) => "Deine letzte Antwort wurde vom Längenlimit des Modells abgeschnitten (nicht von \
            BipAgents): Der Werkzeugaufruf lief nicht, nichts wurde geschrieben. Schreib die Datei in mehreren Teilen \
            (den ersten mit write_file, dann jeden weiteren ans Ende angehängt: append, oder `cat >> datei <<'EOF'` im \
            Terminal) oder fasse zusammen.",
        ("de", Cut::Stream) => "Deine letzte Antwort wurde unterwegs beim Anbieter des Modells abgeschnitten (nicht \
            von BipAgents): Der Werkzeugaufruf lief nicht, nichts wurde geschrieben. Schreib die Datei in mehreren \
            Teilen (den ersten mit write_file, dann jeden weiteren ans Ende angehängt: append, oder `cat >> datei \
            <<'EOF'` im Terminal) oder fasse zusammen.",
        ("es", Cut::Length) => "Tu respuesta anterior se cortó por el límite de longitud del modelo (no por \
            BipAgents): la llamada a la herramienta no se ejecutó, no se escribió nada. Escribe el archivo en varias \
            partes (la primera con write_file, luego cada parte añadida al final: append, o `cat >> archivo <<'EOF'` \
            en el terminal) o resume.",
        ("es", Cut::Stream) => "Tu respuesta anterior se cortó por el camino, en el proveedor del modelo (no por \
            BipAgents): la llamada a la herramienta no se ejecutó, no se escribió nada. Escribe el archivo en varias \
            partes (la primera con write_file, luego cada parte añadida al final: append, o `cat >> archivo <<'EOF'` \
            en el terminal) o resume.",
        (_, Cut::Length) => "Your previous reply was cut by the model's length limit (not by BipAgents): the tool call \
            did not run, nothing was written. Write the file in parts (the first with write_file, then each next part \
            appended at the end: append, or `cat >> file <<'EOF'` in the terminal) or sum up.",
        (_, Cut::Stream) => "Your previous reply was cut on the way, at the model's provider (not by BipAgents): the \
            tool call did not run, nothing was written. Write the file in parts (the first with write_file, then each \
            next part appended at the end: append, or `cat >> file <<'EOF'` in the terminal) or sum up.",
    };
    format!("<!--bip:cut\n{text}\n-->")
}

/// The conversations whose last turn was cut: (agent, session) → why. In memory: a restart forgets them (the person
/// still read the plain line). A few at most.
static CUT: LazyLock<Mutex<HashMap<(String, String), Cut>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const KEPT: usize = 200;

/// A turn of `agent` in `session` was cut: its next message there carries the note.
pub fn noted(agent: &str, session: &str, cut: Cut) {
    let mut map = CUT.lock().unwrap_or_else(|e| e.into_inner());
    if map.len() >= KEPT {
        map.clear();
    }
    map.insert((agent.to_string(), session.to_string()), cut);
}

/// The note for the person's next message to `agent` in `session` (taken: given once).
pub fn take_note(agent: &str, session: &str, lang: &str) -> Option<String> {
    let cut = CUT.lock().unwrap_or_else(|e| e.into_inner()).remove(&(agent.to_string(), session.to_string()))?;
    Some(agent_note(cut, lang))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hermes v0.23.1's copies as they are (`agent/turn_failure_copy.py`).
    const TRUNCATED: &str = "The model's reply was cut off before it finished (it hit its output length limit), so \
        Hermes didn't run the incomplete action. Nothing was changed. Send `continue`, ask for the work in smaller \
        steps, or raise max_tokens for this model.";
    const UNREPORTED: &str = "The model's action arrived cut off partway through, so Hermes didn't run it. Nothing was \
        changed. The model didn't report hitting its output limit, so this was most likely a dropped connection or a \
        provider/router cutting the reply short. Send /retry; if it keeps happening, ask for the work in smaller steps.";
    const CLOSED: &str = "EUrouter kept closing the stream before the model finished writing its action, without \
        reporting an error, so nothing was run. This is usually the provider or a proxy in front of it cutting long \
        replies short. Send /retry; asking for the work in smaller steps also helps.";
    const DROPPED: &str = "The connection to EUrouter kept dropping while the model was writing a large action, so \
        nothing was run. Check your network and send /retry; asking for the file in smaller pieces also helps.";

    #[test]
    fn hermes_copies_are_recognised_and_other_failures_are_not() {
        assert_eq!(kind(TRUNCATED), Some(Cut::Length));
        assert_eq!(kind(UNREPORTED), Some(Cut::Length));
        assert_eq!(kind(CLOSED), Some(Cut::Stream));
        assert_eq!(kind(DROPPED), Some(Cut::Stream));
        assert_eq!(kind("agent run failed"), None);
        assert_eq!(kind("Error code: 401 - invalid api key"), None);
        assert_eq!(kind(""), None);
    }

    #[test]
    fn the_person_reads_who_cut_it_in_the_agents_language() {
        let fr = person_line(Cut::Length, "Wellness", "fr");
        assert!(fr.starts_with("La réponse de Wellness a été coupée par la limite de longueur du modèle (pas par BipAgents)"));
        assert!(fr.contains("« continue »") && !fr.contains("max_tokens"));
        assert!(person_line(Cut::Stream, "Vie", "fr").contains("fournisseur du modèle"));
        assert!(person_line(Cut::Length, "Vie", "en").contains("length limit (not by BipAgents)"));
        for lang in ["fr", "de", "es", "en", "nl"] {
            for cut in [Cut::Length, Cut::Stream] {
                assert!(person_line(cut, "Vie", lang).contains("BipAgents"));
                assert!(agent_note(cut, lang).contains("BipAgents"));
            }
        }
    }

    #[test]
    fn the_agent_gets_the_note_once_in_that_conversation_only() {
        noted("wellness-t", "s1", Cut::Length);
        assert_eq!(take_note("wellness-t", "s2", "fr"), None);
        assert_eq!(take_note("other-t", "s1", "fr"), None);
        let note = take_note("wellness-t", "s1", "fr").unwrap();
        assert!(note.starts_with("<!--bip:cut\n") && note.ends_with("\n-->"));
        assert!(note.contains("limite de longueur du modèle (pas par BipAgents)"));
        assert!(note.contains("plusieurs parties") && note.contains("append") && note.contains("résume"));
        assert_eq!(take_note("wellness-t", "s1", "fr"), None);
    }
}
