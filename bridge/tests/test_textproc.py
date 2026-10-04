from bipbridge.textproc import SentenceSplitter, normalize_for_speech, normalize_segment, split_sentences


def stream(text: str, step: int = 1, max_chars: int = 180):
    sp = SentenceSplitter(max_chars)
    out = []
    for i in range(0, len(text), step):
        out += sp.feed(text[i:i + step])
    return out + sp.flush()


def test_basic_punctuation_and_decimals():
    text = "Bonjour à tous. Il fait 3.5 degrés dehors ! Et toi ? Bon… À demain"
    expected = ["Bonjour à tous.", "Il fait 3.5 degrés dehors !", "Et toi ?", "Bon…", "À demain."]
    assert split_sentences(text) == expected
    # Same result whatever the delta boundaries (character by character, small chunks).
    assert stream(text, 1) == expected
    assert stream(text, 3) == expected


def test_decimal_split_across_deltas_is_not_cut():
    sp = SentenceSplitter()
    assert sp.feed("La dose est de 3.") == []  # undecided: next char unknown
    assert sp.feed("5 mg. Ensuite") == ["La dose est de 3.5 mg."]
    assert sp.flush() == ["Ensuite."]


def test_abbreviations_and_initials():
    assert split_sentences("M. Dupont arrive, etc. et puis J. Martin aussi. Fin.") == [
        "M. Dupont arrive, etc. et puis J. Martin aussi.", "Fin."]


def test_closing_quotes_and_emphasis_stay_with_sentence():
    assert split_sentences("Il a dit « oui. » Puis **c'est fait.** Voilà") == [
        "Il a dit « oui. »", "Puis c'est fait.", "Voilà."]


def test_markdown_lists_headings_links_code():
    text = (
        "## Plan du soir\n\n"
        "- **Coucher** à 22 h\n"
        "- Lire [cet article](https://example.com/a.b) ou https://x.y/z\n"
        "1. Respirer\n"
        "2. Dormir\n"
        "```bash\nrm -rf /tmp/x. y\n```\n"
        "Utilise `sleep_score` demain.\n"
    )
    assert split_sentences(text) == [
        "Plan du soir.", "Coucher à 22 h.", "Lire cet article ou.", "Respirer.", "Dormir.",
        "Utilise sleep_score demain.",
    ]


def test_table_rows_read_as_lists():
    assert split_sentences("| Jour | Heures |\n|---|---|\n| Lundi | 7 |\n") == ["Jour, Heures.", "Lundi, 7."]


def test_long_text_cut_at_word_boundary_near_max():
    words = ("mot " * 100).strip()
    parts = split_sentences(words, max_chars=60)
    assert len(parts) > 1
    for part in parts:
        assert len(part) <= 61  # 60 + final period
        assert not part.startswith(" ")
    assert " ".join(p.rstrip(".") for p in parts).split() == words.split()


def test_long_text_prefers_comma():
    text = "Premier morceau assez long pour dépasser la limite fixée, " + "suite " * 20
    first = split_sentences(text, max_chars=80)[0]
    assert first == "Premier morceau assez long pour dépasser la limite fixée."


def test_streamed_long_text_waits_for_word_end():
    sp = SentenceSplitter(max_chars=50)
    out = sp.feed("a" * 10 + " " + "b" * 45)  # 56 chars, no space after the long word yet
    assert out == ["a" * 10 + "."] or out == []
    rest = sp.feed("bbb fin") + sp.flush()
    assert "".join(out + rest).replace(".", "").replace(" ", "") == "a" * 10 + "b" * 48 + "fin"


def test_normalize_segment_drops_empty_and_symbols():
    assert normalize_segment("---") == ""
    assert normalize_segment("**") == ""
    assert normalize_segment("<b>Gras</b> et _italique_") == "Gras et italique."
    assert normalize_segment("snake_case_name reste") == "snake_case_name reste."


def test_normalize_for_speech_joins():
    assert normalize_for_speech("# Titre\nBonjour **toi**.\n\nÇa va ?") == "Titre. Bonjour toi. Ça va ?"


def test_trailing_space_after_sentence_is_emitted_immediately():
    sp = SentenceSplitter()
    assert sp.feed("Je dois demander. ") == ["Je dois demander."]
    assert sp.feed("Il dit « oui. ") == []  # waits: the guillemet may close
    assert sp.feed("» Ensuite") == ["Il dit « oui. »"]
