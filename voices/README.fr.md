[🇬🇧 English](README.md) · 🇫🇷 Français

# Voix des Bips

Voix de mascotte synthétiques pour les agents BipAgents, rangées par langue. Chaque langue a son propre
`README.md` (voix, usage, fabrication) et un `voices.json` (descriptions et mesures).

| Langue | Moteur | Voix |
|---|---|---|
| [`french/`](french/) | Kyutai Pocket TTS 3.3.0, modèle `french` | loutre, chat2, lutin, ours, colibri |
| `english/`, `spanish/`, `german/` | Kyutai Pocket TTS 3.3.0, modèles `english`, `spanish`, `german` | les mêmes cinq Bips (à générer, cf. ci-dessous) |

Une nouvelle langue = un nouveau dossier (`english/`, `spanish/`…) avec la même structure : `<voix>.safetensors`
(état Pocket TTS), `<voix>_source.wav` (extrait de clonage), `<voix>_sample.wav` (phrase de test), `voices.json`, `README.md`.

Voix calculées avec Kyutai Pocket TTS (poids CC-BY-4.0, Kyutai) à partir d'extraits Qwen3-TTS VoiceDesign (Apache-2.0).
Entièrement synthétiques : aucune personne réelle n'est imitée.

## Autres langues

Les Bips gardent leur nom dans chaque langue : le bridge envoie `pocket:<code>/<voix>` (`en`, `es`, `de`) et le
serveur Pocket charge `voices/<english|spanish|german>/<voix>.safetensors` avec le modèle de la langue. Même recette
qu'en français : description → Qwen3-TTS VoiceDesign dans la langue → extrait → clonage Pocket. Phrases de test :

- en : « Hi there! I looked at your day: three meetings, and a little time for a walk this afternoon. »
- es : « ¡Holi! He mirado tu día: tres citas y un rato para caminar esta tarde. »
- de : « Huhu! Ich habe mir deinen Tag angesehen: drei Termine und ein bisschen Zeit für einen Spaziergang heute Nachmittag. »

Les aperçus de l'app sont `App/Resources/Voices/voice-<voix>-<code>.m4a` (convertis depuis `<voix>_sample.wav`).
