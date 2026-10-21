//! The speech text processing against golden fixtures from the Python (tests/golden/gen_speech.py): textproc,
//! numbers_fr, speech_intl, pronunciations and the pure parts of tts, on hand-written and thousands of random
//! inputs. Every output must match byte for byte.

use bipbridge::voice_speech::num2words::num2words;
use bipbridge::{numbers_fr, pronunciations, speech_intl, textproc, tts};
use indexmap::IndexMap;
use serde_json::Value;
use std::sync::LazyLock;

static CASES: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_reader(flate2::read::GzDecoder::new(&include_bytes!("golden/speech.json.gz")[..]))
        .expect("fixtures")
});

fn section(name: &str) -> &'static Vec<Value> {
    CASES[name].as_array().unwrap_or_else(|| panic!("no section {name}"))
}

fn s(v: &Value) -> &str {
    v.as_str().expect("a string")
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().expect("a list").iter().map(|x| s(x).to_string()).collect()
}

/// Every case of `name` through `run`; the differences listed (20 at most) if any.
fn check(name: &str, run: impl Fn(&Value) -> Value) {
    let cases = section(name);
    assert!(!cases.is_empty());
    let mut failures = Vec::new();
    for case in cases {
        let got = run(&case[0]);
        if got != case[1] {
            failures.push(format!("input {}\n  python {}\n  rust   {}", case[0], case[1], got));
        }
    }
    assert!(
        failures.is_empty(),
        "{name}: {} of {} differ:\n{}",
        failures.len(),
        cases.len(),
        failures[..failures.len().min(20)].join("\n")
    );
}

#[test]
fn normalize_segment() {
    check("normalize_segment", |i| Value::from(textproc::normalize_segment(s(i))));
}

#[test]
fn split_sentences() {
    check("split_sentences", |i| Value::from(textproc::split_sentences(s(&i[0]), i[1].as_u64().unwrap() as usize)));
}

#[test]
fn stream_in_pieces() {
    check("stream", |i| {
        let chars: Vec<char> = s(&i[0]).chars().collect();
        let step = i[1].as_u64().unwrap() as usize;
        let mut splitter = textproc::SentenceSplitter::new(i[2].as_u64().unwrap() as usize);
        let mut out = Vec::new();
        for piece in chars.chunks(step) {
            out.extend(splitter.feed(&piece.iter().collect::<String>()));
        }
        out.extend(splitter.flush());
        Value::from(out)
    });
}

#[test]
fn strip_emoji() {
    check("strip_emoji", |i| Value::from(textproc::strip_emoji(s(i))));
}

#[test]
fn normalize_for_speech() {
    check("normalize_for_speech", |i| {
        Value::from(textproc::normalize_for_speech(s(&i[0]), i[1].as_u64().unwrap() as usize, i[2].as_bool().unwrap()))
    });
}

#[test]
fn french_spell_int() {
    check("fr.spell_int", |i| Value::from(numbers_fr::spell_int(i.as_i64().unwrap())));
}

#[test]
fn french_spell_ordinal() {
    check("fr.spell_ordinal", |i| {
        Value::from(numbers_fr::spell_ordinal(i[0].as_u64().unwrap(), i[1].as_bool().unwrap()))
    });
}

#[test]
fn french_spell_numbers() {
    check("fr.spell_numbers", |i| Value::from(numbers_fr::spell_numbers(s(i))));
}

#[test]
fn french_prepare_for_synthesis() {
    check("fr.prepare_for_synthesis", |i| Value::from(numbers_fr::prepare_for_synthesis(s(i))));
}

#[test]
fn intl_prepare_for_synthesis() {
    check("intl.prepare_for_synthesis", |i| Value::from(speech_intl::prepare_for_synthesis(s(&i[0]), s(&i[1]))));
}

#[test]
fn intl_spell_numbers() {
    check("intl.spell_numbers", |i| Value::from(speech_intl::spell_numbers(s(&i[0]), s(&i[1]))));
}

#[test]
fn pronunciations() {
    check("pronunciations", |i| {
        let mut words: IndexMap<String, IndexMap<String, String>> = IndexMap::new();
        for (lang, table) in i[0].as_object().unwrap() {
            words.insert(
                lang.clone(),
                table.as_object().unwrap().iter().map(|(k, v)| (k.clone(), s(v).to_string())).collect(),
            );
        }
        Value::from(pronunciations::Lexicon::new(&words).apply(s(&i[1]), s(&i[2])))
    });
}

#[test]
fn tts_voice_helpers() {
    check("voice_language", |i| Value::from(tts::voice_language(s(i))));
    check("server_voice_id", |i| Value::from(tts::server_voice_id(s(i))));
    check("server_fallback", |i| Value::from(tts::server_fallback(s(i))));
    check("voice_names", |i| Value::from(tts::voice_names(i)));
}

#[test]
fn num2words_spot_checks() {
    // the full comparison is voice-speech_num2words.rs; these are speech_intl's own uses
    assert_eq!(num2words(1990, "en", "year").as_deref(), Some("nineteen ninety"));
    assert_eq!(strings(&Value::from(vec!["a"])), vec!["a"]);
}
