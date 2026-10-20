"""textprep.py against the golden files pocket wrote (golden.py): the same chunks, tokens and decoded text, without
pocket. The apps' ports run the same files (App/…/VoiceTextTests, android/…/VoiceTextTest)."""
import json
from pathlib import Path

import pytest

import textprep

HERE = Path(__file__).resolve().parent


@pytest.mark.parametrize("language", sorted(textprep.LANGUAGES))
def test_textprep_speaks_like_pocket(language):
    golden = json.loads((HERE / "golden" / f"{language}.json").read_text(encoding="utf-8"))
    tok = textprep.Tokenizer.load(HERE / "tokenizers" / f"{language}.json")
    for pair in golden["tokens"]:
        if pair["text"] is not None:
            assert tok.encode(pair["text"]) == pair["ids"], pair["text"]
        assert tok.decode(pair["ids"]) == pair["decoded"], pair["ids"]
    for case in golden["cases"]:
        assert [c.__dict__ for c in textprep.chunks(tok, case["text"], language)] == case["chunks"], case["text"]


def test_an_empty_text_is_refused():
    with pytest.raises(ValueError):
        textprep.prepare("   ", "french")
