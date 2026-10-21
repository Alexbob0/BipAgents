//! Python's `str` methods the speech modules use, on Python's character tables (`pychars`): `isdigit`,
//! `isalnum`, `isupper`, `isspace`, `strip`, `split`, `splitlines`, `lower`, `int()` of a digit string…

use super::pychars;

fn in_table(table: &[(u32, u32)], c: char) -> bool {
    let cp = c as u32;
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < cp {
                std::cmp::Ordering::Less
            } else if lo > cp {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `re`'s `\w`.
pub fn is_word(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    in_table(pychars::WORD, c)
}

/// `str.isalnum()` of one character.
pub fn is_alnum(c: char) -> bool {
    c != '_' && is_word(c)
}

/// `re`'s `\d`, `str.isdecimal()`.
pub fn is_decimal(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_digit();
    }
    in_table(pychars::DECIMAL, c)
}

/// `str.isdigit()` of one character (`²` too).
pub fn is_digit(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_digit();
    }
    in_table(pychars::DIGIT, c)
}

/// `re`'s `\s`, `str.isspace()`.
pub fn is_space(c: char) -> bool {
    in_table(pychars::SPACE, c)
}

/// `str.isupper()` of one character.
pub fn is_upper(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_uppercase();
    }
    in_table(pychars::UPPER, c)
}

/// The value of a decimal digit (`unicodedata.decimal`).
pub fn decimal_value(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return Some(c as u32 - '0' as u32);
    }
    if !is_decimal(c) {
        return None;
    }
    let cp = c as u32;
    let i = pychars::DECIMAL_ZEROS.partition_point(|&z| z <= cp);
    (i > 0).then(|| cp - pychars::DECIMAL_ZEROS[i - 1])
}

/// The simple lowercase mapping of one character (`sre`'s `lower`).
pub fn lower_char(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// The simple uppercase mapping of one character.
pub fn upper_char(c: char) -> char {
    let mut up = c.to_uppercase();
    match (up.next(), up.next()) {
        (Some(u), None) => u,
        _ => c,
    }
}

/// `str.lower()`.
pub fn lower(text: &str) -> String {
    text.to_lowercase()
}

/// `str.strip()`.
pub fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `str.lstrip()`.
pub fn lstrip(text: &str) -> &str {
    text.trim_start_matches(is_space)
}

/// `str.rstrip()`.
pub fn rstrip(text: &str) -> &str {
    text.trim_end_matches(is_space)
}

/// `str.strip(chars)`.
pub fn strip_chars<'a>(text: &'a str, chars: &str) -> &'a str {
    text.trim_matches(|c| chars.contains(c))
}

/// `str.split()` (runs of whitespace, none at the ends).
pub fn split_ws(text: &str) -> Vec<&str> {
    text.split(is_space).filter(|s| !s.is_empty()).collect()
}

/// `str.splitlines()`: `\n`, `\r`, `\r\n`, `\v`, `\f`, `\x1c`-`\x1e`, `\x85`, ` `, ` `.
pub fn splitlines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(std::mem::take(&mut line));
            }
            '\n' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}' => {
                out.push(std::mem::take(&mut line));
            }
            c => line.push(c),
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// `any(c.isdigit() for c in text)`.
pub fn has_digit(text: &str) -> bool {
    text.chars().any(is_digit)
}

/// `str.isdigit()` of a whole string (not empty, every character a digit).
pub fn all_digits(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_digit)
}

/// `str(int(digits))` of decimal digits (any length, Unicode digits read as Python's `int()` does).
pub fn int_str(digits: &str) -> String {
    let ascii: String = digits.chars().filter_map(|c| decimal_value(c).and_then(|v| char::from_digit(v, 10))).collect();
    let trimmed = ascii.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// `int(digits)` of decimal digits, None beyond u64 (the callers spell nothing that large).
pub fn int_value(digits: &str) -> Option<u64> {
    int_str(digits).parse().ok()
}

/// The number of characters (`len(text)`).
pub fn len(text: &str) -> usize {
    text.chars().count()
}

/// `text[:n]` in characters.
pub fn head(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_predicates() {
        assert!(is_word('²') && is_digit('²') && !is_decimal('²'));
        assert!(!is_word('\u{301}')); // a combining accent is not alphanumeric in Python
        assert!(is_space('\x1c') && is_space('\u{a0}') && !is_space('\u{200b}'));
        assert_eq!(decimal_value('٣'), Some(3));
        assert_eq!(int_str("٠٠42"), "42");
        assert_eq!(splitlines("a\r\nb\rc\n\nd\x1ce\n"), vec!["a", "b", "c", "", "d", "e"]);
        assert_eq!(strip(" \u{a0}x\x1f"), "x");
        assert_eq!(lower_char('İ'), 'i');
    }
}
