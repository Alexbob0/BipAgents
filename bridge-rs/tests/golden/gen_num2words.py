"""Golden fixtures for src/voice_speech/num2words.rs: num2words' cardinals, ordinals and years in English, Spanish
and German, as speech_intl.spell asks for them (commas removed), over wide ranges and random samples below 10**12.

Run from bridge/ with the bridge's venv:
    python ../bridge-rs/tests/golden/gen_num2words.py
"""
import json
import gzip
import os
import random

from num2words import num2words

HERE = os.path.dirname(os.path.abspath(__file__))
random.seed(20261021)

numbers = set(range(0, 1101))
numbers |= {n for n in range(1100, 100001, 97)}
for k in range(1, 13):
    p = 10 ** k
    numbers |= {p - 2, p - 1, p, p + 1, p + 2, 2 * p, 5 * p, 7 * p + 1, 11 * p, 21 * p, 99 * p, 100 * p + 7}
numbers |= {1000 ** e for e in range(1, 5)}
numbers |= {random.randrange(0, 10 ** random.randint(3, 12)) for _ in range(1500)}
numbers = sorted(n for n in numbers if 0 <= n < 10 ** 12)

cases = []
for lang in ("en", "es", "de"):
    for to in ("cardinal", "ordinal", "year"):
        for n in numbers:
            if to == "year" and n > 10 ** 7 and n % 3:
                continue  # fewer large years
            try:
                out = num2words(n, lang=lang, to=to).replace(",", "")
            except (NotImplementedError, OverflowError, ValueError) as exc:
                out = None
            cases.append([n, lang, to, out])

with gzip.open(os.path.join(HERE, "num2words.json.gz"), "wt", encoding="utf-8") as fh:
    # one case per line, [n, lang, to, words] (None: num2words refused it)
    fh.write("[\n" + ",\n".join(json.dumps(case, ensure_ascii=False) for case in cases) + "\n]\n")
print(len(cases), "cases")
