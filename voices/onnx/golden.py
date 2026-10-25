"""The golden examples the ports of textprep.py are checked against, written by Pocket TTS itself (4.0.1): for each
language, texts as the agents write them (and the hard cases: decimals, quotes, long sentences, bytes outside the
vocabulary, special tokens) with the chunks pocket makes of them, and a few encode/decode pairs. Run with the export's
venv (requirements.txt):

    voices/onnx/.venv/bin/python voices/onnx/golden.py

Writes golden/<language>.json and copies each language's tokenizer.json to tokenizers/<language>.json (Kyutai,
CC-BY-4.0), then checks textprep.py gives the same."""
from __future__ import annotations

import json
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import textprep  # noqa: E402

SHARED = [
    "Bonjour", "ok", "  deux  espaces  ", "Tab\tici et là", "ligne\nsuivante\r\nfin",
    "Il fait 3.5 degrés. Puis 18,5 demain.", "Version 2.0.1 sortie… enfin !", "9h30, 14h45 et 18h.",
    "Emoji 🎉 et 漢字 ici", "Literal <s> and </s> tokens", "a<0x41>b", "UPPER lower Mixed",
    "Fin sans point", "Fin avec virgule,", "Fin avec deux-points :", "Un tiret —", "« Guillemets » et \"doubles\"",
    "(parenthèses) et [crochets]", "Point-virgule; ici; là", "Question ? Réponse ! Suite...",
    "e-mail: alex@example.com, site https://example.com/a?b=c",
    "De 9h – 10h, puis — enfin — la pause - courte.", "It’s Alex’s ‘thing’ – really.",
]
LONG = {
    "french": "Demain matin, tu as trois rendez-vous : le premier à neuf heures et demie avec l'équipe produit, le "
              "deuxième à onze heures chez le dentiste, et le troisième en visio à quinze heures, puis la météo "
              "annonce dix-huit degrés, un peu de pluie en fin d'après-midi, un vent léger venu de l'ouest et des "
              "éclaircies en soirée, alors pense au parapluie et à recharger ton vélo avant de partir.",
    "english": "Tomorrow morning you have three meetings: the first at half past nine with the product team, the "
               "second at eleven at the dentist, and the third on video at three, then the forecast says eighteen "
               "degrees, some rain late in the afternoon, a light westerly wind and sunny spells in the evening, so "
               "remember your umbrella and charge your bike before you leave.",
    "german": "Morgen früh hast du drei Termine: den ersten um halb zehn mit dem Produktteam, den zweiten um elf "
              "beim Zahnarzt und den dritten per Video um fünfzehn Uhr, dann sagt der Wetterbericht achtzehn Grad, "
              "etwas Regen am späten Nachmittag, leichten Westwind und Aufheiterungen am Abend voraus, also denk an "
              "den Schirm und lade dein Fahrrad, bevor du losfährst.",
    "spanish": "Mañana por la mañana tienes tres citas: la primera a las nueve y media con el equipo de producto, la "
               "segunda a las once en el dentista y la tercera por vídeo a las tres, luego el pronóstico anuncia "
               "dieciocho grados, algo de lluvia a última hora de la tarde, un viento suave del oeste y claros por la "
               "noche, así que no olvides el paraguas y carga tu bici antes de salir.",
}
OWN = {
    "french": ["Salut ! J’ai regardé ta journée : trois rendez-vous, et un peu de temps pour marcher cet "
               "après-midi.", "Rappelle-toi : le vétérinaire de Pixel, c'est jeudi à 17 h.",
               "Ça coûte 12,50 € ; c’est noté.", "oui", "D'accord. Je m'en occupe. À tout à l'heure !"],
    "english": ["Hi there! I looked at your day: three meetings, and a little time for a walk this afternoon.",
                "Don't forget: Pixel's vet is on Thursday at 5 pm.", "It costs $12.50; noted.", "yes",
                "Okay. I'm on it. See you later!"],
    "german": ["Huhu! Ich habe mir deinen Tag angesehen: drei Termine und ein bisschen Zeit für einen "
               "Spaziergang heute Nachmittag.", "Vergiss nicht: Pixels Tierarzt ist am Donnerstag um 17 Uhr.",
               "Das kostet 12,50 €; notiert.", "ja", "Größe, Straße, Übermaß. Alles klar!"],
    "spanish": ["¡Holi! He mirado tu día: tres citas y un rato para caminar esta tarde.",
                "No olvides: el veterinario de Pixel es el jueves a las 17 h.", "¿Cuánto cuesta? 12,50 €.", "sí",
                "¡Vale! Me encargo. ¡Hasta luego!"],
}


