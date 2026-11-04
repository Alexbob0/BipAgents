"""Shorter voice prompts for the slow Bips: each one's source clip cut to about 8 s, its state cloned again, and the
voices folder written anew with these states (every other file hard-linked from the old folder, untouched).

    voices/onnx/.venv/bin/python voices/onnx/short_prompts.py build/voices build/voices-short \
        [--voices english:colibri,english:galet,...]

A Bip's state is Pocket's cache after its clip: every generated frame attends to all of it, so a 20-25 s clip (the
English Bips, French galet/lumen/mousse) speaks 1.5-2x slower than an 8 s one (German, Spanish, Dutch, French
colibri/ours) on a phone's CPU (bench.py, 2026-11-04). The cut, the same for every clip (`cut`):

1. the pauses: runs of 20 ms frames more than 35 dB below the loudest one (Pocket's own `end_on_pause` floor), of 0.1 s
   or more; every pause longer than 0.4 s is shortened to 0.4 s, the leading silence dropped;
2. the window: it starts at the clip's start or after a pause of 0.25 s or more and ends at such a pause, so it holds
   whole phrases; it lasts 6.5 to 9 s; the closest to 8 s wins, then the earliest;
3. Pocket ends it on its own pause (`end_on_pause`, in `get_state_for_audio_prompt`).

out/sources/<language>/<bip>_source.wav and <bip>.safetensors are the new clip and state (export_model_state, what
voices/<language>/ holds for the servers' Pocket); out/<language>-<id>/ the package with the new caches, its manifest's
`voices` and `files` updated (so a new id); out/index.json lists every language, the others as they were. Nothing in
the old folder is written. Needs the cloning weights (kyutai/pocket-tts, accepted on Hugging Face) in the cache."""
from __future__ import annotations

import argparse
import json
import os
import sys
import wave
from pathlib import Path
from typing import Dict, List, Tuple

import numpy as np

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(HERE))

from build import sha256, write_raw  # noqa: E402

LONG = ("english:colibri,english:galet,english:lumen,english:mousse,english:ours,"
        "french:galet,french:lumen,french:mousse")
RATE = 24000
FRAME = RATE // 50          # 20 ms
FLOOR_DB = 35.0
MIN_PAUSE = 0.1
KEEP_PAUSE = 0.4            # longer pauses shortened to this
CUT_PAUSE = 0.25            # a window starts and ends at a pause this long
SPAN = (6.5, 9.0)
TARGET = 8.0


def read_wav(path: Path) -> np.ndarray:
    with wave.open(str(path)) as wav:
        assert wav.getframerate() == RATE and wav.getnchannels() == 1 and wav.getsampwidth() == 2, path
        return np.frombuffer(wav.readframes(wav.getnframes()), "<i2").astype(np.float32) / 32768


def write_wav(path: Path, audio: np.ndarray) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(RATE)
        wav.writeframes((np.clip(audio, -1, 1) * 32767).astype("<i2").tobytes())


def pauses(audio: np.ndarray) -> List[Tuple[int, int]]:
    """(start, end) in samples of each quiet run of MIN_PAUSE or more."""
    n = len(audio) // FRAME
    rms = np.sqrt((audio[:n * FRAME].reshape(n, FRAME) ** 2).mean(axis=1))
    db = 20 * np.log10(rms + 1e-12)
    quiet = db < db.max() - FLOOR_DB
    runs, i = [], 0
    while i < n:
        if not quiet[i]:
            i += 1
            continue
        j = i
        while j < n and quiet[j]:
            j += 1
        if (j - i) * FRAME >= MIN_PAUSE * RATE or i == 0 or j == n:
            runs.append((i * FRAME, j * FRAME))
        i = j
    return runs


def squeeze(audio: np.ndarray) -> np.ndarray:
    """The leading silence dropped and every pause over KEEP_PAUSE shortened to it (its middle cut out)."""
    keep, cursor = [], 0
    for start, end in pauses(audio):
        if start == 0:
            cursor = end
            continue
        if end - start > KEEP_PAUSE * RATE:
            half = int(KEEP_PAUSE * RATE) // 2
            keep.append(audio[cursor:start + half])
            cursor = end - half
    keep.append(audio[cursor:])
    return np.concatenate(keep)


