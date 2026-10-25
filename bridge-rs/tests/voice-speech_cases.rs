//! Port of tests/test_numbers_fr.py and tests/test_speech_intl.py: their parametrized cases, with the values the
//! Python tests expect, taken from the test files by tests/golden/gen_speech_cases.py.

use bipbridge::pronounce::Spelling::Respelled;
use bipbridge::{numbers_fr, speech_intl, tts};
use serde_json::Value;

fn cases(module: &str, test: &str) -> Vec<Vec<Value>> {
    let all: Value = serde_json::from_str(include_str!("golden/speech_cases.json")).unwrap();
    let list = all[module][test].as_array().unwrap_or_else(|| panic!("{module}.{test}"));
    assert!(!list.is_empty());
    list.iter().map(|c| c.as_array().unwrap().clone()).collect()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

#[test]
fn numbers_fr_spell_int() {
    for c in cases("numbers_fr", "test_spell_int") {
        assert_eq!(numbers_fr::spell_int(c[0].as_i64().unwrap()), s(&c[1]), "{:?}", c[0]);
    }
}

#[test]
fn numbers_fr_spell_ordinal() {
    for c in cases("numbers_fr", "test_spell_ordinal") {
        assert_eq!(numbers_fr::spell_ordinal(c[0].as_u64().unwrap(), false), s(&c[1]), "{:?}", c[0]);
    }
}

#[test]
fn numbers_fr_spell_numbers() {
    for c in cases("numbers_fr", "test_spell_numbers") {
        assert_eq!(numbers_fr::spell_numbers(s(&c[0])), s(&c[1]));
    }
}

#[test]
fn numbers_fr_prepare_for_synthesis() {
    for test in
        ["test_prepare_for_synthesis", "test_lexicon", "test_tilde_and_english_words", "test_ranges_and_dashed_dates"]
    {
        for c in cases("numbers_fr", test) {
            assert_eq!(numbers_fr::prepare_for_synthesis_with(s(&c[0]), Respelled), s(&c[1]), "{test}");
        }
    }
}

#[test]
fn speech_intl_voice_language() {
    for c in cases("speech_intl", "test_voice_language") {
        assert_eq!(tts::voice_language(s(&c[0])), s(&c[1]));
    }
}

#[test]
fn speech_intl_prepare_for_synthesis() {
    for c in cases("speech_intl", "test_prepare_for_synthesis") {
        assert_eq!(speech_intl::prepare_for_synthesis_with(s(&c[1]), s(&c[0]), Respelled), s(&c[2]));
    }
}
