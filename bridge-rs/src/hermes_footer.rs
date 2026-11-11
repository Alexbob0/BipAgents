//! Hermes' file-mutation verifier, kept from the person. When a `write_file` or `patch` of the turn failed and no
//! later write of that file landed, Hermes adds a note for a developer at a terminal under the reply
//! (agent/turn_finalizer.py `_append_file_mutation_footer`, `display.file_mutation_verifier`, on by default):
//!
//! ```text
//! ⚠️ File-mutation verifier: 2 file edit(s) FAILED this turn despite any wording above that may suggest otherwise. Run `git status` or `read_file` to confirm what actually landed.
//!   • `/workspace/checkin_read.py` — [write_file] Failed to write file: Permission denied: /workspace
//!   • … and 3 more
//! ```
//!
//! English (or Hermes' display language), technical, alarming, about files the person never asked for: Alex read one
//! under Wellness' morning check-in (11/11, a routine's report in the Boîte, read aloud by « Écouter »). Hermes adds it
//! to the text it returns (a run's `output`, a chat's `assistant.completed`, a chat's reply) after the reply was
//! stored, so the model never reads it in its conversation (Hermes' `locales/en.yaml`, `explainer:`), and the tool
//! results it read already said which writes failed. The bridge takes it out where it reads Hermes' answer
//! (hermes.rs: a run's events and status, a chat's reply; live_turns.rs: a Live call's stream), so no copy reaches
//! the apps, a push, the Boîte, its audio, a Live call or a share; it is logged (`bipbridge::hermes`) for whoever
//! looks into the agent's files.

use serde_json::Value;

use crate::hermes::SSEEvent;

/// `explainer.file_mutation.header` in every language of Hermes v0.22.5 and v0.23.1 (`locales/*.yaml`, the same in
/// both), after its warning sign, up to its colon: the note starts a line with the sign and one of them.
const NAMES: [&str; 17] = [
    "Lêerwysiging-verifieerder:",                  // af
    "مدقّق تعديل الملفات:",                         // ar
    "Dateiänderungs-Prüfer:",                      // de
    "File-mutation verifier:",                     // en
    "Verificador de modificación de archivos:",    // es
    "Vérificateur de modifications de fichiers :", // fr
    "Fíoraitheoir athruithe comhaid:",             // ga
    "Fájlmódosítás-ellenőrző:",                    // hu
    "Verifica modifiche ai file:",                 // it
    "ファイル変更検証:",                           // ja
    "파일 변경 검증기:",                           // ko
    "Verificador de mutação de arquivos:",         // pt
    "Проверка изменений файлов:",                  // ru
    "Dosya değişikliği doğrulayıcısı:",            // tr
    "Перевірка змін файлів:",                      // uk
    "檔案變更驗證器：",                            // zh-hant
    "文件修改校验器：",                            // zh
];

/// The text fields of a final answer's event: a run's end (`output`), a chat's `assistant.completed` (`content`, or
/// `text` as other servers name it). A delta is never read: the note comes after the stream.
const TEXT_KEYS: [&str; 6] = ["output", "content", "text", "final_output", "response", "result"];

/// A reply once the verifier's note is out of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Cut {
    /// The reply as the person reads it (with Hermes' explanation of a turn without an answer when one followed the
    /// note: no_reply.rs puts it in plain words).
    pub text: String,
    /// The note itself, for the log.
    pub note: String,
}

/// Whether a line is the note's first: the warning sign (its emoji form or not) and one of the names.
fn is_header(line: &str) -> bool {
    let Some(rest) = line.trim_start_matches([' ', '\t']).strip_prefix('⚠') else { return false };
    let rest = rest.strip_prefix('\u{FE0F}').unwrap_or(rest).trim_start_matches(' ');
    NAMES.iter().any(|name| rest.starts_with(name))
}

/// Where each line of `text` starts.
fn line_starts(text: &str) -> impl Iterator<Item = usize> + '_ {
    std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).filter(move |&i| i < text.len())
}

