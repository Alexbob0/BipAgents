"""Words a voice says wrong, respelled by the person (`[pronunciations.<lang>]` in the bridge's config):

    [pronunciations.fr]
    Garmin = "Garmine"
    "Mme Dupuis" = "Madame Dupuis"

Whole words or phrases, any case, applied before the bridge's own lexicon (numbers_fr._ENGLISH, speech_intl), so
they win over it.
"""
from __future__ import annotations

import re
from typing import Dict, List, Pattern, Tuple


class Lexicon:
    def __init__(self, words: Dict[str, Dict[str, str]]):
        self._rules: Dict[str, List[Tuple[Pattern[str], str]]] = {
            lang: [(re.compile(r"(?<!\w)" + re.escape(word) + r"(?!\w)", re.IGNORECASE), str(spoken))
                   for word, spoken in sorted(table.items(), key=lambda item: -len(item[0]))]
            for lang, table in words.items() if isinstance(table, dict)
        }

    def apply(self, text: str, language: str) -> str:
        for pattern, spoken in self._rules.get(language, []):
            text = pattern.sub(lambda _: spoken, text)
        return text
