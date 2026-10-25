🇬🇧 English · [🇫🇷 Français](README.fr.md)

# The Bips' voices on the phone

The apps speak with the Bips' voices on the phone itself, without the server: Kyutai's Pocket TTS 4.0.1 converted to
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
Store Connect with Transporter, then submit them with the build) and from Google Play (the publisher's upload script puts
them in the bundle); the offer's servers get them from their guichet (copy `build/voices` to its `voices` folder).

```bash
voices/onnx/.venv/bin/python voices/onnx/store_packs.py          # both stores; --apple or --android for one
```

Versions. Each manifest says the Pocket it was made from (`engine`: « pocket-tts 4.0.1 ») and the packages' version
(`package`: 2 since 2026-10-25, Pocket 4.0.1; 1 or none: Pocket 3.3.0). The apps send that version to the bridge
(`engine` in `POST /v1/tts/prepare`): a 3.3.0 package keeps the respelled names its voices need, a 4.x one gets the text
as written; an app that doesn't send it is taken to have 3.3.0 packages. Each package carries its own text rules
(`text`: Pocket 4 turns dashes into commas, PR #387) and Pocket 4's fade-in of each chunk's first 5 ms
(`generation.fade_in_samples`), so a phone keeps speaking with the packages it has until the store brings the new ones.
The decoder (`mimi-c54a51aa0964`) is the same for both: a build keeps a decoder already in `build/voices` with the same
weights, so phones and servers don't fetch it again. The golden files are pocket 4.0.1's, with the rules (`text`) the
apps' tests read.

Measured on a Mac M4, one thread (2026-10-20): a package in 10 to 15 s; speech 6 to 8 times faster than real time;
the graph in 32 bits within 6e-6 of pocket, in 8 bits within 2 to 5 % on one step; Whisper transcribes the four
samples right. Pocket 4.0.1 (2026-10-25): fr 120.6 MB, en 130.0, de 107.8, es 107.5, decoder 41.4 (unchanged); 32 bits
within 3e-6 of pocket, 8 bits 3 to 9 % on one step; 5.5 to 7x real time on one thread; each Bip in each language
against PyTorch 4.0.1: Whisper (medium) hears the same sample sentence from both. Still to measure on a real iPhone and a mid-range Android.

Credit, shipped in each package's manifest: Pocket TTS by Kyutai (https://github.com/kyutai-labs/pocket-tts), weights
CC-BY-4.0, converted to ONNX and quantized to 8 bits. The Bips' voices are synthetic (Qwen3-TTS, Apache-2.0, then
cloned with Pocket TTS, kyutai/pocket-tts, CC-BY-4.0). Kyutai's own voices under CC BY-NC are never shipped.
