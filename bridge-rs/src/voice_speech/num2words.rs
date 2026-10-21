//! The parts of num2words 0.5.14 (LGPL) that speech_intl uses: cardinals, ordinals and years in English, Spanish and
//! German, for 0 <= n < 10**12. A port of its algorithm (`splitnum` / `clean` / each language's `merge`), so the
//! words come out as Python's: checked against golden fixtures over wide ranges and random samples
//! (tests/golden/gen_num2words.py).

use super::pyre::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone)]
enum V {
    T(String, u64),
    L(Vec<V>),
}

#[derive(Clone, Copy, PartialEq)]
enum Lang {
    En,
    Es,
    De,
}

fn lang_of(lang: &str) -> Option<Lang> {
    match lang {
        "en" => Some(Lang::En),
        "es" => Some(Lang::Es),
        "de" => Some(Lang::De),
        _ => None,
    }
}

const P6: u64 = 1_000_000;
const P9: u64 = 1_000_000_000;
const P12: u64 = 1_000_000_000_000;

/// The cards (value, word) in the order num2words keeps them (high, mid, low), up to what a number below 10**12
/// can reach.
fn cards(lang: Lang) -> &'static [(u64, &'static str)] {
    match lang {
        Lang::En => &[
            (P12, "trillion"),
            (P9, "billion"),
            (P6, "million"),
            (1000, "thousand"),
            (100, "hundred"),
            (90, "ninety"),
            (80, "eighty"),
            (70, "seventy"),
            (60, "sixty"),
            (50, "fifty"),
            (40, "forty"),
            (30, "thirty"),
            (20, "twenty"),
            (19, "nineteen"),
            (18, "eighteen"),
            (17, "seventeen"),
            (16, "sixteen"),
            (15, "fifteen"),
            (14, "fourteen"),
            (13, "thirteen"),
            (12, "twelve"),
            (11, "eleven"),
            (10, "ten"),
            (9, "nine"),
            (8, "eight"),
            (7, "seven"),
            (6, "six"),
            (5, "five"),
            (4, "four"),
            (3, "three"),
            (2, "two"),
            (1, "one"),
            (0, "zero"),
        ],
        Lang::Es => &[
            (P12, "billón"),
            (P6, "millón"),
            (1000, "mil"),
            (100, "cien"),
            (90, "noventa"),
            (80, "ochenta"),
            (70, "setenta"),
            (60, "sesenta"),
            (50, "cincuenta"),
            (40, "cuarenta"),
            (30, "treinta"),
            (29, "veintinueve"),
            (28, "veintiocho"),
            (27, "veintisiete"),
            (26, "veintiséis"),
            (25, "veinticinco"),
            (24, "veinticuatro"),
            (23, "veintitrés"),
            (22, "veintidós"),
            (21, "veintiuno"),
            (20, "veinte"),
            (19, "diecinueve"),
            (18, "dieciocho"),
            (17, "diecisiete"),
            (16, "dieciséis"),
            (15, "quince"),
            (14, "catorce"),
            (13, "trece"),
            (12, "doce"),
            (11, "once"),
            (10, "diez"),
            (9, "nueve"),
            (8, "ocho"),
            (7, "siete"),
            (6, "seis"),
            (5, "cinco"),
            (4, "cuatro"),
            (3, "tres"),
            (2, "dos"),
            (1, "uno"),
            (0, "cero"),
        ],
        Lang::De => &[
            (1_000_000_000_000_000, "Billiarde"),
            (P12, "Billion"),
            (P9, "Milliarde"),
            (P6, "Million"),
            (1000, "tausend"),
            (100, "hundert"),
            (90, "neunzig"),
            (80, "achtzig"),
            (70, "siebzig"),
            (60, "sechzig"),
            (50, "fünfzig"),
            (40, "vierzig"),
            (30, "dreißig"),
            (20, "zwanzig"),
            (19, "neunzehn"),
            (18, "achtzehn"),
            (17, "siebzehn"),
            (16, "sechzehn"),
            (15, "fünfzehn"),
            (14, "vierzehn"),
            (13, "dreizehn"),
            (12, "zwölf"),
            (11, "elf"),
            (10, "zehn"),
            (9, "neun"),
            (8, "acht"),
            (7, "sieben"),
            (6, "sechs"),
            (5, "fünf"),
            (4, "vier"),
            (3, "drei"),
            (2, "zwei"),
            (1, "eins"),
            (0, "null"),
        ],
    }
}

