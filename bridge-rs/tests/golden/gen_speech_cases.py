"""The parametrized cases of tests/test_numbers_fr.py and tests/test_speech_intl.py (their own expected values), for
tests/voice-speech_cases.rs.

Run from bridge/ with the bridge's venv:
    python ../bridge-rs/tests/golden/gen_speech_cases.py
"""
import json
import os
import sys

sys.path.insert(0, os.getcwd())
sys.path.insert(0, os.path.join(os.getcwd(), "tests"))
import test_numbers_fr  # noqa: E402
import test_speech_intl  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))


def cases(module):
    out = {}
    for name in dir(module):
        func = getattr(module, name)
        if not name.startswith("test_") or not callable(func):
            continue
        for mark in getattr(func, "pytestmark", []):
            if mark.name == "parametrize":
                out[name] = [list(values) for values in mark.args[1]]
    return out


with open(os.path.join(HERE, "speech_cases.json"), "w", encoding="utf-8") as fh:
    json.dump({"numbers_fr": cases(test_numbers_fr), "speech_intl": cases(test_speech_intl)}, fh, ensure_ascii=False,
              indent=1)
