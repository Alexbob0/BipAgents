# Voix des Bips

Voix de mascotte synthétiques pour les agents BipAgents, rangées par langue. Chaque langue a son propre
`README.md` (voix, usage, fabrication) et un `voices.json` (descriptions et mesures).

| Langue | Moteur | Voix |
|---|---|---|
| [`french/`](french/) | Kyutai Pocket TTS 3.3.0, modèle `french` | loutre, chat2, lutin, ours, colibri |

Une nouvelle langue = un nouveau dossier (`english/`, `spanish/`…) avec la même structure : `<voix>.safetensors`
(état Pocket TTS), `<voix>_source.wav` (extrait de clonage), `<voix>_sample.wav` (phrase de test), `voices.json`, `README.md`.

Voix calculées avec Kyutai Pocket TTS (poids CC-BY-4.0, Kyutai) à partir d'extraits Qwen3-TTS VoiceDesign (Apache-2.0).
Entièrement synthétiques : aucune personne réelle n'est imitée.
