🇬🇧 English · [🇫🇷 Français](README.fr.md)

# Bips' voices

Synthetic mascot voices for the BipAgents agents, organized by language. Each language has its own
`README.md` (voices, usage, how they were made) and a `voices.json` (descriptions and measurements).

| Language | Engine | Voices |
|---|---|---|
| [`french/`](french/) | Kyutai Pocket TTS 3.3.0, `french` model | loutre, chat2, lutin, ours, colibri |
| `english/`, `spanish/`, `german/` | Kyutai Pocket TTS 3.3.0, `english`, `spanish`, `german` models | the same five Bips (to be generated, see below) |

A new language = a new folder (`english/`, `spanish/`…) with the same structure: `<voice>.safetensors`
(Pocket TTS state), `<voice>_source.wav` (cloning clip), `<voice>_sample.wav` (test sentence), `voices.json`, `README.md`.

Voices computed with Kyutai Pocket TTS (CC-BY-4.0 weights, Kyutai) from Qwen3-TTS VoiceDesign clips (Apache-2.0).
Fully synthetic: no real person is imitated.

## Other languages

The Bips keep their names in every language: the bridge sends `pocket:<code>/<voice>` (`en`, `es`, `de`) and the
Pocket server loads `voices/<english|spanish|german>/<voice>.safetensors` with that language's model. Same recipe
as in French: description → Qwen3-TTS VoiceDesign in the language → clip → Pocket cloning. Test sentences:

- en: "Hi there! I looked at your day: three meetings, and a little time for a walk this afternoon."
- es: "¡Holi! He mirado tu día: tres citas y un rato para caminar esta tarde."
- de: "Huhu! Ich habe mir deinen Tag angesehen: drei Termine und ein bisschen Zeit für einen Spaziergang heute Nachmittag."

The app's previews are `App/Resources/Voices/voice-<voice>-<code>.m4a` (converted from `<voice>_sample.wav`).
