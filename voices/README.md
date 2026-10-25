🇬🇧 English · [🇫🇷 Français](README.fr.md)

# Bips' voices

Synthetic mascot voices for the BipAgents agents, organized by language. Each language has its own
`README.md` (voices, usage, how they were made) and a `voices.json` (descriptions and measurements).

| Language | Engine | Voices |
|---|---|---|
| [`french/`](french/) | Kyutai Pocket TTS 4.0.1, `french` model | galet, lumen, mousse, ours, colibri |
| [`english/`](english/), [`german/`](german/), [`spanish/`](spanish/) | Kyutai Pocket TTS 4.0.1, `english`, `german`, `spanish` models | the same five Bips, designed in each language (2026-10-15) |

A new language = a new folder (`english/`, `spanish/`…) with the same structure: `<voice>.safetensors`
(Pocket TTS state), `<voice>_source.wav` (cloning clip), `<voice>_sample.wav` (test sentence), `voices.json`, `README.md`.

Voices computed with Kyutai Pocket TTS (CC-BY-4.0 weights, Kyutai) from Qwen3-TTS VoiceDesign clips (Apache-2.0).
Fully synthetic: no real person is imitated.

## Pocket 4.0.1 (2026-10-25)

Pocket 4.0.1 retrained the French, German and Spanish models (and the English one) and says brands and English words
right from the raw text, so the Bips moved to it (a listening test, 2026-10-25). A voice state only fits the weights it
was computed with: each `<voice>.safetensors` was recomputed from its own language's `<voice>_source.wav` (never
another language's clip: that keeps an accent) with Kyutai's cloning model `kyutai/pocket-tts` (CC-BY-4.0, gated:
accept its terms, Hugging Face login), `get_state_for_audio_prompt(..., truncate=True)` then `export_model_state`, and
`<voice>_sample.wav` spoken again with 4.0.1. The 3.3.0 states are in git history.

A server's Pocket and these states move together: `bipagents update` (in the account that runs Pocket) installs the
pocket-tts that `pocket/requirements.txt` pins and restarts it on the voices of the same commit; the bridge then stops
respelling names (`[pocket] engine = "4.0.1"`, installer/bipinstall/migrations.py, `pocket-engine`).

## Other languages

The Bips keep their names in every language: the bridge sends `pocket:<code>/<voice>` (`en`, `es`, `de`) and the
Pocket server loads `voices/<english|spanish|german>/<voice>.safetensors` with that language's model. Same recipe
as in French: description written in the language (native-speaker accent) → Qwen3-TTS VoiceDesign in the language → clip →
Pocket cloning; three takes per Bip, picked by ear on 2026-10-15 (each folder's `voices.json`). In German and Spanish the
cloned reference is the take cut on a pause before 10 s: with ~23 s, those Pocket models cut the reading short. Test
sentences:

- en: "Hi there! I looked at your day: three meetings, and a little time for a walk this afternoon."
- es: "¡Holi! He mirado tu día: tres citas y un rato para caminar esta tarde."
- de: "Huhu! Ich habe mir deinen Tag angesehen: drei Termine und ein bisschen Zeit für einen Spaziergang heute Nachmittag."

The apps' previews (`voice-<voice>-<code>.m4a`, in the apps' own source) are converted from `<voice>_sample.wav`.
