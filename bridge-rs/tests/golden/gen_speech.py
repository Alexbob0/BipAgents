"""Golden fixtures for the speech text processing (textproc, numbers_fr, speech_intl, pronunciations, the pure parts
of tts): inputs -> Python's outputs, hand-written cases plus thousands of random ones built from the tokens these
modules care about (numbers in every format, units, dates, markdown, emoji, abbreviations, odd spaces…).

Run from bridge/ with the bridge's venv:
    python ../bridge-rs/tests/golden/gen_speech.py
"""
import json
import gzip
import os
import random
import sys

sys.path.insert(0, os.getcwd())
from bipbridge import numbers_fr, speech_intl, textproc, tts  # noqa: E402
from bipbridge.pronunciations import Lexicon  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
rng = random.Random(20261021)

HAND = [
    "Bonjour à tous. Il fait 3.5 degrés dehors ! Et toi ? Bon… À demain",
    "## Plan du soir\n\n- **Coucher** à 22 h\n- Lire [cet article](https://example.com/a.b) ou https://x.y/z\n1. Respirer\n"
    "2. Dormir\n```bash\nrm -rf /tmp/x. y\n```\nUtilise `sleep_score` demain.\n",
    "| Jour | Heures |\n|---|---|\n| Lundi | 7 |\n",
    "Il a dit « oui. » Puis **c'est fait.** Voilà",
    "M. Dupont arrive, etc. et puis J. Martin aussi. Fin.",
    "Le billet coûte 1 200 € ou 1 € de frais, 1 250 000 personnes.",
    "Rendez-vous le 04/10/2026 à 14h30, rappel le 01/11 ; 7j/7 et 24h/24, 3/4 du trajet.",
    "Entre 42-46% des cas, de 9-12h, le 12-10-2026, le 2026-10-12, -5 °C, ~15 min, 2~3 jours, ≈ 30 min, ≥ 5 km.",
    "RDV @ 14h avec @Léa @ la maison, prix@10 €, écris à alex.martin+bip@example.co.uk.",
    "Cherche sur DuckDuckGo ou YouTube, OpenAI, WhatsApp, iPhone, SNCF, LinkedIn et AirPods.",
    "Pluie -> annulation => on reste au chaud.\n→ Premier point\n<- retour ← ici",
    "Environ 1 mm, 3 m de haut, 250 g de farine, 1 L d'eau, 50 cl, 2 Go, 500 Mo, 1 kWh, 12 m², 3 m³, 1 To, 5 KB.",
    "Les runs Hermes et le cron du matin, check tes mails, le bug du bridge, Garmin/Oura, ChatGPT, Notion, une notion.",
    "Bed at 11:15, 64 °F. You spent $420.50 (12% less) in 2026. 1,250 steps vs 1 h on the 3rd day. Due 2026-10-12.",
    "A las 23:15, 18 °C. Gastaste 420,50 € el 3º día, la 1ª vez. 1 día, 1 semana, 1.250 pasos, 3,5 km. Para el 2026-10-12.",
    "Um 23:15 Uhr, 18 °C. 420,50 € am 3. Oktober 2026. 1 Tag, 1 Woche, 1.250 Schritte, z. B. 1999, Nr. 5, Str. 3.",
    "Famille 👨‍👩‍👧 ok, Note 1️⃣ ici, Bravo 🎉 ! Super 👍. 🎧 Podcast du matin ✅",
    "Intro **courte** :\n\n- Premier point\n- Second point.\n\n```sh\nrm -rf x\n\nls\n```\nFin ?",
    "***très*** important, __gras__, _italique_, ~~barré~~, snake_case_name, <b>html</b>, ![img](x.png), [ref][1].",
    "Appelle le 06 12 34, version 3.3.0, 0042 colis, 1er, 2e, 21e, 1re, 3ème, 100e, 1000e, 80e, 9e.",
    "Le 1er octobre 2026, la 1re séance ; 99999999999999 étoiles, 1234567890123 grains, 0,5 kg, 0 °C, 1 min, 1 ms.",
    "Données Garmin/Oura, billet aller/retour, A/R, et/ou à 50 km/h, 3 × 10 reps, PSG vs OM, Wellness + Vie.",
    "Mme Martin, Mmes, Mlle, Dr Lopez, Pr. Dupont, St-Jean, Ste Marie, Bd Haussmann, av. Foch, env. 2 h, p. ex., c.-à-d., n°4, tél. 01, cf. page, svp, SVP.",
    "Texte très long " + "avec beaucoup de mots, de virgules ; et de points-virgules : " * 6 + "fin",
    "x" * 300 + " suite",
    "Un. Deux. Trois.",
    "",
    "   ",
    "> citation\n>> double\n# Titre\n### Sous-titre ###\n---\n***\n___\n=== \n",
]