def main() -> int:
    from pocket_tts import TTSModel
    from pocket_tts.default_parameters import MAX_TOKEN_PER_CHUNK
    from pocket_tts.models.text_chunking import prepare_text_prompt, split_into_best_sentences
    from pocket_tts.utils.utils import download_if_necessary

    assert MAX_TOKEN_PER_CHUNK == textprep.MAX_TOKENS
    (HERE / "golden").mkdir(exist_ok=True)
    (HERE / "tokenizers").mkdir(exist_ok=True)
    failures = 0
    for language in textprep.LANGUAGES:
        model = TTSModel.load_model(language=language)
        assert not model.pad_with_spaces_for_short_inputs and model.model_recommended_frames_after_eos is None
        assert (model.replace_characters or {}) == textprep.LANGUAGES[language]["replace"], language
        assert model.remove_semicolons == textprep.LANGUAGES[language]["semicolons"], language
        source = download_if_necessary(model.config.flow_lm.lookup_table.tokenizer_path)
        shutil.copyfile(source, HERE / "tokenizers" / f"{language}.json")
        tok = model.flow_lm.conditioner.tokenizer
        texts = SHARED + OWN[language] + [LONG[language]]
        cases = []
        for text in texts:
            out = []
            for sentence in split_into_best_sentences(tok, text, MAX_TOKEN_PER_CHUNK, model.pad_with_spaces_for_short_inputs,
                                                      remove_semicolons=model.remove_semicolons,
                                                      append_terminal_punctuation=model.append_terminal_punctuation,
                                                      capitalize_first_letter=model.capitalize_first_letter,
                                                      replace_characters=model.replace_characters):
                prepared, after = prepare_text_prompt(sentence, model.pad_with_spaces_for_short_inputs,
                                                      model.remove_semicolons, model.append_terminal_punctuation,
                                                      model.capitalize_first_letter, model.replace_characters)
                ids = tok.encode(prepared)
                out.append({"text": prepared, "ids": ids, "frames_after_eos": after + 2,
                            "max_gen_len": model._estimate_max_gen_len(len(ids))})
            cases.append({"text": text, "chunks": out})
        pairs = [{"text": t, "ids": tok.encode(t), "decoded": tok.decode(tok.encode(t))} for t in texts]
        pairs += [{"text": None, "ids": ids, "decoded": tok.decode(ids)}
                  for ids in ([1, 260, 260], [4, 260, 69], [0, 2, 3, 300], [234, 192], [234, 192, 166, 260])]
        # The text rules as the package's manifest carries them (`text`): the ports' tests build theirs from here.
        golden = {"language": language, "max_tokens": MAX_TOKEN_PER_CHUNK,
                  "text": {"replace": textprep.LANGUAGES[language]["replace"],
                           "semicolons": textprep.LANGUAGES[language]["semicolons"]},
                  "cases": cases, "tokens": pairs}
        (HERE / "golden" / f"{language}.json").write_text(json.dumps(golden, ensure_ascii=False, indent=1) + "\n",
                                                           encoding="utf-8")
        failures += check(language)
        failures += fuzz(language, tok)
    return 1 if failures else 0


def fuzz(language: str, pocket_tokenizer, count: int = 3000) -> int:
    """Random texts (letters, accents, digits, punctuation, spaces, emoji, CJK, special tokens): textprep's tokenizer
    against pocket's, encode and decode."""
    import random
    rng = random.Random(f"bipagents-{language}")
    alphabet = ("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZéèêàçùûôîïœÉÀÇßäöüÄÖÜñÑ¡¿áíóú0123456789"
                "     .,;:!?…'’\"«»()-–—\n\t\u00a0€$%@#/🎉😀漢字中文한국어Ωπ")
    words = ["<s>", "</s>", "<pad>", "<unk>", "<0x41>", "Bonjour", "the", "und", "que", "...", "  "]
    tok = textprep.Tokenizer.load(HERE / "tokenizers" / f"{language}.json")
    wrong = 0
    for _ in range(count):
        parts = [rng.choice(words) if rng.random() < 0.08 else rng.choice(alphabet)
                 for _ in range(rng.randint(1, 40))]
        text = "".join(parts)
        expected = pocket_tokenizer.encode(text)
        got = tok.encode(text)
        if got != expected:
            wrong += 1
            if wrong <= 5:
                print(f"{language} fuzz encode {text!r}: {got} != {expected}")
        if tok.decode(expected) != pocket_tokenizer.decode(expected):
            wrong += 1
            if wrong <= 5:
                print(f"{language} fuzz decode {expected}: {tok.decode(expected)!r} != {pocket_tokenizer.decode(expected)!r}")
    print(f"{language}: {count} random texts, {wrong} differences")
    return wrong


def check(language: str) -> int:
    """textprep.py against the golden file: the number of differences (each one printed)."""
    golden = json.loads((HERE / "golden" / f"{language}.json").read_text(encoding="utf-8"))
    tok = textprep.Tokenizer.load(HERE / "tokenizers" / f"{language}.json")
    wrong = 0
    for pair in golden["tokens"]:
        if pair["text"] is not None and tok.encode(pair["text"]) != pair["ids"]:
            wrong += 1
            print(f"{language} encode {pair['text']!r}: {tok.encode(pair['text'])} != {pair['ids']}")
        if tok.decode(pair["ids"]) != pair["decoded"]:
            wrong += 1
            print(f"{language} decode {pair['ids']}: {tok.decode(pair['ids'])!r} != {pair['decoded']!r}")
    for case in golden["cases"]:
        got = [c.__dict__ for c in textprep.chunks(tok, case["text"], language)]
        if got != case["chunks"]:
            wrong += 1
            print(f"{language} chunks {case['text']!r}:\n  {got}\n  != {case['chunks']}")
    print(f"{language}: {len(golden['cases'])} texts, {len(golden['tokens'])} pairs, {wrong} differences")
    return wrong


if __name__ == "__main__":
    sys.exit(main())
