🇬🇧 English · [🇫🇷 Français](README.fr.md)

# The Bips' voices on the phone

The apps speak with the Bips' voices on the phone itself, without the server: Kyutai's Pocket TTS 3.3.0 converted to
ONNX (the language model in 8 bits), run by ONNX Runtime. One package per language (about 105 to 125 MB, downloaded at
first use) and the audio decoder they share (40 MB).

| File | What it does |
|---|---|
| `build.py` | makes the packages into `build/voices/` (gitignored), checks the graphs against pocket, and speaks a sample per language into `build/voices/samples/` |
| `wrappers.py` | pocket's streaming steps rewritten for the export, every state an input and an output |
| `reference.py` | speaks with a package as the apps do: the loop their engines follow |
| `textprep.py` | the text as pocket prepares it, its tokenizer included, without pocket: what the apps' ports copy |
| `golden.py`, `golden/` | examples written by pocket itself; `test_textprep.py` and the apps' tests check against them |
| `tokenizers/` | each language's tokenizer.json (Kyutai, CC-BY-4.0) |
| `store_packs.py` | the packages as the stores deliver them: Apple-hosted asset packs (`build/voices-store/apple/*.aar`) and Google Play's on-demand asset packs (`android/voicepacks/`) |

```bash
uv venv --python 3.12 voices/onnx/.venv && uv pip install --python voices/onnx/.venv/bin/python -r voices/onnx/requirements.txt
voices/onnx/.venv/bin/python voices/onnx/build.py
```

The packages are never at a public address. The apps get them from the App Store (upload the `.aar` files to App
Store Connect with Transporter, then submit them with the build) and from Google Play (`scripts/play-upload.py` puts
them in the bundle); the offer's servers get them from their guichet (copy `build/voices` to its `voices` folder).

```bash
voices/onnx/.venv/bin/python voices/onnx/store_packs.py          # both stores; --apple or --android for one
```

Measured on a Mac M4, one thread (2026-10-20): a package in 10 to 15 s; speech 6 to 8 times faster than real time;
the graph in 32 bits within 6e-6 of pocket, in 8 bits within 2 to 5 % on one step; Whisper transcribes the four
samples right. Still to measure on a real iPhone and a mid-range Android.

Credit, shipped in each package's manifest: Pocket TTS by Kyutai (https://github.com/kyutai-labs/pocket-tts), weights
CC-BY-4.0, converted to ONNX and quantized to 8 bits. The Bips' voices are synthetic (Qwen3-TTS, Apache-2.0, then
Pocket TTS). Kyutai's own voices under CC BY-NC are never shipped.