TOKENS = [
    "Bonjour", "le", "la", "à", "et", "de", "Il", "fait", "mots", "Hello", "the", "day", "días", "Tag", "Woche", "semana",
    "hora", "problema", "Stunde", "Mehl", "agua", "1", "0", "01", "007", "12", "21", "71", "80", "91", "100", "200", "1000",
    "1990", "2026", "80000", "1999", "1234567", "1 200", "1 200", "1 200", "1,250", "1.250", "3,5", "3.5", "-2",
    "-5", "0,5", "420,50", "420.50", "$5", "€12", "£3", "12 €", "5$", "%", "12%", "3,5 %", "°C", "18 °C", "-2°", "64 °F",
    "° C", "km", "10 km", "km/h", "50 km/h", "12 m²", "3 m³", "1 mm", "3 m", "250 g", "1 L", "1 l", "50 cl", "2 Go", "500 Mo",
    "1 To", "5 KB", "2 GB", "1 kWh", "3 kW", "45 ms", "52 min", "1 min", "145 bpm", "60 lpm", "5 kg", "2 lbs", "60 mph",
    "1 h", "2 h", "1h", "14h30", "9h", "07:05", "23:15", "11:15", "12:00", "23.15 Uhr", "1er", "1re", "21e", "3ème", "2nd",
    "3rd", "4th", "21st", "3º", "1ª", "3.", "Oktober", "October", "in", "since", "1990s", "04/10/2026", "01/11", "12-10-2026",
    "2026-10-12", "2026-13-40", "42-46", "9-12h", "2025–2026", "8/10", "3/4", "1/2", "7j/7", "24h/24", "A/R", "et/ou",
    "aller/retour", "Garmin/Oura", "~15", "2~3", "≈", "≥ 5", "≤ 3", "3 × 10", "+", "&", "→", "->", "=>", "<-", "←", "—",
    "–", "-", "(", ")", "[", "]", "(TGV)", "@", "@Léa", "bob@example.com", "prix@10", "M.", "Mme", "Dr", "St-Jean", "Ste",
    "etc.", "env.", "p. ex.", "c.-à-d.", "n°4", "No. 5", "#3", "Nr. 2", "z. B.", "e.g.", "i.e.", "Mr. Smith", "Sr. Pérez",
    "Ud.", "usw.", "vs", "DuckDuckGo", "OpenAI", "WhatsApp", "YouTube", "iPhone", "SNCF", "ChatGPT", "runs", "cron",
    "Live", "check", "Notion", "notion", "**gras**", "*it*", "_it_", "__b__", "`code`", "```", "~~x~~", "[lien](http://x.y)",
    "https://example.com/a.b", "www.site.fr", "<b>", "</b>", "| a | b |", "|---|", "- ", "* ", "1. ", "2) ", "# ", "> ",
    "🎉", "👍", "😴", "✅", "👨‍👩‍👧", "1️⃣", "«", "»", "« oui. »", "“", "”", "’", "'", "\"", "…", "!", "?",
    ".", ",", ";", ":", "...", "?!", "²", "³", "½", "٣", "١٢", "é", "İ", "ß", "Ä", "ñ", " ", " ", "\t",
    "\n", "\n\n", "\r\n", " ", "  ",
]


def random_text(max_tokens: int = 18) -> str:
    out = []
    for _ in range(rng.randint(1, max_tokens)):
        out.append(rng.choice(TOKENS))
        out.append(rng.choice([" ", " ", " ", "", ", ", ". ", "\n", " "]))
    return "".join(out)


ALPHABET = "aàbcdeéèfghijklmnopqrstuvwxyzAÉBCDEFGHMNOPRSTUVWXYZ0123456789 .,;:!?…-–—/()[]*_`#>|@&+~%$€°²«»\"'’\n\t "


def random_chars() -> str:
    return "".join(rng.choice(ALPHABET) for _ in range(rng.randint(0, 60)))


texts = HAND + [random_text() for _ in range(2500)] + [random_chars() for _ in range(800)]

cases = {}
cases["normalize_segment"] = [[t, textproc.normalize_segment(t)] for t in texts]
cases["split_sentences"] = []
for t in texts:
    m = rng.choice([40, 60, 80, 180])
    cases["split_sentences"].append([[t, m], textproc.split_sentences(t, m)])
cases["stream"] = []
for t in texts[:1500]:
    step, m = rng.choice([1, 2, 3, 5, 7, 13]), rng.choice([40, 180])
    sp = textproc.SentenceSplitter(m)
    out = []
    for i in range(0, len(t), step):
        out += sp.feed(t[i:i + step])
    out += sp.flush()
    cases["stream"].append([[t, step, m], out])