def cut(audio: np.ndarray) -> Tuple[np.ndarray, Tuple[float, float]]:
    """The window of `squeeze(audio)` (see the module's rule) and its (start, end) in seconds of the squeezed clip."""
    audio = squeeze(audio)
    runs = [(s, e) for s, e in pauses(audio) if e - s >= CUT_PAUSE * RATE - FRAME]
    starts = [0] + [e for _, e in runs]
    ends = [s for s, _ in runs if s > 0] + [len(audio)]
    best = None
    for a in starts:
        for b in ends:
            length = (b - a) / RATE
            if SPAN[0] <= length <= SPAN[1]:
                key = (abs(length - TARGET), a)
                if best is None or key < best[0]:
                    best = (key, a, b)
    if best is None:
        raise SystemExit(f"no {SPAN[0]}-{SPAN[1]} s window between pauses")
    _, a, b = best
    # the window opens 50 ms before its speech (the pause before it, if any)
    a = max(0, a - RATE // 20) if a else 0
    return audio[a:b], (a / RATE, b / RATE)


def link_tree(source: Path, target: Path) -> None:
    for path in sorted(source.rglob("*")):
        out = target / path.relative_to(source)
        if path.is_dir():
            out.mkdir(parents=True, exist_ok=True)
            continue
        out.parent.mkdir(parents=True, exist_ok=True)
        os.link(path, out)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("old", help="the voices folder now (build/voices)")
    parser.add_argument("out", help="a new folder")
    parser.add_argument("--voices", default=LONG, help="language:bip,… to cut and clone again")
    parser.add_argument("--threads", type=int, default=2)
    args = parser.parse_args(argv)
    import torch
    from pocket_tts import TTSModel
    from pocket_tts.models.model_state import export_model_state
    torch.set_num_threads(args.threads)
    old, out = Path(args.old), Path(args.out)
    if out.exists():
        raise SystemExit(f"{out}: already there")
    index = json.loads((old / "index.json").read_text(encoding="utf-8"))
    wanted: Dict[str, List[str]] = {}
    for item in args.voices.split(","):
        language, bip = item.split(":")
        wanted.setdefault(language, []).append(bip)
    out.mkdir(parents=True)
    report = {}
    for language, entry in index["languages"].items():
        source = old / entry["folder"]
        if language not in wanted:
            link_tree(source, out / entry["folder"])
            continue
        model = TTSModel.load_model(language=language)
        if not model.has_voice_cloning:
            raise SystemExit(f"{language}: the cloning weights (kyutai/pocket-tts) aren't in the cache")
        scratch = out / f".{language}"
        link_tree(source, scratch)
        manifest = json.loads((scratch / "manifest.json").read_text(encoding="utf-8"))
        for bip in wanted[language]:
            clip, (start, end) = cut(read_wav(REPO / "voices" / language / f"{bip}_source.wav"))
            write_wav(out / "sources" / language / f"{bip}_source.wav", clip)
            state = model.get_state_for_audio_prompt(torch.from_numpy(clip.copy())[None])
            export_model_state(state, out / "sources" / language / f"{bip}.safetensors")
            layers = [state[f"transformer.layers.{i}.self_attn"] for i in range(len(model.flow_lm.transformer.layers))]
            cache = torch.stack([layer["cache"] for layer in layers])
            frames = int(cache.shape[3])
            assert all(int(layer["offset"].reshape(-1)[0]) == frames for layer in layers)
            kv = scratch / "voices" / f"{bip}.kv.f16"
            kv.unlink()   # a hard link to the old folder's file: never written through
            spec = write_raw(kv, cache.numpy(), "<f2")
            manifest["voices"][bip] = {"file": f"voices/{bip}.kv.f16", "frames": frames, **spec}
            manifest["files"][f"voices/{bip}.kv.f16"] = {"bytes": kv.stat().st_size, "sha256": sha256(kv)}
            report[f"{language}:{bip}"] = {"clip_s": round(len(clip) / RATE, 2), "window_s": [round(start, 2),
                                           round(end, 2)], "frames": frames,
                                           "old_frames": json.loads((source / "manifest.json").read_text(
                                               encoding="utf-8"))["voices"][bip]["frames"]}
            print(f"{language}/{bip}: {len(clip) / RATE:.1f} s clip ({start:.1f}-{end:.1f} s of the squeezed one), "
                  f"{frames} frames (was {report[f'{language}:{bip}']['old_frames']})", flush=True)
        manifest["voice_prompts"] = ("2026-11-04: " + ", ".join(wanted[language]) + " cloned again from ~8 s of their "
                                     "source clip (voices/onnx/short_prompts.py), for speed on phones")
        (scratch / "manifest.json").unlink()   # a hard link too
        (scratch / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=1) + "\n",
                                               encoding="utf-8")
        folder = f"{language}-{sha256(scratch / 'manifest.json')[:12]}"
        scratch.rename(out / folder)
        index["languages"][language] = {"folder": folder, "manifest_sha256": sha256(out / folder / "manifest.json"),
                                        "bytes": sum(p.stat().st_size for p in (out / folder).rglob("*") if p.is_file())}
    link_tree(old / index["mimi"]["folder"], out / index["mimi"]["folder"])
    (out / "index.json").write_text(json.dumps(index, indent=1) + "\n", encoding="utf-8")
    (out / "sources" / "cuts.json").write_text(json.dumps(report, indent=1) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
