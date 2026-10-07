"""English, Spanish and German text preparation for speech (the French one is ``numbers_fr``).

Pocket TTS reads digits one by one and skips most symbols, so numbers, money, times, units and common
abbreviations are spelled out in the voice's language ("$5" -> "five dollars", "14:30" -> "vierzehn Uhr
dreißig", "3,5 %" -> "tres coma cinco por ciento"). Pauses (parentheses, dashes) and CamelCase names are
handled as in French. Anything not recognized is left untouched.
"""
from __future__ import annotations

import re
from typing import Callable, Dict, List, Optional, Tuple

from . import numbers_fr as fr

try:  # LGPL, optional: without it numbers are left as digits
    from num2words import num2words as _num2words
except ImportError:  # pragma: no cover - depends on the deployment
    _num2words = None

LANGUAGES = ("en", "es", "de")


def spell(n: int, lang: str, to: str = "cardinal") -> str:
    if _num2words is None or abs(n) >= 10 ** 12:
        return str(n)
    try:
        return _num2words(n, lang=lang, to=to).replace(",", "")
    except (NotImplementedError, OverflowError, ValueError):
        return str(n)


def _digits(digits: str, lang: str) -> str:
    """"0042" keeps its leading zeros, read one by one."""
    stripped = digits.lstrip("0")
    zeros = len(digits) - len(stripped)
    zero = {"en": "zero", "es": "cero", "de": "null"}[lang]
    words = [zero] * zeros + ([spell(int(stripped), lang)] if stripped else [])
    return " ".join(words) or zero


