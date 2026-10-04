"""French numbers in words, for speech: Pocket TTS reads digits one by one ("91" -> "neuf un").

``spell_numbers`` rewrites integers, decimals, times, ordinals, percentages, money, temperatures and a few
common units ("91" -> "quatre-vingt-onze", "14h30" -> "quatorze heures trente", "3,5 %" -> "trois virgule
cinq pour cent", "1er" -> "premier"). Anything it does not recognize is left untouched.
"""
from __future__ import annotations

import re

_UNITS = ["zéro", "un", "deux", "trois", "quatre", "cinq", "six", "sept", "huit", "neuf", "dix", "onze",
          "douze", "treize", "quatorze", "quinze", "seize"]
_TENS = {2: "vingt", 3: "trente", 4: "quarante", 5: "cinquante", 6: "soixante"}
_MAX = 10 ** 12


def _below_100(n: int) -> str:
    if n <= 16:
        return _UNITS[n]
    if n < 20:
        return "dix-" + _UNITS[n - 10]
    tens, unit = divmod(n, 10)
    if tens == 7:
        return "soixante et onze" if n == 71 else "soixante-" + _below_100(n - 60)
    if tens == 8:
        return "quatre-vingts" if unit == 0 else "quatre-vingt-" + _UNITS[unit]
    if tens == 9:
        return "quatre-vingt-" + _below_100(n - 80)
    word = _TENS[tens]
    if unit == 0:
        return word
    if unit == 1:
        return word + " et un"
    return word + "-" + _UNITS[unit]


def _below_1000(n: int) -> str:
    hundreds, rest = divmod(n, 100)
    if hundreds == 0:
        return _below_100(rest)
    head = "cent" if hundreds == 1 else _UNITS[hundreds] + " cent"
    if rest == 0:
        return head + ("s" if hundreds > 1 else "")
    return head + " " + _below_100(rest)


def spell_int(n: int) -> str:
    """0 <= n < 10**12, in words ("quatre-vingt-onze", "deux mille vingt-six")."""
    if n < 0:
        return "moins " + spell_int(-n)
    if n < 1000:
        return _below_1000(n)
    parts = []
    for value, singular, plural in ((10 ** 9, "milliard", "milliards"), (10 ** 6, "million", "millions")):
        count, n = divmod(n, value)
        if count:
            parts.append(f"{spell_int(count)} {singular if count == 1 else plural}")
    thousands, n = divmod(n, 1000)
    if thousands:
        words = _below_1000(thousands)
        if words.endswith(("vingts", "cents")):  # "quatre-vingt mille", "deux cent mille"
            words = words[:-1]
        parts.append("mille" if thousands == 1 else words + " mille")
    if n:
        parts.append(_below_1000(n))
    return " ".join(parts)


def _feminine(words: str) -> str:
    """"un" -> "une" at the end ("vingt et une heures")."""
    return words[:-2] + "une" if words == "un" or words.endswith((" un", "-un")) else words


def spell_ordinal(n: int, feminine: bool = False) -> str:
    if n == 1:
        return "première" if feminine else "premier"
    words = spell_int(n)
    if words.endswith("cinq"):
        return words + "uième"
    if words.endswith("neuf"):
        return words[:-1] + "vième"
    if words.endswith("s"):  # quatre-vingts, deux cents
        words = words[:-1]
    if words.endswith("e"):
        return words[:-1] + "ième"
    return words + "ième"


def _spell_digits(digits: str) -> str:
    """A digit string: leading zeros are read ("06" -> "zéro six"), huge numbers digit by digit."""
    if len(digits) > 1 and digits.startswith("0"):
        return " ".join(["zéro"] * (len(digits) - len(digits.lstrip("0"))) + ([spell_int(int(digits))] if digits.strip("0") else []))
    value = int(digits)
    if value >= _MAX:
        return " ".join(_UNITS[int(d)] for d in digits)
    return spell_int(value)


def _decimal(whole: str, fraction: str) -> str:
    return f"{_spell_digits(whole)} virgule {_spell_digits(fraction)}"