fn card(lang: Lang, value: u64) -> &'static str {
    cards(lang).iter().find(|(v, _)| *v == value).map(|(_, w)| *w).unwrap_or("")
}

fn splitnum(lang: Lang, value: u64) -> Vec<V> {
    for &(elem, word) in cards(lang) {
        if elem > value {
            continue;
        }
        let (div, rest) = if value == 0 { (1, 0) } else { (value / elem, value % elem) };
        let mut out = Vec::new();
        if div == 1 {
            out.push(V::T(card(lang, 1).to_string(), 1));
        } else {
            if div == value {
                // the system tallies (never reached by these languages' cards)
                return vec![V::T(word.repeat(div as usize), div * elem)];
            }
            out.push(V::L(splitnum(lang, div)));
        }
        out.push(V::T(word.to_string(), elem));
        if rest != 0 {
            out.push(V::L(splitnum(lang, rest)));
        }
        return out;
    }
    Vec::new()
}

fn chars_drop_last(text: &str, n: usize) -> String {
    let count = text.chars().count();
    text.chars().take(count.saturating_sub(n)).collect()
}

fn merge(lang: Lang, (ltext, lnum): (String, u64), (rtext, rnum): (String, u64)) -> (String, u64) {
    match lang {
        Lang::En => {
            if lnum == 1 && rnum < 100 {
                (rtext, rnum)
            } else if 100 > lnum && lnum > rnum {
                (format!("{ltext}-{rtext}"), lnum + rnum)
            } else if lnum >= 100 && 100 > rnum {
                (format!("{ltext} and {rtext}"), lnum + rnum)
            } else if rnum > lnum {
                (format!("{ltext} {rtext}"), lnum * rnum)
            } else {
                (format!("{ltext}, {rtext}"), lnum + rnum)
            }
        }
        Lang::Es => {
            let (mut ctext, cnum, mut ntext, nnum) = (ltext, lnum, rtext, rnum);
            if cnum == 1 {
                if nnum < 1_000_000 {
                    return (ntext, nnum);
                }
                ctext = "un".into();
            } else if cnum == 100 && nnum % 1000 != 0 {
                ctext += "to";
            }
            if nnum < cnum {
                if cnum < 100 {
                    return (format!("{ctext} y {ntext}"), cnum + nnum);
                }
                return (format!("{ctext} {ntext}"), cnum + nnum);
            } else if nnum % 1_000_000 == 0 && cnum > 1 {
                ntext = chars_drop_last(&ntext, 3) + "lones";
            }
            if nnum == 100 {
                if cnum == 5 {
                    ctext = "quinien".into();
                    ntext = String::new();
                } else if cnum == 7 {
                    ctext = "sete".into();
                } else if cnum == 9 {
                    ctext = "nove".into();
                }
                ntext += "tos";
            } else {
                ntext = format!(" {ntext}");
            }
            (ctext + &ntext, cnum * nnum)
        }
        Lang::De => {
            let (mut ctext, cnum, mut ntext, nnum) = (ltext, lnum, rtext, rnum);
            if cnum == 1 {
                if nnum == 100 || nnum == 1000 {
                    return (format!("ein{ntext}"), nnum);
                } else if nnum < 1_000_000 {
                    return (ntext, nnum);
                }
                ctext = "eine".into();
            }

            let val = if nnum > cnum {
                if nnum >= 1_000_000 {
                    if cnum > 1 {
                        if ntext.ends_with('e') {
                            ntext += "n";
                        } else {
                            ntext += "en";
                        }
                    }
                    ctext += " ";
                }
                cnum * nnum
            } else {
                if nnum < 10 && 10 < cnum && cnum < 100 {
                    if nnum == 1 {
                        ntext = "ein".into();
                    }
                    let swapped = ctext;
                    ctext = ntext + "und";
                    ntext = swapped;
                } else if cnum >= 1_000_000 {
                    ctext += " ";
                }
                cnum + nnum
            };
            (ctext + &ntext, val)
        }
    }
}