# Per language: decimal word, minus, "or", units (pattern, plural, singular), lexicon, the one-before-a-noun forms.
_UNITS: Dict[str, List[Tuple[str, str, str]]] = {
    "en": [(r"%", "percent", "percent"), (r"€|EUR\b|euros?\b", "euros", "euro"), (r"\$|USD\b|dollars?\b", "dollars", "dollar"),
           (r"£|GBP\b", "pounds", "pound"), (r"°C\b|° ?C\b", "degrees Celsius", "degree Celsius"),
           (r"°F\b|° ?F\b", "degrees Fahrenheit", "degree Fahrenheit"), (r"°", "degrees", "degree"),
           (r"km/h\b", "kilometers per hour", "kilometer per hour"), (r"mph\b", "miles per hour", "mile per hour"),
           (r"km\b", "kilometers", "kilometer"), (r"kg\b", "kilograms", "kilogram"), (r"lbs?\b", "pounds", "pound"),
           (r"cm\b", "centimeters", "centimeter"), (r"ms\b", "milliseconds", "millisecond"),
           (r"min\b|minutes?\b", "minutes", "minute"), (r"h\b|hours?\b", "hours", "hour"),
           (r"bpm\b", "beats per minute", "beat per minute")],
    "es": [(r"%", "por ciento", "por ciento"), (r"€|EUR\b|euros?\b", "euros", "euro"), (r"\$|USD\b|dólar(?:es)?\b", "dólares", "dólar"),
           (r"°C\b|° ?C\b|°", "grados", "grado"), (r"km/h\b", "kilómetros por hora", "kilómetro por hora"),
           (r"km\b", "kilómetros", "kilómetro"), (r"kg\b", "kilos", "kilo"), (r"cm\b", "centímetros", "centímetro"),
           (r"ms\b", "milisegundos", "milisegundo"), (r"min\b|minutos?\b", "minutos", "minuto"), (r"h\b|horas?\b", "horas", "hora"),
           (r"lpm\b|bpm\b", "pulsaciones por minuto", "pulsación por minuto")],
    "de": [(r"%", "Prozent", "Prozent"), (r"€|EUR\b|Euro\b", "Euro", "Euro"), (r"\$|USD\b|Dollar\b", "Dollar", "Dollar"),
           (r"°C\b|° ?C\b|°", "Grad", "Grad"), (r"km/h\b", "Kilometer pro Stunde", "Kilometer pro Stunde"),
           (r"km\b", "Kilometer", "Kilometer"), (r"kg\b", "Kilogramm", "Kilogramm"), (r"cm\b", "Zentimeter", "Zentimeter"),
           (r"ms\b", "Millisekunden", "Millisekunde"), (r"min\b|Minuten?\b", "Minuten", "Minute"),
           (r"h\b|Stunden?\b", "Stunden", "Stunde"),
           (r"bpm\b", "Schläge pro Minute", "Schlag pro Minute")],
}
# "1" before a singular unit: "one hour", "una hora", "eine Stunde".
_ONE = {
    "en": lambda word: "one",
    "es": lambda word: "una" if word in ("hora", "pulsación por minuto") else "un",
    "de": lambda word: "eine" if word in ("Stunde", "Minute", "Millisekunde") else "ein",
}
_CURRENCIES = {"euros", "dollars", "pounds", "dólares", "Euro", "Dollar"}
_WORDS = {
    "en": {"point": "point", "minus": "minus", "or": "or", "to": "to"},
    "es": {"point": "coma", "minus": "menos", "or": "o", "to": "a"},
    "de": {"point": "Komma", "minus": "minus", "or": "oder", "to": "bis"},
}
_LEXICON_SOURCE: Dict[str, List[Tuple[str, str]]] = {
    "en": [(r"\s*(?:→|->|⟶|⇒|=>)\s*", " to "), (r"\s+&\s+", " and "), (r"\s+\+\s+", " plus "),
           (r"(?<=\d)\s?[~∼]\s?(?=\d)", " to "), (r"\s*(?<!~)[~∼≈≃](?!~)\s*", " about "), (r"(?<!\w)≥\s?", "at least "),
           (r"(?<!\w)≤\s?", "at most "), (r"(?<=\d)\s?×\s?(?=\d)", " times "), (r"\s@\s", " at "),
           (r"\be\.g\.,?", "for example"), (r"\bi\.e\.,?", "that is"), (r"\betc\.(?=\s*\S)", "et cetera,"),
           (r"\betc\.", "et cetera."), (r"\bvs\.?(?= )", "versus"), (r"\bapprox\.", "approximately"),
           (r"\bDr\.(?= [A-Z])", "Doctor"), (r"\bMr\.(?= [A-Z])", "Mister"), (r"\bMrs\.(?= [A-Z])", "Missus"),
           (r"\bMs\.(?= [A-Z])", "Miz"), (r"\bNo\.(?= ?\d)", "number"), (r"#(?=\d)", "number ")],
    "es": [(r"\s*(?:→|->|⟶|⇒|=>)\s*", " a "), (r"\s+&\s+", " y "), (r"\s+\+\s+", " más "),
           (r"(?<=\d)\s?[~∼]\s?(?=\d)", " a "), (r"\s*(?<!~)[~∼≈≃](?!~)\s*", " unos "), (r"(?<!\w)≥\s?", "al menos "),
           (r"(?<!\w)≤\s?", "como mucho "), (r"(?<=\d)\s?×\s?(?=\d)", " por "), (r"\s@\s", " arroba "),
           (r"\bp\. ?ej\.", "por ejemplo"), (r"\betc\.(?=\s*\S)", "etcétera,"), (r"\betc\.", "etcétera."),
           (r"\baprox\.", "aproximadamente"), (r"\bSr\.(?= [A-ZÁÉÍÓÚÑ])", "señor"), (r"\bSra\.(?= [A-ZÁÉÍÓÚÑ])", "señora"),
           (r"\bDr\.(?= [A-ZÁÉÍÓÚÑ])", "doctor"), (r"\bDra\.(?= [A-ZÁÉÍÓÚÑ])", "doctora"), (r"\bUd\.", "usted"),
           (r"\bUds\.", "ustedes"), (r"\bpág\.", "página"), (r"\bn\.?º\s?", "número ")],
    "de": [(r"\s*(?:→|->|⟶|⇒|=>)\s*", " nach "), (r"\s+&\s+", " und "), (r"\s+\+\s+", " plus "),
           (r"(?<=\d)\s?[~∼]\s?(?=\d)", " bis "), (r"\s*(?<!~)[~∼≈≃](?!~)\s*", " etwa "), (r"(?<!\w)≥\s?", "mindestens "),
           (r"(?<!\w)≤\s?", "höchstens "), (r"(?<=\d)\s?×\s?(?=\d)", " mal "), (r"\s@\s", " at "),
           (r"\bz\. ?B\.", "zum Beispiel"), (r"\bd\. ?h\.", "das heißt"), (r"\bu\. ?a\.", "unter anderem"),
           (r"\busw\.", "und so weiter"), (r"\bbzw\.", "beziehungsweise"), (r"\bggf\.", "gegebenenfalls"),
           (r"\bca\.", "circa"), (r"\bevtl\.", "eventuell"), (r"\binkl\.", "inklusive"), (r"\bvs\.?(?= )", "gegen"),
           (r"\bDr\.(?= [A-ZÄÖÜ])", "Doktor"), (r"\bNr\.(?= ?\d)", "Nummer"), (r"\bStr\.", "Straße")],
}
_LEXICON = {lang: [(re.compile(p), w) for p, w in rules] for lang, rules in _LEXICON_SOURCE.items()}