_NUM = r"\d+(?:[,.]\d+)?"
_GROUPED = re.compile(r"(?<![\d,.])(\d{1,3}(?:[   ]\d{3})+)(?![\d]|[,.]\d)")
_ORDINAL = re.compile(r"\b(\d+)(er|re|ère|ème|eme|e)\b")
_TIME_H = re.compile(r"\b([01]?\d|2[0-3]) ?[hH](?: ?([0-5]\d))?\b")
_TIME_COLON = re.compile(r"\b([01]?\d|2[0-3]):([0-5]\d)\b")
_UNIT_WORDS = [
    (r"%", "pour cent"), (r"€|EUR\b|euros?\b", "euros"), (r"\$|USD\b|dollars?\b", "dollars"),
    (r"°C\b|° ?C\b|°", "degrés"), (r"km/h\b", "kilomètres heure"), (r"km\b", "kilomètres"), (r"kg\b", "kilos"),
    (r"cm\b", "centimètres"), (r"ms\b", "millisecondes"), (r"min\b", "minutes"), (r"bpm\b", "battements par minute"),
]
_WITH_UNIT = re.compile(r"(?<![\w,.])(-?" + _NUM + r") ?(" + "|".join(p for p, _ in _UNIT_WORDS) + r")")
_DECIMAL = re.compile(r"(?<![\w.,])(\d+)[,.](\d+)(?![\d.,]*\d)")
_INTEGER = re.compile(r"(?<![\w.,])(\d+)(?![\w]|[.,]\d)")
_NEGATIVE = re.compile(r"(?<![\w-])-(?=\d)")


def _number(text: str) -> str:
    """"12" or "3,5" / "3.5" in words."""
    match = re.fullmatch(r"(-?)(\d+)(?:[,.](\d+))?", text)
    if not match:
        return text
    sign, whole, fraction = match.groups()
    words = _decimal(whole, fraction) if fraction else _spell_digits(whole)
    return ("moins " if sign else "") + words


def _unit_word(symbol: str) -> str:
    for pattern, word in _UNIT_WORDS:
        if re.fullmatch(pattern, symbol):
            return word
    return symbol


def _with_unit(match: re.Match) -> str:
    value, symbol = match.group(1), match.group(2)
    word = _unit_word(symbol)
    spoken = _number(value)
    if value.lstrip("-") in ("0", "1") and word.endswith("s") and word not in ("pour cent",):
        word = {"euros": "euro", "dollars": "dollar", "degrés": "degré", "kilomètres": "kilomètre",
                "kilos": "kilo", "centimètres": "centimètre", "millisecondes": "milliseconde",
                "minutes": "minute"}.get(word, word)
        if word in ("minute", "milliseconde") and value.lstrip("-") == "1":
            spoken = _feminine(spoken)
    return f"{spoken} {word}"


def _time(hours: str, minutes: str | None) -> str:
    h = int(hours)
    words = _feminine(spell_int(h)) + (" heure" if h <= 1 else " heures")
    if minutes and int(minutes):
        words += " " + _feminine(spell_int(int(minutes)))
    return words


_MONTHS = ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre",
           "novembre", "décembre"]
_FRACTIONS = {(1, 2): "un demi", (1, 3): "un tiers", (2, 3): "deux tiers", (1, 4): "un quart", (3, 4): "trois quarts"}
_PER = re.compile(r"\b(\d+) ?([hj]) ?/ ?(\d+)\b")
_DATE_FULL = re.compile(r"\b(\d{1,2})/(\d{1,2})/(\d{4}|\d{2})\b")
_DATE_SHORT = re.compile(r"\b(0[1-9]|[12]\d|3[01])/(0[1-9]|1[0-2])\b")
_FRACTION = re.compile(r"(?<![\w/])(\d+) ?/ ?(\d+)(?![\w/])")


