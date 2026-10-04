# Voix de mascotte BipAgents — Kyutai Pocket TTS (français)

Cinq voix synthétiques, conçues le 4 octobre 2026 pour les agents de l'app BipAgents, utilisables avec
[Kyutai Pocket TTS](https://github.com/kyutai-labs/pocket-tts) sur CPU (premier son ≈ 60–110 ms, ≈ 4× temps réel sur un seul cœur).

| Voix | Caractère | Fichiers |
|---|---|---|
| **loutre** | joueuse et complice, voix médium-grave, chaude et ronde, débit vif et enjoué | `loutre.safetensors`, `loutre_source.wav`, `loutre_sample.wav` |
| **chat2** | chat malicieux et un peu paresseux, voix médium-grave, ronronnante et amusée, débit vif | `chat2.*` |
| **lutin** | lutin farceur et vif, voix médium, rieuse et expressive, débit rapide mais articulé | `lutin.*` |
| **ours** | grand ours en peluche calme, voix grave et douce, rassurante, débit lent | `ours.*` |
| **colibri** | petit oiseau vif et enjoué, voix claire et légère, médium, chantante, débit rapide et précis | `colibri.*` |

- `<voix>.safetensors` : état de voix Pocket TTS pré-calculé (modèle `french`, pocket-tts 3.3.0), 3 à 4 Mo. C'est le fichier à utiliser.
- `<voix>_source.wav` : l'extrait de 5 à 8 s qui a servi au clonage (sortie Qwen3-TTS VoiceDesign, 24 kHz). Permet de recalculer l'état si les poids Pocket TTS changent.
- `<voix>_sample.wav` : rendu Pocket TTS de la phrase de test « Coucou ! J'ai regardé ta journée : trois rendez-vous, et un peu de temps pour marcher cet après-midi. »
- `voices.json` : descriptions et mesures.

## Utilisation

```bash
pip install pocket-tts
pocket-tts generate --language french --voice ./loutre.safetensors --text "Bonjour, prêt pour la séance ?" --output bonjour.wav
```

```python
from pocket_tts import TTSModel
model = TTSModel.load_model(language="french")
voice = model.get_state_for_audio_prompt("./loutre.safetensors")   # chargement ≈ 1 ms
for chunk in model.generate_audio_stream(voice, "Bonjour, prêt pour la séance ?"):
    ...  # tenseur PCM float, model.sample_rate = 24000 Hz, un chunk ≈ 80 ms
```

Un état `.safetensors` se charge **sans** le modèle de clonage (dépôt Hugging Face `kyutai/pocket-tts`, soumis à acceptation
des conditions) : le modèle public `kyutai/pocket-tts-without-voice-cloning` suffit, comme pour les voix du catalogue Kyutai.
Le clonage n'est nécessaire que pour recalculer un état depuis `<voix>_source.wav` :

```bash
pocket-tts export-voice --language french ./loutre_source.wav ./loutre.safetensors   # nécessite l'accès au modèle de clonage
```

Les états sont liés aux poids du modèle `french` avec lesquels ils ont été calculés. Si Kyutai publie de nouveaux poids,
recalculer depuis les sources.

## Comment elles ont été faites

1. Description en français → **Qwen3-TTS-12Hz-1.7B-VoiceDesign** (`generate_voice_design`, language French) : un extrait de la phrase de test par voix.
2. Extrait → **Pocket TTS** `get_state_for_audio_prompt` → `export_model_state`.
3. Écoute et sélection manuelle sur une quinzaine de candidates.

## Licences

- Voix (ces fichiers) : synthétiques, aucune personne réelle imitée. Les états `.safetensors` dérivent des poids Pocket TTS
  (**CC-BY-4.0**, Kyutai) : mention « Voix calculées avec Kyutai Pocket TTS » requise. Les extraits source viennent de
  Qwen3-TTS (**Apache-2.0**).
- Code Pocket TTS : MIT. Conditions d'usage de Kyutai : pas d'imitation de voix sans consentement, pas de contenu trompeur.
