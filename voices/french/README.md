🇬🇧 English · [🇫🇷 Français](README.fr.md)

# BipAgents mascot voices — Kyutai Pocket TTS (French)

Five synthetic voices for the BipAgents app's agents, usable with
[Kyutai Pocket TTS](https://github.com/kyutai-labs/pocket-tts) on CPU (first audio ≈ 85–160 ms, ≈ 7–10× real time on 8 threads).
Galet, Lumen and Mousse (October 7, 2026) are small-mascot voices: calm, informative, slightly stylized like an animated
character, never realistic nor robotic. Ours and Colibri date from October 4, 2026.

| Voice | Character | Files |
|---|---|---|
| **galet** (pebble) | composed, medium-low, clear and confident, steady pace, the tone of a calm guide who explains clearly | `galet.safetensors`, `galet_source.wav`, `galet_sample.wav` |
| **lumen** | clear and bright, medium, very articulate, neutral and efficient, steady pace, ideal for reading information | `lumen.*` |
| **mousse** (moss) | round and light, medium, smiling but sober, warm, natural pace, crisp articulation | `mousse.*` |
| **ours** (bear) | big calm teddy bear, low and soft voice, reassuring, slow pace | `ours.*` |
| **colibri** (hummingbird) | small lively, cheerful bird, clear and light voice, medium, singsong, fast and precise pace | `colibri.*` |

- `<voice>.safetensors`: precomputed Pocket TTS voice state (`french` model, pocket-tts 3.3.0). This is the file to use.
  13 to 16 MB for galet, lumen and mousse (22–26 s clips), 3 to 4 MB for ours and colibri (5–7 s clips).
- `<voice>_source.wav`: the clip used for cloning (Qwen3-TTS VoiceDesign output, mono 24 kHz, peak −1 dBFS). Lets you recompute the state if the Pocket TTS weights change.
- `<voice>_sample.wav`: Pocket TTS rendering of the test sentence « Coucou ! J'ai regardé ta journée : trois rendez-vous, et un peu de temps pour marcher cet après-midi. » ("Hi there! I looked at your day: three meetings, and a little time for a walk this afternoon.")
- `voices.json`: descriptions, design prompts, seeds and measurements.

## Usage

```bash
pip install pocket-tts
pocket-tts generate --language french --voice ./mousse.safetensors --text "Bonjour, prêt pour la séance ?" --output bonjour.wav
```

```python
from pocket_tts import TTSModel
model = TTSModel.load_model(language="french")
voice = model.get_state_for_audio_prompt("./mousse.safetensors")   # loading ≈ 1 ms
for chunk in model.generate_audio_stream(voice, "Bonjour, prêt pour la séance ?"):
    ...  # float PCM tensor, model.sample_rate = 24000 Hz, one chunk ≈ 80 ms
```

A `.safetensors` state loads **without** the cloning model (Hugging Face repository `kyutai/pocket-tts`, subject to accepting
the terms): the public model `kyutai/pocket-tts-without-voice-cloning` is enough, as for the voices in Kyutai's catalog.
Cloning is only needed to recompute a state from `<voice>_source.wav`:

```bash
pocket-tts export-voice --language french ./mousse_source.wav ./mousse.safetensors   # requires access to the cloning model
```

The states are tied to the `french` model weights they were computed with. If Kyutai releases new weights,
recompute them from the sources.

## How they were made

1. A "small mascot" description in French (see `design_instruct` in `voices.json`) → **Qwen3-TTS-12Hz-1.7B-VoiceDesign**
   (`generate_voice_design`, language French): one single take of 24 to 30 s of a "daily briefing" text per candidate,
   three seeds per concept, six concepts.
2. Take → trimmed, peak −1 dBFS → **Pocket TTS** `export-voice --language french` (`get_state_for_audio_prompt` → `export_model_state`).
3. Screening: no clipping, no stutter (speech recognition check), no voice change along the take (speaker embedding at start vs end);
   then listening and manual choice among the 18 candidates, with the same five replies rendered by each one.

Ours and Colibri come from the first series (a short 5–8 s clip of the test sentence, same model, same cloning).

## Licenses

- Voices (these files): synthetic, no real person imitated. The `.safetensors` states derive from the Pocket TTS weights
  (**CC-BY-4.0**, Kyutai): the attribution "Voices computed with Kyutai Pocket TTS" is required. The source clips come from
  Qwen3-TTS (**Apache-2.0**).
- Pocket TTS code: MIT. Kyutai's terms of use: no voice imitation without consent, no deceptive content.
