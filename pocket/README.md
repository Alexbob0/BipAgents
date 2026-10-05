🇬🇧 English · [🇫🇷 Français](README.fr.md)

# Pocket TTS server — the Bips' voices

A small HTTP server around [Kyutai Pocket TTS](https://github.com/kyutai-labs/pocket-tts): 100M parameters,
**on CPU, no GPU**, in French, English, Spanish and German. The bridge (`bridge/`) calls it for all
`pocket:…` voices; the app never talks to it directly.

| Machine | First audio | Speed | Memory (4 languages) |
|---|---|---|---|
| Mac mini / MacBook Air **Apple M4** (measured, 16 GB) | ≈ 50 ms | ≈ 6.5× real time | ≈ 0.6 GB resident |
| **x86** Linux server (measured through the bridge) | 52–59 ms with `--quantize` (128–148 ms without) | ≈ 8.7× | ≈ 1.6 GB resident |
| x86 VPS, 2 to 4 dedicated vCPUs (estimated) | a few hundred ms | > real time | less with `--quantize` |

One generation at a time (Pocket is not designed for parallel use and keeps the CPU busy): the bridge queues
its requests anyway. Plan for 2 free cores for it alongside Hermes.

## Installation

Python ≥ 3.10. The repository cloned into `~/BipAgents` (the voices live in `~/BipAgents/voices`).

```bash
cd ~/BipAgents/pocket
python3 -m venv .venv            # or: uv venv --python 3.12 .venv
.venv/bin/pip install -r requirements.txt
.venv/bin/python pocket_server.py --voices ../voices --languages fr,en,es,de
# first launch: downloads the models from Hugging Face (≈ 440 MB per language, once)
curl -s http://127.0.0.1:8098/health
```

Service at boot: `launchd/io.github.bipagents.pocket.plist` (macOS) or `systemd/pocket-tts.service` (Linux),
see [`docs/self-hosting.md`](../docs/self-hosting.md). On the bridge side: `[pocket] url = "http://127.0.0.1:8098"`.

Options: `--languages fr,en` (load only some languages), `--port 8098`, `--host 127.0.0.1` (keep it
local: only the bridge calls it), `--quantize` (int8 weights: less memory, ≈ 25% faster on x86),
`--threads 2` (cores used: leaves room for Hermes; same speed measured on Apple M4).

Measure: `python3 bench.py` (first audio and speed, no dependencies). On an x86 Linux VPS, install **CPU**
PyTorch before the rest (`pip install torch --index-url https://download.pytorch.org/whl/cpu`), otherwise pip pulls
the CUDA libraries (several GB). VPS tips: [`docs/self-hosting.md`](../docs/self-hosting.md#on-a-vps).

The Bips' voices are precomputed states (`voices/<language>/<voice>.safetensors`): reading them only requires the
public model `kyutai/pocket-tts-without-voice-cloning`, downloaded automatically. The cloning model (Hugging Face
access must be requested) is only needed to **create** new voices, see [`voices/README.md`](../voices/README.md).

## API (Kyutai's, used by the bridge)

- `GET /health` → `{"status": "ok", "languages": ["fr", "en", …], "voices": {"fr": ["loutre", …], …}}`
- `POST /v1/audio/speech` `{"input": "…", "voice": "loutre", "response_format": "wav" | "mp3"}` → the whole file
- `POST /v1/audio/stream` `{"input": "…", "voice": "en/loutre"}` → raw PCM16 mono 24 kHz, chunk by chunk;
  generation stops if the client disconnects (voice interruption, cancelled reply)

`voice`: `loutre` (French) or `<code>/loutre` with `en`, `es`, `de`. Unknown voice → 404; the bridge then returns
an error and the app reads the text with the iPhone's voice. The text arrives already prepared by the bridge (numbers
spelled out, symbols, abbreviations).

Tests (no model, no download): `python -m pytest pocket/tests` (with `fastapi`, `httpx` and `pytest`, for
example from the bridge's venv).

Licenses: code MIT; Pocket TTS weights CC-BY-4.0 (Kyutai); Bips' voices: see `voices/`.
