"""Golden fixtures for src/souls.rs: every pure function of bipbridge.souls on a list of inputs.

Run from bridge/: "<venv>/bin/python" ../bridge-rs/tests/golden/gen_souls.py"""
import json
import os
import sys

sys.path.insert(0, os.getcwd())
from bipbridge import souls  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
LANGS = [None, "", "fr", "fr-FR", "FR_ca", "en", "en_US", "es", "es-ES", "de", "de-CH", "it", "Français", "French",
         "  german ", "Spanish", "english", "pt-BR", "zz"]
out = []
for lang in LANGS:
    out.append({"fn": "agent_language", "input": [lang], "output": souls.agent_language(lang)})
    out.append({"fn": "language", "input": [lang], "output": souls.language(lang)})
    out.append({"fn": "language_block", "input": [lang], "output": souls.language_block(lang)})
    out.append({"fn": "templates", "input": [lang], "output": souls.templates(lang)})
for soul in ["Tu es Vie.", "Tu es Vie.\n\n" + souls.language_block("es"), "  spaced  \n" + souls.language_block("de") + "\nafter\n",
             souls.BLOCK_END + " reversed " + souls.BLOCK_START, souls.BLOCK_START + " no language " + souls.BLOCK_END,
             "", "\n\n", "Line\n" + souls.BLOCK_START + "\nwrite in French.\n" + souls.BLOCK_END]:
    out.append({"fn": "split_language", "input": [soul], "output": list(souls.split_language(soul))})
    for lang in [None, "fr", "es-ES", "it"]:
        out.append({"fn": "with_language", "input": [soul, lang], "output": souls.with_language(soul, lang)})
for category in souls.CATEGORIES + ["custom", "nope"]:
    for lang in [None, "fr", "en", "de"]:
        try:
            result = souls.soul_for("Léa", category, lang)
        except ValueError as exc:
            result = {"error": str(exc)}
        out.append({"fn": "soul_for", "input": ["Léa", category, lang], "output": result})
for args in [("Fougère", "suit mes plantes.", "drôle", "", "fr"), ("Ivy", " tracks my plants ", "", "never at night...", "en"),
             ("Max", "x", "   ", " r. ", "de"), ("Léo", "rôle", "", "", None)]:
    out.append({"fn": "custom_soul", "input": list(args), "output": souls.custom_soul(*args[:4], lang=args[4])})
for name, taken in [("Léa", []), ("Mon coach", []), ("Test", []), ("Wellness", ["wellness", "wellness-2"]), ("🤖", []),
                    ("  Ça  va ! ", []), ("x" * 50, []), ("Ŝtéphane-Ö_1", []), ("ﬁnance", []), ("Default", ["default-agent"]),
                    ("a.b/c", []), ("", ["agent"])]:
    out.append({"fn": "profile_name", "input": [name, taken], "output": souls.profile_name(name, taken)})

with open(os.path.join(HERE, "souls.json"), "w", encoding="utf-8") as fh:
    json.dump(out, fh, ensure_ascii=False, indent=1)
print(len(out), "cases")
