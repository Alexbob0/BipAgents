"""How fast each Bip speaks from a voices folder (build.py's or short_prompts.py's), the way a phone does: reference.py's
loop, ONNX Runtime on the CPU, one thread, the same three sentences per language (short, long, with numbers) and the
same seed for every voice, so two folders compare voice by voice.

    voices/onnx/.venv/bin/python voices/onnx/bench.py build/voices [--languages english,french] [--bips galet,lumen]
        [--json out.json] [--wavs folder]

Per voice: the prompt's frames (12.5 a second: its length), the first audio's delay (the text's step over the voice's
cache, then the first frame through the decoder: what the listener waits for, sentence by sentence) and the speed
(seconds of audio per second of work, all three sentences). A longer prompt is a longer cache that every generated
frame attends to: the speed falls with it. `--wavs` keeps each sentence's audio (<language>-<bip>-<n>.wav) to listen to."""
from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from reference import Package, save_wav  # noqa: E402

BIPS = ("colibri", "galet", "lumen", "mousse", "ours")
SENTENCES = {
    "english": ["Okay, it's done!",
                "I looked at your day: three meetings this morning, then lunch with Sarah, and a little time for a "
                "walk late this afternoon, before the rain comes back.",
                "Your train leaves at 7:45 from platform 12, and the ticket costs 38 euros."],
    "french": ["D'accord, c'est fait !",
               "J'ai regardé ta journée : trois rendez-vous ce matin, puis un déjeuner avec Sarah, et un peu de temps "
               "pour marcher en fin d'après-midi, avant le retour de la pluie.",
               "Ton train part à sept heures quarante-cinq du quai douze, et le billet coûte trente-huit euros."],
    "german": ["Okay, erledigt!",
               "Ich habe mir deinen Tag angesehen: drei Termine heute Vormittag, dann Mittagessen mit Sarah, und am "
               "späten Nachmittag noch ein bisschen Zeit für einen Spaziergang, bevor der Regen zurückkommt.",
               "Dein Zug fährt um 7:45 Uhr von Gleis 12, und die Fahrkarte kostet 38 Euro."],
    "spanish": ["¡Vale, hecho!",
                "He mirado tu día: tres citas esta mañana, luego una comida con Sara, y un rato para caminar al final "
                "de la tarde, antes de que vuelva la lluvia.",
                "Tu tren sale a las 7:45 del andén 12, y el billete cuesta 38 euros."],
    "dutch": ["Oké, het is klaar!",
              "Ik heb even naar je dag gekeken: drie afspraken vanochtend, dan lunch met Sarah, en aan het eind van de "
              "middag nog wat tijd voor een wandeling, voordat de regen terugkomt.",
              "Je trein vertrekt om 7:45 van spoor 12, en het kaartje kost 38 euro."],
}


def bench_voice(package: Package, language: str, bip: str, seed: int, wavs: Path = None) -> dict:
    frames = int(package.manifest["voices"][bip]["frames"])
    audio_s, work_s, firsts = 0.0, 0.0, []
    for n, sentence in enumerate(SENTENCES[language], 1):
        started = time.perf_counter()
        first, parts = None, []
        for part in package.speak(sentence, bip, seed=seed):
            if first is None:
                first = time.perf_counter() - started
            parts.append(part)
        work_s += time.perf_counter() - started
        audio = np.concatenate(parts)
        audio_s += len(audio) / 24000
        firsts.append(first)
        if wavs:
            save_wav(wavs / f"{language}-{bip}-{n}.wav", audio)
    return {"language": language, "bip": bip, "prompt_frames": frames, "prompt_s": round(frames / 12.5, 1),
            "audio_s": round(audio_s, 2), "work_s": round(work_s, 2), "rtf": round(audio_s / work_s, 2),
            "first_audio_s": [round(f, 3) for f in firsts]}


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("voices", help="a voices folder with its index.json")
    parser.add_argument("--languages", default=",".join(SENTENCES))
    parser.add_argument("--bips", default=",".join(BIPS))
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--threads", type=int, default=1)
    parser.add_argument("--json", help="write the results there")
    parser.add_argument("--wavs", help="keep each sentence's audio in this folder")
    args = parser.parse_args(argv)
    out = Path(args.voices)
    index = json.loads((out / "index.json").read_text(encoding="utf-8"))
    wavs = Path(args.wavs) if args.wavs else None
    if wavs:
        wavs.mkdir(parents=True, exist_ok=True)
    print(f"load {os.getloadavg()[0]:.2f}", flush=True)
    results = []
    for language in args.languages.split(","):
        package = Package(out / index["languages"][language]["folder"], out / index["mimi"]["folder"],
                          threads=args.threads)
        list(package.speak("Ok.", args.bips.split(",")[0], seed=0))   # the sessions warmed up
        for bip in args.bips.split(","):
            result = bench_voice(package, language, bip, args.seed, wavs)
            results.append(result)
            print(f"{language:8} {bip:8} prompt {result['prompt_s']:5.1f} s  speed {result['rtf']:5.2f}x  "
                  f"first audio {' / '.join(f'{f:.2f}' for f in result['first_audio_s'])} s", flush=True)
    print(f"load {os.getloadavg()[0]:.2f}", flush=True)
    if args.json:
        Path(args.json).write_text(json.dumps({"load": os.getloadavg()[0], "results": results}, indent=1) + "\n",
                                   encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