def _date(day: str, month: str, year: str | None = None) -> str | None:
    d, m = int(day), int(month)
    if not (1 <= d <= 31 and 1 <= m <= 12):
        return None
    words = ("premier" if d == 1 else spell_int(d)) + " " + _MONTHS[m - 1]
    if year:
        y = int(year) + (2000 if len(year) == 2 else 0)
        words += " " + spell_int(y)
    return words


def _per(match: re.Match) -> str:
    unit = "heures" if match.group(2) == "h" else "jours"
    return f"{spell_int(int(match.group(1)))} {unit} sur {spell_int(int(match.group(3)))}"


def _fraction(match: re.Match) -> str:
    n, d = int(match.group(1)), int(match.group(2))
    return _FRACTIONS.get((n, d)) or f"{_spell_digits(match.group(1))} sur {_spell_digits(match.group(2))}"


def spell_numbers(text: str) -> str:
    if not any(c.isdigit() for c in text):
        return text
    text = _GROUPED.sub(lambda m: re.sub(r"[   ]", "", m.group(1)), text)
    text = _PER.sub(_per, text)
    text = _DATE_FULL.sub(lambda m: _date(*m.groups()) or m.group(0), text)
    text = _DATE_SHORT.sub(lambda m: _date(*m.groups()) or m.group(0), text)
    text = _FRACTION.sub(_fraction, text)
    text = _ORDINAL.sub(lambda m: spell_ordinal(int(m.group(1)), feminine=m.group(2) in ("re", "ère")), text)
    text = _TIME_COLON.sub(lambda m: _time(m.group(1), m.group(2)), text)
    text = _TIME_H.sub(lambda m: _time(m.group(1), m.group(2)), text)
    text = _WITH_UNIT.sub(_with_unit, text)
    text = _DECIMAL.sub(lambda m: _decimal(m.group(1), m.group(2)), text)
    text = _NEGATIVE.sub("moins ", text)
    text = _INTEGER.sub(lambda m: _spell_digits(m.group(1)), text)
    return text


_CAMEL = re.compile(r"\b[A-Z][a-z]+(?:[A-Z][a-z]+|[A-Z]{2,}\b)+\b")
_OPEN = re.compile(r"\s*[(\[]\s*")
_CLOSE = re.compile(r"\s*[)\]]\s*")
_DASH = re.compile(r"\s+[—–]\s+")
_COMMA_RUNS = re.compile(r"\s*,(?:\s*,)+\s*")
_COMMA_BEFORE_END = re.compile(r"\s*,\s*([.!?…:;])")


def _split_camel(match: re.Match) -> str:
    """"DuckDuckGo" -> "Duck Duck Go", "OpenAI" -> "Open AI" (each part is then read as a word)."""
    return re.sub(r"(?<=[a-z])(?=[A-Z])", " ", match.group(0))


_SLASH_WORDS = [(re.compile(r"\bet ?/ ?ou\b", re.I), "et ou"), (re.compile(r"\baller ?/ ?retour\b", re.I), "aller-retour"),
                (re.compile(r"\bA/R\b"), "aller-retour"), (re.compile(r"\bkm/h\b"), "kilomètres heure")]
_SLASH = re.compile(r"(?<=\w) ?/ ?(?=\w)")


def prepare_for_synthesis(text: str) -> str:
    """Last touch before a TTS engine: numbers in words, slashes read ("ou", "sur", dates), CamelCase names
    split, and parentheses or dashes read as pauses ("le train (TGV) part" -> "le train, TGV, part")."""
    text = spell_numbers(text)
    for pattern, words in _SLASH_WORDS:
        text = pattern.sub(words, text)
    text = _SLASH.sub(" ou ", text)  # "Garmin/Oura" -> "Garmin ou Oura"
    text = _CAMEL.sub(_split_camel, text)
    text = _DASH.sub(", ", text)
    text = _OPEN.sub(", ", text)
    text = _CLOSE.sub(", ", text)
    text = _COMMA_RUNS.sub(", ", text)
    text = _COMMA_BEFORE_END.sub(r"\1", text)
    return text.strip(" ,")
