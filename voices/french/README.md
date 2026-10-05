🇬🇧 English · [🇫🇷 Français](README.fr.md)

# BipAgents mascot voices — Kyutai Pocket TTS (French)

Five synthetic voices, designed on October 4, 2026 for the BipAgents app's agents, usable with
[Kyutai Pocket TTS](https://github.com/kyutai-labs/pocket-tts) on CPU (first audio ≈ 60–110 ms, ≈ 4× real time on a single core).

| Voice | Character | Files |
|---|---|---|
| **loutre** (otter) | playful and friendly, medium-low voice, warm and round, lively and cheerful pace | `loutre.safetensors`, `loutre_source.wav`, `loutre_sample.wav` |
| **chat2** (cat) | mischievous, slightly lazy cat, medium-low voice, purring and amused, lively pace | `chat2.*` |
| **lutin** (elf) | prankish, lively elf, medium voice, laughing and expressive, fast but articulate pace | `lutin.*` |
| **ours** (bear) | big calm teddy bear, low and soft voice, reassuring, slow pace | `ours.*` |
| **colibri** (hummingbird) | small lively, cheerful bird, clear and light voice, medium, singsong, fast and precise pace | `colibri.*` |

- `<voice>.safetensors`: precomputed Pocket TTS voice state (`french` model, pocket-tts 3.3.0), 3 to 4 MB. This is the file to use.
- `<voice>_source.wav`: the 5 to 8 s clip used for cloning (Qwen3-TTS VoiceDesign output, 24 kHz). Lets you recompute the state if the Pocket TTS weights change.
- `<voice>_sample.wav`: Pocket TTS rendering of the test sentence « Coucou ! J'ai regardé ta journée : trois rendez-vous, et un peu de temps pour marcher cet après-midi. » ("Hi there! I looked at your day: three meetings, and a little time for a walk this afternoon.")
- `voices.json`: descriptions and measurements.

## Usage

```bash
pip install pocket-tts
pocket-tts generate --language french --voice ./loutre.safetensors --text "Bonjour, prêt pour la séance ?" --output bonjour.wav
```

```python
from pocket_tts import TTSModel
model = TTSModel.load_model(language="french")
voice = model.get_state_for_audio_prompt("./loutre.safetensors")   # loading ≈ 1 ms
for chunk in model.generate_audio_stream(voice, "Bonjour, prêt pour la séance ?"):
    ...  # float PCM tensor, model.sample_rate = 24000 Hz, one chunk ≈ 80 ms
```

A `.safetensors` state loads **without** the cloning model (Hugging Face repository `kyutai/pocket-tts`, subject to accepting
the terms): the public model `kyutai/pocket-tts-without-voice-cloning` is enough, as for the voices in Kyutai's catalog.
Cloning is only needed to recompute a state from `<voice>_source.wav`:

```bash
pocket-tts export-voice --language french ./loutre_source.wav ./loutre.safetensors   # requires access to the cloning model
```

The states are tied to the `french` model weights they were computed with. If Kyutai releases new weights,
recompute them from the sources.

## How they were made

1. Description in French → **Qwen3-TTS-12Hz-1.7B-VoiceDesign** (`generate_voice_design`, language French): one clip of the test sentence per voice.
2. Clip → **Pocket TTS** `get_state_for_audio_prompt` → `export_model_state`.
3. Listening and manual selection among about fifteen candidates.

## Licenses

- Voices (these files): synthetic, no real person imitated. The `.safetensors` states derive from the Pocket TTS weights
  (**CC-BY-4.0**, Kyutai): the attribution "Voices computed with Kyutai Pocket TTS" is required. The source clips come from
  Qwen3-TTS (**Apache-2.0**).
- Pocket TTS code: MIT. Kyutai's terms of use: no voice imitation without consent, no deceptive content.