fn clean(lang: Lang, val: Vec<V>) -> V {
    let mut val = val;
    while val.len() != 1 {
        let mut out = Vec::new();
        match (&val[0], &val[1]) {
            (V::T(lt, ln), V::T(rt, rn)) => {
                let (text, num) = merge(lang, (lt.clone(), *ln), (rt.clone(), *rn));
                out.push(V::T(text, num));
                if val.len() > 2 {
                    out.push(V::L(val[2..].to_vec()));
                }
            }
            _ => {
                for elem in val {
                    match elem {
                        V::L(inner) => {
                            if inner.len() == 1 {
                                out.push(inner.into_iter().next().unwrap_or(V::L(Vec::new())));
                            } else {
                                out.push(clean(lang, inner));
                            }
                        }
                        other => out.push(other),
                    }
                }
            }
        }
        val = out;
    }
    val.into_iter().next().unwrap_or(V::L(Vec::new()))
}

fn cardinal(lang: Lang, value: u64) -> String {
    match clean(lang, splitnum(lang, value)) {
        V::T(words, _) => words,
        V::L(_) => String::new(),
    }
}

fn ordinal(lang: Lang, value: u64) -> String {
    match lang {
        Lang::En => {
            let card = cardinal(lang, value);
            let mut outwords: Vec<String> = card.split(' ').map(str::to_string).collect();
            let last = outwords.pop().unwrap_or_default();
            let mut lastwords: Vec<String> = last.split('-').map(str::to_string).collect();
            let mut lastword = lastwords.pop().unwrap_or_default().to_lowercase();
            let ords = [
                ("one", "first"),
                ("two", "second"),
                ("three", "third"),
                ("four", "fourth"),
                ("five", "fifth"),
                ("six", "sixth"),
                ("seven", "seventh"),
                ("eight", "eighth"),
                ("nine", "ninth"),
                ("ten", "tenth"),
                ("eleven", "eleventh"),
                ("twelve", "twelfth"),
            ];
            match ords.iter().find(|(k, _)| *k == lastword) {
                Some((_, o)) => lastword = o.to_string(),
                None => {
                    if lastword.ends_with('y') {
                        lastword = chars_drop_last(&lastword, 1) + "ie";
                    }
                    lastword += "th";
                }
            }
            lastwords.push(lastword);
            outwords.push(lastwords.join("-"));
            outwords.join(" ")
        }
        Lang::Es => es_ordinal(value),
        Lang::De => {
            let mut outword = cardinal(lang, value).to_lowercase();
            let ords = [
                ("eins", "ers"),
                ("drei", "drit"),
                ("acht", "ach"),
                ("sieben", "sieb"),
                ("ig", "igs"),
                ("ert", "erts"),
                ("end", "ends"),
                ("ion", "ions"),
                ("nen", "ns"),
                ("rde", "rds"),
                ("rden", "rds"),
            ];
            for (key, ord) in ords {
                if outword.ends_with(key) {
                    outword = chars_drop_last(&outword, key.chars().count()) + ord;
                    break;
                }
            }
            let mut res = outword + "te";
            if res == "eintausendste" || res == "einhundertste" {
                res = res.replacen("ein", "", 1);
            }
            static EINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"eine ([a-z]+(illion|illiard)ste)$"));
            static SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" ([a-z]+(illion|illiard)ste)$"));
            let res = EINE.replace_with(&res, |m| m.group(1));
            SPACE.replace_with(&res, |m| m.group(1))
        }
    }
}

const ES_ORDS: &[(u64, &str)] = &[
    (1, "primer"),
    (2, "segund"),
    (3, "tercer"),
    (4, "cuart"),
    (5, "quint"),
    (6, "sext"),
    (7, "séptim"),
    (8, "octav"),
    (9, "noven"),
    (10, "décim"),
    (20, "vigésim"),
    (30, "trigésim"),
    (40, "quadragésim"),
    (50, "quincuagésim"),
    (60, "sexagésim"),
    (70, "septuagésim"),
    (80, "octogésim"),
    (90, "nonagésim"),
    (100, "centésim"),
    (200, "ducentésim"),
    (300, "tricentésim"),
    (400, "cuadrigentésim"),
    (500, "quingentésim"),
    (600, "sexcentésim"),
    (700, "septigentésim"),
    (800, "octigentésim"),
    (900, "noningentésim"),
    (1000, "milésim"),
    (P6, "millonésim"),
    (P9, "billonésim"),
    (P12, "trillonésim"),
    (1_000_000_000_000_000, "cuadrillonésim"),
];