_MONTHS_DE = ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober",
              "November", "Dezember"]
_YEAR_CONTEXT_EN = re.compile(r"\b(in|since|by|from|until|before|after|of|year|January|February|March|April|May|"
                              r"June|July|August|September|October|November|December) (1[1-9]\d\d|20\d\d)\b")


def _number(value: str, lang: str) -> str:
    """"12", "3.5" or "3,5" in words (the decimal separator is accepted either way)."""
    match = re.fullmatch(r"(-?)(\d+)(?:[,.](\d+))?", value)
    if not match:
        return value
    sign, whole, fraction = match.groups()
    words = spell(int(whole), lang)
    if fraction:
        if lang == "es" and len(fraction) <= 2 and not fraction.startswith("0"):
            words += f" coma {spell(int(fraction), lang)}"
        else:
            words += f" {_WORDS[lang]['point']} " + " ".join(spell(int(d), lang) for d in fraction)
    return (_WORDS[lang]["minus"] + " " if sign else "") + words


def _with_unit(lang: str) -> Callable[[re.Match], str]:
    def replace(match: re.Match) -> str:
        value, symbol = match.group(1), match.group(2)
        plural, singular = next((p, s) for pattern, p, s in _UNITS[lang] if re.fullmatch(pattern, symbol))
        whole, _, cents = value.replace(",", ".").partition(".")
        if plural in _CURRENCIES and len(cents) == 2:
            money = f"{_with_unit(lang)(re.match(r'(.*) (.*)', f'{whole} {symbol}'))}"
            if int(cents):
                money += {"en": f" and {spell(int(cents), lang)} cents", "es": f" con {spell(int(cents), lang)}",
                          "de": f" {spell(int(cents), lang)}"}[lang]
            return money
        if value.lstrip("-") == "1":
            return f"{_ONE[lang](singular)} {singular}"
        return f"{_number(value, lang)} {plural}"
    return replace


def _time(lang: str) -> Callable[[re.Match], str]:
    def replace(match: re.Match) -> str:
        h, m = int(match.group(1)), int(match.group(2))
        hours = "ein" if (lang == "de" and h == 1) else spell(h, lang)
        if lang == "en":
            if m == 0:
                return f"{hours} o'clock" if h <= 12 else f"{hours} hundred"
            return f"{hours} {'oh ' + spell(m, lang) if m < 10 else spell(m, lang)}"
        if lang == "es":
            return f"{hours} en punto" if m == 0 else f"{hours} y {spell(m, lang)}"
        return f"{hours} Uhr" if m == 0 else f"{hours} Uhr {spell(m, lang)}"
    return replace


_ISO_DATE = re.compile(r"\b(\d{4})-(\d{2})-(\d{2})\b")
_TIME_COLON = re.compile(r"\b([01]?\d|2[0-3]):([0-5]\d)\b")
_TIME_COLON_DE = re.compile(r"\b([01]?\d|2[0-3])[:.]([0-5]\d)(?: ?Uhr\b)?")
_GROUPED_SPACE = re.compile(r"(?<![\d,.])(\d{1,3}(?:[   ]\d{3})+)(?![\d]|[,.]\d)")
_GROUPED_COMMA = re.compile(r"(?<![\d,.])(\d{1,3}(?:,\d{3})+)(?![\d]|[,.]\d)")
_GROUPED_DOT = re.compile(r"(?<![\d,.])(\d{1,3}(?:\.\d{3})+)(?![\d]|[,.]\d)")
_CURRENCY_FIRST = re.compile(r"([$€£])\s?(-?\d+(?:[.,]\d+)?)")
_DECIMAL = re.compile(r"(?<![\w.,])(-?\d+[,.]\d+)(?![\d.,]*\d)")
_INTEGER = re.compile(r"(?<![\w.,])(\d+)(?![\w]|[.,]\d)")
_NEGATIVE = re.compile(r"(?<![\w-])-(?=\d)")
_ONE_BEFORE_NOUN = re.compile(r"(?<![\w.,])1 (?=([^\W\d]+))")
_ORDINAL_EN = re.compile(r"\b(\d+)(st|nd|rd|th)\b")
_ORDINAL_ES = re.compile(r"\b(\d+)\.?([ºª])")
_ORDINAL_DE_DATE = re.compile(r"\b([1-9]|[12]\d|3[01])\. (?=" + "|".join(_MONTHS_DE) + r")")
_YEAR_DE = re.compile(r"(?<![\w.,])(1[1-9]\d\d)(?![\w]|[.,]\d)")