cases["strip_emoji"] = [[t, textproc.strip_emoji(t)] for t in texts]
cases["normalize_for_speech"] = []
for t in texts:
    paragraphs = rng.random() < 0.5
    cases["normalize_for_speech"].append([[t, 180, paragraphs], textproc.normalize_for_speech(t, 180, paragraphs)])

cases["fr.spell_int"] = [[n, numbers_fr.spell_int(n)] for n in
                         sorted(set(range(0, 2101)) | {rng.randrange(0, 10 ** rng.randint(4, 15)) for _ in range(1500)}
                                | {10 ** k + d for k in range(3, 16) for d in (-1, 0, 1)})]
cases["fr.spell_ordinal"] = [[[n, f], numbers_fr.spell_ordinal(n, feminine=f)] for n in
                             sorted(set(range(1, 1101)) | {rng.randrange(1, 10 ** 9) for _ in range(300)}) for f in (False, True)]
cases["fr.spell_numbers"] = [[t, numbers_fr.spell_numbers(t)] for t in texts]
# fr/intl.prepare_for_synthesis: the Rust bridge left the Python on purpose on 2026-10-25 (pronounce.rs); these two
# sections of speech.json.gz hold the Rust outputs since, and writing them from here would bring the old ones back.
cases["fr.prepare_for_synthesis"] = [[t, numbers_fr.prepare_for_synthesis(t)] for t in texts]
cases["intl.prepare_for_synthesis"] = []
cases["intl.spell_numbers"] = []
for t in texts:
    for lang in ("en", "es", "de"):
        cases["intl.prepare_for_synthesis"].append([[t, lang], speech_intl.prepare_for_synthesis(t, lang)])
for t in texts[:1200]:
    lang = rng.choice(["en", "es", "de", "fr", "it"])
    cases["intl.spell_numbers"].append([[t, lang], speech_intl.spell_numbers(t, lang)])

WORDS = [
    {"fr": {"Garmin": "Garmine", "Oura": "Ou-ra", "Mme Dupuis": "Madame Dupuis", "Mme": "Mâdame", "C++": "cé plus plus",
            "a.b": "a point b", "x (y)": "x y", "Été": "été"}},
    {"en": {"Garmin": "Gar-min", "GIF": "jif"}, "fr": {"Garmin": "Garmine"}},
]
cases["pronunciations"] = []
for i, t in enumerate(texts[:1500]):
    words = WORDS[i % len(WORDS)]
    lang = rng.choice(["fr", "en", "de"])
    cases["pronunciations"].append([[words, t + " garmin GARMIN Mme Dupuis c++ a.b x (y) été", lang],
                                    Lexicon(words).apply(t + " garmin GARMIN Mme Dupuis c++ a.b x (y) été", lang)])

VOICES = ["pocket:loutre", "5476", "pocket:en/loutre", "pocket:es/ours", "pocket:de/lutin", "pocket:xx/lutin", "server:en/af_bella",
          "server:fr/v1", "server:af_bella", "server:xx/y", "drama:fr-mascotte-s17", "drama:en-naturelle-s47", "drama:de",
          "drama:", "server:", "pocket:", "", "pocket:en/", "server:es/mascot", "drama:es-mascotte"]
cases["voice_language"] = [[v, tts.voice_language(v)] for v in VOICES]
cases["server_voice_id"] = [[v, tts.server_voice_id(v)] for v in VOICES if tts.is_server_voice(v)]
cases["server_fallback"] = [[v, tts.server_fallback(v)] for v in VOICES]
NAMES = [{"voices": ["a", "b"]}, {"voices": [{"id": "x"}, {"name": "y"}, {"voice_id": "z"}, {"id": ""}, 3, "w"]},
         {"aliases": {"5476": "a.wav", "posee": "b.wav"}}, {"data": [{"id": "m1"}]}, ["p", "q"], {"other": 1}, None, "x",
         {"voices": {"k1": 1, "k2": 2}}, {"voices": [{"id": 5}, {"name": None, "voice_id": "v"}]}]
cases["voice_names"] = [[n, tts.voice_names(n)] for n in NAMES]

total = 0
with gzip.open(os.path.join(HERE, "speech.json.gz"), "wt", encoding="utf-8") as fh:
    fh.write("{\n")
    for i, (name, items) in enumerate(cases.items()):
        total += len(items)
        fh.write(json.dumps(name) + ": [\n" + ",\n".join(json.dumps(item, ensure_ascii=False) for item in items) + "\n]")
        fh.write(",\n" if i + 1 < len(cases) else "\n")
    fh.write("}\n")
print(total, "cases")
