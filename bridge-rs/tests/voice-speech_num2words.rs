//! num2words (speech_intl's speller) against golden fixtures from Python's num2words 0.5.14
//! (tests/golden/gen_num2words.py): cardinals, ordinals and years in English, Spanish and German.

use bipbridge::voice_speech::num2words::num2words;
use serde_json::Value;

#[test]
fn num2words_matches_python() {
    let cases: Vec<Value> =
        serde_json::from_reader(flate2::read::GzDecoder::new(&include_bytes!("golden/num2words.json.gz")[..]))
            .expect("fixtures");
    let mut failures = Vec::new();
    for case in &cases {
        let n = case[0].as_u64().expect("n");
        let (lang, to) = (case[1].as_str().unwrap(), case[2].as_str().unwrap());
        let expected = case[3].as_str().map(str::to_string);
        let got = num2words(n, lang, to).map(|s| s.replace(',', ""));
        if got != expected {
            failures.push(format!("{n} {lang} {to}: {got:?} != {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} differ, e.g.:\n{}",
        failures.len(),
        cases.len(),
        failures[..failures.len().min(20)].join("\n")
    );
}
