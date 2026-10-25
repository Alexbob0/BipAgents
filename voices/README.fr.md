[🇬🇧 English](README.md) · 🇫🇷 Français

# Voix des Bips

Voix de mascotte synthétiques pour les agents BipAgents, rangées par langue. Chaque langue a son propre
`README.md` (voix, usage, fabrication) et un `voices.json` (descriptions et mesures).

| Langue | Moteur | Voix |
|---|---|---|
| [`french/`](french/) | Kyutai Pocket TTS 4.0.1, modèle `french` | galet, lumen, mousse, ours, colibri |
| [`english/`](english/), [`german/`](german/), [`spanish/`](spanish/) | Kyutai Pocket TTS 4.0.1, modèles `english`, `german`, `spanish` | les mêmes cinq Bips, conçus dans chaque langue (15/10/2026) |

Une nouvelle langue = un nouveau dossier (`english/`, `spanish/`…) avec la même structure : `<voix>.safetensors`
(état Pocket TTS), `<voix>_source.wav` (extrait de clonage), `<voix>_sample.wav` (phrase de test), `voices.json`, `README.md`.

Voix calculées avec Kyutai Pocket TTS (poids CC-BY-4.0, Kyutai) à partir d'extraits Qwen3-TTS VoiceDesign (Apache-2.0).
Entièrement synthétiques : aucune personne réelle n'est imitée.

## Pocket 4.0.1 (25/10/2026)

Pocket 4.0.1 a réentraîné les modèles français, allemand et espagnol (et l'anglais) et prononce juste les marques et les
mots anglais à partir du texte brut : les Bips y sont passés (test d'écoute du 25/10/2026). Un état de voix ne va qu'avec
les poids qui l'ont calculé : chaque `<voix>.safetensors` a été recalculé à partir du `<voix>_source.wav` de sa propre
langue (jamais l'extrait d'une autre langue : cela garde un accent) avec le modèle de clonage de Kyutai
`kyutai/pocket-tts` (CC-BY-4.0, accès sur acceptation des conditions, connexion Hugging Face),
`get_state_for_audio_prompt(..., truncate=True)` puis `export_model_state`, et `<voix>_sample.wav` refait avec 4.0.1.
Les états 3.3.0 restent dans l'historique git.

Le Pocket d'un serveur et ces états avancent ensemble : `bipagents update` (dans le compte qui fait tourner Pocket)
installe le pocket-tts que fixe `pocket/requirements.txt` et le redémarre sur les voix du même commit ; le bridge cesse
alors de réécrire les noms (`[pocket] engine = "4.0.1"`, installer/bipinstall/migrations.py, `pocket-engine`).

## Autres langues

Les Bips gardent leur nom dans chaque langue : le bridge envoie `pocket:<code>/<voix>` (`en`, `es`, `de`) et le
serveur Pocket charge `voices/<english|spanish|german>/<voix>.safetensors` avec le modèle de la langue. Même recette
qu'en français : description écrite dans la langue (accent de locuteur natif) → Qwen3-TTS VoiceDesign dans la langue → extrait →
clonage Pocket ; trois prises par Bip, choisies à l'écoute le 15/10/2026 (`voices.json` de chaque dossier). En allemand et en
espagnol, la référence clonée est la prise coupée sur une pause avant 10 s : avec ~23 s, ces modèles Pocket coupent la
lecture. Phrases de test :

- en : « Hi there! I looked at your day: three meetings, and a little time for a walk this afternoon. »
- es : « ¡Holi! He mirado tu día: tres citas y un rato para caminar esta tarde. »
- de : « Huhu! Ich habe mir deinen Tag angesehen: drei Termine und ein bisschen Zeit für einen Spaziergang heute Nachmittag. »

Les aperçus des apps (`voice-<voix>-<code>.m4a`, dans le code des apps) sont convertis depuis `<voix>_sample.wav`.
