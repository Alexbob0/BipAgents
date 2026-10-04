# BipAgents

App iOS native, orientée voix, pour parler à ses agents [Hermes](https://github.com/NousResearch/hermes-agent) auto-hébergés, à travers Tailscale : texte, fichiers, dictée, appel mains libres avec interruption à la voix, approbations depuis l'écran verrouillé, messages proactifs en push.

- `SPEC.md` — la spécification complète (app iOS, côté serveur, contrat d'API, jalons).
- `context/` — faits machine et docs Hermes de la version installée : **local uniquement**, non publié (ignoré par git).

Aucun secret dans ce dépôt : les clés (api_server, bridge, APNs) sont saisies dans l'app (Trousseau) ou dans la config du bridge sur le serveur.

---

## BipAgents — app iOS (dépôt)

- `BipAgents.xcodeproj` — app SwiftUI (iOS 18+, Swift 6) + extension `NotificationService`. Les dossiers `App/` et `NotificationService/` sont synchronisés : tout fichier ajouté y est compilé. `scripts/genproj.py` régénère le projet si on ajoute une cible.
  - `App/Design/` — thème clair (sombre via le système), catégories d'agents, mascottes vectorielles (`MascotView`, mêmes tracés que la maquette).
  - `App/Model/` — `AgentStore` (agents en JSON dans l'App Group, clés dans le Trousseau), `BridgeClient`, `Router`.
  - `App/Features/` — Agents, Sessions, Conversation (streaming, outils, accords, fichiers, caméra, scan PDF, dictée maintenue), Appel vocal, Boîte (messages proactifs + audio), Réglages, Diagnostics, notifications (Approuver/Refuser depuis l'écran verrouillé).
  - `NotificationService/` — récupère le texte complet et l'audio d'un message proactif sur le bridge.
- `Packages/HermesKit` — client de l'api_server Hermes (SSE, sessions, runs, accords, pièces jointes), testable sans UI.
- `Packages/VoiceKit` — voix : capture, transcription sur l'iPhone, TTS Kyutai via le bridge (repli voix iOS), barge-in.
- `bridge/` — service Python pour aibox : TTS par phrases, fichiers, push APNs, boîte (ntfy), accords en arrière-plan.
- `docs/aibox-fichiers.md` — envoi de PDF/Excel/texte via le bridge (`POST /v1/files`).

Avant de lancer sur un iPhone : choisir son équipe de signature dans Xcode (cibles BipAgents et NotificationService). Les capacités Push, App Group `group.io.github.bipagents` et Keychain Sharing sont déclarées dans `Config/*.entitlements`.

Lancer sans serveur, avec des données d'exemple : schéma BipAgents → Arguments → `-demo` (ajouter `-screen conversation` ou `-screen call`).
Maquette : https://claude.ai/artifact/Fn4zTdfuaPpFXnyatkne8c