/// `text` without the verifier's note; None when it has none. The note is the last line starting as one (a reply
/// quoting one keeps its words) and all that follows it (its files, one per bullet, a preview cut short « … »), but
/// for Hermes' explanation of a turn without an answer when it comes after (`_explain_abnormal_exit`, a stream that
/// stopped early): that stays, under the reply.
pub fn cut(text: &str) -> Option<Cut> {
    let at = line_starts(text).filter(|&i| is_header(&text[i..])).last()?;
    let reply = text[..at].trim_end();
    let rest = &text[at..];
    let after = line_starts(rest).skip(1).find(|&i| crate::no_reply::is_hermes_no_reply(&rest[i..]));
    let (note, explanation) = match after {
        Some(i) => (rest[..i].trim_end(), Some(rest[i..].trim())),
        None => (rest.trim_end(), None),
    };
    let text = match explanation {
        Some(e) if reply.is_empty() => e.to_string(),
        Some(e) => format!("{reply}\n\n{e}"),
        None => reply.to_string(),
    };
    Some(Cut { text, note: note.to_string() })
}

/// `text` as the person reads it, the note logged (`agent`, `about`: the run or the conversation it came from).
pub fn without(text: &str, agent: &str, about: &str) -> Option<String> {
    let cut = cut(text)?;
    let note: String = cut.note.chars().take(1200).collect();
    tracing::info!(target: "bipbridge::hermes", agent = %agent, about = %about, note = %note,
        "Hermes' file-mutation verifier note left out of the reply");
    Some(cut.text)
}