fn es_ord(value: u64) -> &'static str {
    ES_ORDS.iter().find(|(v, _)| *v == value).map(|(_, w)| *w).unwrap_or("")
}

fn es_ordinal(value: u64) -> String {
    let text = if value == 0 {
        String::new()
    } else if value <= 10 {
        format!("{}o", es_ord(value))
    } else if value <= 12 {
        format!("{}o{}", es_ord(10), es_ordinal(value - 10))
    } else if value <= 100 {
        let dec = (value / 10) * 10;
        format!("{}o {}", es_ord(dec), es_ordinal(value - dec))
    } else if value <= 1000 {
        let cen = (value / 100) * 100;
        format!("{}o {}", es_ord(cen), es_ordinal(value - cen))
    } else {
        // Round down to the nearest 1e(3n), as num2words: 1000 ** int(math.log(value, 1000))
        let exponent = ((value as f64).ln() / 1000f64.ln()) as u32;
        let dec = 1000u64.pow(exponent);
        let (high, low) = (value / dec, value % dec);
        let card = if high != 1 { cardinal(Lang::Es, high) } else { String::new() };
        format!("{card}{}o {}", es_ord(dec), es_ordinal(low))
    };
    text.trim().to_string()
}

fn year(lang: Lang, value: u64) -> String {
    match lang {
        Lang::En => {
            let (high, low) = (value / 100, value % 100);
            if high == 0 || (high % 10 == 0 && low < 10) || high >= 100 {
                cardinal(lang, value)
            } else {
                let hightext = cardinal(lang, high);
                let lowtext = if low == 0 {
                    "hundred".to_string()
                } else if low < 10 {
                    format!("oh-{}", cardinal(lang, low))
                } else {
                    cardinal(lang, low)
                };
                format!("{hightext} {lowtext}")
            }
        }
        Lang::Es => cardinal(lang, value),
        Lang::De => {
            if (value / 100).is_multiple_of(10) {
                return cardinal(lang, value);
            }
            // to_splitnum(val, hightxt="hundert", longval=True), spaces removed
            let (high, low) = (value / 100, value % 100);
            let mut out = Vec::new();
            if high != 0 {
                out.push(cardinal(lang, high));
                out.push("hundert".to_string());
            }
            if low != 0 {
                out.push(cardinal(lang, low));
            }
            out.join(" ").replace(' ', "")
        }
    }
}

/// `num2words(n, lang=lang, to=to)` for `to` in cardinal, ordinal, year; None for another language or kind.
pub fn num2words(n: u64, lang: &str, to: &str) -> Option<String> {
    let lang = lang_of(lang)?;
    match to {
        "cardinal" => Some(cardinal(lang, n)),
        "ordinal" => Some(ordinal(lang, n)),
        "year" => Some(year(lang, n)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::num2words;

    #[test]
    fn a_few() {
        assert_eq!(num2words(1250, "en", "cardinal").unwrap(), "one thousand, two hundred and fifty");
        assert_eq!(num2words(2026, "en", "year").unwrap(), "twenty twenty-six");
        assert_eq!(num2words(12, "en", "ordinal").unwrap(), "twelfth");
        assert_eq!(num2words(1250, "es", "cardinal").unwrap(), "mil doscientos cincuenta");
        assert_eq!(num2words(3, "es", "ordinal").unwrap(), "tercero");
        assert_eq!(num2words(1250, "de", "cardinal").unwrap(), "eintausendzweihundertfünfzig");
        assert_eq!(num2words(1999, "de", "year").unwrap(), "neunzehnhundertneunundneunzig");
        assert_eq!(num2words(3, "de", "ordinal").unwrap(), "dritte");
    }
}