def spell_numbers(text: str, lang: str) -> str:
    if lang not in LANGUAGES or not any(c.isdigit() for c in text):
        return text
    if lang == "de":
        text = _YEAR_DE.sub(lambda m: spell(int(m.group(1)), lang, "year"), text)
    text = _GROUPED_SPACE.sub(lambda m: re.sub(r"[   ]", "", m.group(1)), text)
    grouped = _GROUPED_COMMA if lang == "en" else _GROUPED_DOT
    text = grouped.sub(lambda m: re.sub(r"[,.]", "", m.group(1)), text)
    text = _CURRENCY_FIRST.sub(lambda m: f"{m.group(2)} {m.group(1)}", text)  # "$5" -> "5 $" -> "five dollars"
    text = _ISO_DATE.sub(lambda m: f"{m.group(1)}/{m.group(2)}/{m.group(3)}", text)  # kept whole, not a range
    text = fr.RANGE.sub(f" {_WORDS[lang]['to']} ", text)  # "42-46 %" -> "42 to 46 %", not one hyphenated word
    if lang == "en":
        text = _ORDINAL_EN.sub(lambda m: spell(int(m.group(1)), lang, "ordinal"), text)
        text = _YEAR_CONTEXT_EN.sub(lambda m: f"{m.group(1)} {spell(int(m.group(2)), lang, 'year')}", text)
    elif lang == "es":
        text = _ORDINAL_ES.sub(lambda m: spell(int(m.group(1)), lang, "ordinal") if m.group(2) == "º"
                               else re.sub(r"o$", "a", spell(int(m.group(1)), lang, "ordinal")), text)
        text = re.sub(r"\b(prim|terc)ero (?=[^\W\d])", r"\1er ", text)  # "tercer día"
    else:
        text = _ORDINAL_DE_DATE.sub(lambda m: spell(int(m.group(1)), lang, "ordinal") + "n ", text)
    text = (_TIME_COLON_DE if lang == "de" else _TIME_COLON).sub(_time(lang), text)
    units = "|".join(p for p, _, _ in _UNITS[lang])
    text = re.sub(r"(?<![\w,.])(-?\d+(?:[,.]\d+)?) ?(" + units + r")", _with_unit(lang), text)
    text = _DECIMAL.sub(lambda m: _number(m.group(1), lang), text)
    text = _NEGATIVE.sub(_WORDS[lang]["minus"] + " ", text)
    if lang in ("es", "de"):  # "1 día" -> "un día", "1 Woche" -> "eine Woche" (gender guessed from the ending)
        feminine = r"^(?!(?:día|mapa|problema|programa|sistema|tema|idioma|clima|planeta)$).*a$" if lang == "es" else r"e$"
        text = _ONE_BEFORE_NOUN.sub(lambda m: (_ONE[lang]("hora" if re.search(feminine, m.group(1)) else "")
                                               if lang == "es" else ("eine" if re.search(feminine, m.group(1)) else "ein"))
                                    + " ", text)
    text = _INTEGER.sub(lambda m: _digits(m.group(1), lang) if m.group(1).startswith("0") and len(m.group(1)) > 1
                        else spell(int(m.group(1)), lang), text)
    return text


_SLASH = re.compile(r"(?<=[^\W\d]) ?/ ?(?=[^\W\d])")


def prepare_for_synthesis(text: str, lang: str) -> str:
    """Same last touch as the French one, in ``lang`` (``en``, ``es`` or ``de``)."""
    for pattern, words in _LEXICON.get(lang, []):
        text = pattern.sub(words, text)
    text = spell_numbers(text, lang)
    text = _SLASH.sub(f" {_WORDS[lang]['or']} ", text) if lang in _WORDS else text
    text = fr._CAMEL.sub(fr._split_camel, text)
    text = fr._DASH.sub(", ", text)
    text = fr._OPEN.sub(", ", text)
    text = fr._CLOSE.sub(", ", text)
    text = fr._COMMA_RUNS.sub(", ", text)
    text = fr._COMMA_BEFORE_END.sub(r"\1", text)
    text = re.sub(r" {2,}", " ", text)
    return text.strip(" ,")