/// A final answer's event (a run's end, a chat's `assistant.completed`) without the note in its text, its raw JSON
/// written again; any other event as it is.
pub fn scrub_event(event: SSEEvent, agent: &str) -> SSEEvent {
    if !(crate::hermes::is_terminal(&event.r#type) || event.r#type == "assistant.completed") {
        return event;
    }
    let about = ["run_id", "session_id"]
        .iter()
        .find_map(|k| event.data.get(*k).and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let mut data = event.data;
    let mut changed = false;
    for key in TEXT_KEYS {
        let Some(Value::String(text)) = data.get(key) else { continue };
        if let Some(kept) = without(text, agent, &about) {
            data.insert(key.into(), Value::String(kept));
            changed = true;
        }
    }
    if !changed {
        return SSEEvent { data, ..event };
    }
    let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
    SSEEvent::new(event.r#type, data, raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// What Alex saw on 11/11 under Wellness' check-in (its second file's error as the app showed it, cut short).
    const SEEN: &str = "⚠️ File-mutation verifier: 2 file edit(s) FAILED this turn despite any wording above that may \
suggest otherwise. Run `git status` or `read_file` to confirm what actually landed.
  • `/workspace/checkin_read.py` — [write_file] Failed to write file: Permission denied: /workspace
  • `/home/alex/.hermes/profiles/wellness/cache/scratch/checkin_read.py` — [write_file] Refusing to overwrite \
`/home/alex/.hermes/profiles/wellness/cache/scratch/checkin_read.py`: `/home/alex/.hermes/profiles/wellness/cache/\
scratch/checkin_read.py` exists but this task h…";

    const REPLY: &str = "9. Notes libres ?\n\nDès que tu réponds, je logge la nuit avant toute autre analyse.\n\n\
Séance du jour : Jambes (muscu 40') — voir Garmin Connect pour le workout du jour.";

    #[test]
    fn the_check_in_of_11_11_reads_without_the_note() {
        let output = format!("{REPLY}\n\n{SEEN}");
        let cut = cut(&output).expect("a note");
        assert_eq!(cut.text, REPLY);
        assert_eq!(cut.note, SEEN);
        // nothing of it left: no sign, no path, no English
        assert!(!cut.text.contains('⚠') && !cut.text.contains("/workspace") && !cut.text.contains("verifier"));
    }

    #[test]
    fn every_language_and_every_shape_of_the_note() {
        let notes = [
            "⚠️ File-mutation verifier: 1 file edit(s) FAILED this turn despite any wording above that may suggest \
otherwise. Run `git status` or `read_file` to confirm what actually landed.\n  • `notes.md` — [patch] Could not \
find old_string",
            "⚠️ Vérificateur de modifications de fichiers : 12 modification(s) de fichier ont ÉCHOUÉ ce tour, quoi \
qu'en dise le texte ci-dessus. Lancez `git status` ou `read_file` pour vérifier ce qui a réellement été appliqué.\n  \
• `/workspace/a.py` — [write_file] échec\n  • … et 2 de plus",
            "⚠️ 文件修改校验器：本回合有 1 处文件编辑失败，无论上方措辞如何暗示。请运行 `git status` 或 `read_file` 确认实际落地的内容。\n  • \
`/workspace/a.py`——[write_file] 失败",
            "⚠️ Dateiänderungs-Prüfer: 1 Dateiänderung(en) sind in diesem Zug FEHLGESCHLAGEN, auch wenn die \
Formulierungen oben etwas anderes nahelegen.",
            // the sign without its emoji form, a preview on two lines
            "⚠ File-mutation verifier: 1 file edit(s) FAILED this turn.\n  • `x.json` — [write_file] Invalid JSON:\n\
line 3 column 1",
        ];
        for note in notes {
            let cut = cut(&format!("Voilà ton plan.\n\n{note}")).unwrap_or_else(|| panic!("{note}"));
            assert_eq!(cut.text, "Voilà ton plan.", "{note}");
            assert_eq!(cut.note, note);
        }
        // a reply that was only blank: nothing left to read
        assert_eq!(cut(&format!("\n\n{}", notes[0])).unwrap().text, "");
    }

    #[test]
    fn hermes_explanation_after_the_note_stays_for_no_reply() {
        let output = format!(
            "Je regarde ta nuit et\n\n{SEEN}\n\n⚠️ No reply: streaming stopped early and only a \
partial response was recovered. Send `continue` to resume from where it stopped."
        );
        let cut = cut(&output).unwrap();
        assert_eq!(cut.note, SEEN);
        assert!(cut.text.starts_with("Je regarde ta nuit et\n\n⚠️ No reply: streaming stopped early"));
        assert_eq!(crate::no_reply::words_before(&cut.text), Some("Je regarde ta nuit et"));
    }

    #[test]
    fn a_reply_that_only_speaks_of_it_keeps_its_words() {
        for said in [
            "Le « File-mutation verifier: » de Hermes signale les écritures ratées.",
            "Attention : ⚠️ File-mutation verifier: est une note de Hermes.",
            "⚠️ Attention : il va pleuvoir demain.",
            "⚠️ Vérificateur : tout est bon.",
        ] {
            assert_eq!(cut(said), None, "{said}");
        }
    }

    #[test]
    fn a_final_event_loses_the_note_and_its_raw_follows() {
        let output = format!("{REPLY}\n\n{SEEN}");
        let data = json!({"type": "run.completed", "run_id": "r1", "output": output});
        let event = SSEEvent::new("run.completed", data.as_object().cloned().unwrap(), data.to_string());
        let event = scrub_event(event, "wellness");
        assert_eq!(event.data["output"], json!(REPLY));
        assert!(!event.raw.contains("verifier") && event.raw.contains("Jambes"));
        assert_eq!(serde_json::from_str::<Value>(&event.raw).unwrap()["output"], json!(REPLY));
        // a Live call's chat
        let data = json!({"session_id": "s1", "content": output});
        let event = scrub_event(SSEEvent::new("assistant.completed", data.as_object().cloned().unwrap(), "x"), "w");
        assert_eq!(event.data["content"], json!(REPLY));
        // a delta (never the note: Hermes adds it after the stream) and an event without one, as they came
        let delta = SSEEvent::new("message.delta", json!({"delta": SEEN}).as_object().cloned().unwrap(), "RAW");
        assert_eq!(scrub_event(delta.clone(), "w"), delta);
        let plain = SSEEvent::new("run.completed", json!({"output": "Ok"}).as_object().cloned().unwrap(), "RAW");
        assert_eq!(scrub_event(plain.clone(), "w"), plain);
    }

    #[test]
    fn the_names_are_hermes_own() {
        // each name is the header of its language up to the colon (an ASCII one, a full-width one in Chinese)
        for name in NAMES {
            assert!(name.ends_with(':') || name.ends_with('：'), "{name}");
            assert!(!name.starts_with('⚠') && !name.starts_with(' '), "{name}");
        }
        assert!(is_header("⚠️ File-mutation verifier: 3 file edit(s) FAILED"));
        assert!(is_header("⚠️ مدقّق تعديل الملفات: فشل 1"));
        assert!(!is_header("File-mutation verifier: 3"));
    }
}
