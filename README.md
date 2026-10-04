# BipAgents

App iOS native, orientée voix, pour parler à ses agents [Hermes](https://github.com/NousResearch/hermes-agent) auto-hébergés, à travers Tailscale : texte, fichiers, notes vocales, conversation « Live » mains libres avec interruption à la voix, approbations depuis l'écran verrouillé, messages proactifs en push. Chaque agent est un Bip, une mascotte avec sa propre voix.

- `SPEC.md` — la spécification complète (app iOS, côté serveur, contrat d'API, jalons).
- `context/` — faits machine et docs Hermes de la version installée : **local uniquement**, non publié (ignoré par git).

Aucun secret dans ce dépôt : les clés (api_server, bridge, APNs) sont saisies dans l'app (Trousseau) ou dans la config du bridge sur le serveur.

---

## BipAgents — app iOS (dépôt)

- `BipAgents.xcodeproj` — app SwiftUI (iOS 18+, Swift 6) + extension `NotificationService`. Les dossiers `App/` et `NotificationService/` sont synchronisés : tout fichier ajouté y est compilé. `scripts/genproj.py` régénère le projet si on ajoute une cible.
  - `App/Design/` — thème clair (sombre via le système), catégories d'agents, mascottes vectorielles (`MascotView`, mêmes tracés que la maquette).
    - `InteractiveMascot` — les Bips réagissent au toucher : tape (réaction au hasard), chatouilles, caresse, pression (« Boing ! » au relâcher), glisser ; trop embêtés, ils ont le tournis puis boudent.
    - `BipBabble` — leur « Animalese » : un babillage synthétisé sur l'iPhone (sans modèle ni réseau), coupé par le mode silencieux et jamais pendant un Live ou une note vocale ; réglage « Sons des Bips ».
    - `AgentVoices` — voix de chaque agent : une voix de Bip choisie selon la catégorie (`pocket:loutre`, `colibri`, `lutin`, `chat2`, `ours`) ou la voix classique Kyutai ; modifiable dans Réglages › agent › Voix, avec aperçu.
  - `App/Model/` — `AgentStore` (agents en JSON dans l'App Group, clés dans le Trousseau), `BridgeClient`, `Router`.
  - `App/Features/` — Agents, Sessions, Conversation (streaming, outils, accords, fichiers, caméra, scan PDF, notes vocales), Live (conversation vocale), Boîte (messages proactifs + audio, réponses manquées), Réglages, Diagnostics, notifications (Approuver/Refuser depuis l'écran verrouillé).
    - Carte d'agent : trois actions **Audio** / **Écrire** / **Live**. Maintenir « Audio » enregistre une note vocale, relâcher l'envoie dans la conversation en cours et la réponse s'affiche sur la carte (`QuickVoice`) ; un simple appui ouvre la conversation, prête pour une note vocale.
    - Les messages partent par `POST /v1/runs` de Hermes : la réponse continue sur le serveur écran verrouillé, l'app s'y rattache en rouvrant la conversation, et une réponse terminée pendant l'absence arrive dans la Boîte. Repli sur `chat/stream` pour les messages avec images ou si le serveur refuse les runs.
    - « Écouter » sous une réponse la lit pendant sa synthèse (streaming) et la garde comme note vocale.
  - `NotificationService/` — récupère le texte complet et l'audio d'un message proactif sur le bridge.
- `Packages/HermesKit` — client de l'api_server Hermes (SSE, sessions, runs, accords, pièces jointes), testable sans UI.
- `Packages/VoiceKit` — voix : capture, transcription sur l'iPhone, TTS via le bridge en flux PCM (repli voix iOS), barge-in.
- `bridge/` — service Python pour aibox : TTS par phrases et en flux (`/v1/tts/stream`), fichiers, push APNs, boîte (ntfy), accords en arrière-plan. Deux moteurs : Kyutai 1.6B (GPU) pour la voix classique et le podcast, Kyutai Pocket TTS (CPU) pour les voix `pocket:<nom>` des Bips, avec repli sur Kyutai. Avant synthèse, le texte est rendu prononçable (`bipbridge/numbers_fr.py` : nombres en lettres, symboles et abréviations, mots anglais réécrits, parenthèses lues comme des pauses). Détails : [`bridge/README.md`](bridge/README.md).
- `voices/` — voix des Bips (états Pocket TTS, `voices/french/`), cf. [`voices/README.md`](voices/README.md).
- `docs/aibox-fichiers.md` — envoi de PDF/Excel/texte via le bridge (`POST /v1/files`).

Avant de lancer sur un iPhone : choisir son équipe de signature dans Xcode (cibles BipAgents et NotificationService). Les capacités Push, App Group `group.io.github.bipagents` et Keychain Sharing sont déclarées dans `Config/*.entitlements`.

Lancer sans serveur, avec des données d'exemple : schéma BipAgents → Arguments → `-demo` (ajouter `-screen conversation` ou `-screen call`).
Maquette : https://claude.ai/artifact/Fn4zTdfuaPpFXnyatkne8c
