[🇬🇧 English](SPEC.md) · 🇫🇷 Français

# SPEC — App iOS native « console vocale » pour les agents Hermes

Version 1 — 2026-10-04. Rédigée sur le serveur à partir de l'état réel de la machine (voir `context/`, local).
Destinataire : Claude Code sur le MacBook (Xcode). Les parties « côté serveur » (§B) seront déployées sur le serveur
par le Claude Code qui y tourne, ou via SSH (§D) ; le Mac en écrit le code et le contrat.

## 0. Résumé exécutif

Objectif : remplacer SimpleX comme canal iPhone des agents Hermes par une **app iOS native Swift/SwiftUI**,
orientée bots, avec une **voix sans latence perceptible** (streaming dans les deux sens, barge-in), des
**approbations HITL** par boutons, et des **messages proactifs** (check-ins cron) reçus en push.

Principes non négociables :
1. **Natif** (Swift 6, SwiftUI, Xcode courant, cible iOS 18+). Pas de React Native, pas d'Expo, pas de WebView pour le chat.
2. **Transport Tailscale uniquement.** Rien d'exposé sur Internet. HTTPS via `tailscale serve` avec certificat réel
   (`server.example.ts.net`), donc App Transport Security sans exception. Le push passe par APNs (Apple), le contenu est récupéré sur le tailnet.
3. **Streaming partout** : WebSocket persistant, audio en trames, TTS joué dès la première phrase, interruption instantanée.
4. **Hermes reste la source de vérité** : sessions, mémoire, outils, approbations vivent dans Hermes via son
   `api_server` (OpenAI-compatible + routes natives). L'app ne réimplémente aucune logique d'agent.
5. **Open source dès le départ** : aucun secret, aucune donnée personnelle dans le dépôt. Configuration = liste d'agents (URL + clé) saisie dans l'app, stockée dans le Trousseau.

Composants :
- **App iOS** (Mac/Xcode) — §A.
- **Côté serveur** (déjà en place ou à installer) — §B : api_server Hermes ×2 profils, `tailscale serve`, serveur **ntfy** auto-hébergé,
  et un petit service **bridge** (Python, FastAPI) pour la voix (TTS streaming par phrases, STT serveur optionnel) et le push APNs.
- **Contrat d'API** entre les deux — §C.

## A. App iOS

### A1. Stack et structure
- Swift 6, SwiftUI, concurrence structurée (`async/await`, `AsyncStream`), Observation (`@Observable`).
- Réseau : `URLSession` (HTTP + SSE via `bytes(for:)`), `URLSessionWebSocketTask` pour le bridge vocal. Pas de dépendance réseau tierce.
- Audio : `AVAudioEngine` (capture + lecture), `AVAudioSession` catégorie `.playAndRecord`, mode `.voiceChat`, options `.allowBluetooth`, `.defaultToSpeaker`.
- STT v1 : **sur l'iPhone**. Priorité au framework Speech moderne (`SpeechAnalyzer` / `SpeechTranscriber`, iOS 26+, on-device, streaming, fr-FR) avec repli `SFSpeechRecognizer` (`requiresOnDeviceRecognition = true`) sur iOS 18/19. Option v2 : STT serveur via le bridge (§B3).
- Push : APNs (token-based auth), `UNUserNotificationCenter`, **Notification Service Extension** (récupère le contenu complet sur le tailnet) et `UNNotificationCategory` avec actions « Approuver / Refuser » pour les approbations.
- Persistance locale : SwiftData (cache des sessions/messages, liste d'agents). Secrets dans le Trousseau (`kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`).
- Cibles : iPhone d'abord ; iPad et Mac Catalyst non prioritaires mais ne rien faire qui les exclue.
- Structure du dépôt : `App/` (SwiftUI), `Packages/HermesKit` (client API Hermes, pur Swift, testable sans UI), `Packages/VoiceKit` (capture, lecture, barge-in, client WebSocket du bridge), `NotificationService/` (extension), `Tests/`.

### A2. Modèle de données (côté app)
- `Agent` : `id`, `name` (ex. Wellness, Vie), `baseURL` (`https://server.example.ts.net:8642`), `apiKey` (Trousseau), `voice` (alias Kyutai, ex. `5476`), `color`, `defaultSessionID?`.
- `Session` : `id` (id Hermes), `agentID`, `title`, `updatedAt`, `lastMessagePreview`. Miroir de `GET /api/sessions`.
- `Message` : `id`, `role` (user/assistant/tool/system/notice), `text`, `createdAt`, `attachments` (images), `toolEvents` ([`ToolEvent`]), `reasoning?` (repliable), `audioState` (none/synthesizing/playing/played).
- `ToolEvent` : `tool`, `preview`, `status` (started/completed/failed), `duration?`.
- `ApprovalRequest` : `runID`, `requestID?`, `command` (déjà expurgée côté serveur), `choices` ⊆ {once, session, always, deny}, `agentID`, `sessionID`.
- `Run` : `runID`, `status` (running/waiting_for_approval/completed/failed/cancelled/interrupted/stopping).

### A3. Écrans
1. **Agents** (racine) : un onglet ou une liste par agent configuré ; pastille d'état (joignable / hors tailnet / clé invalide) obtenue par `GET /health` + `GET /v1/capabilities`.
2. **Sessions** d'un agent : liste paginée (`GET /api/sessions?limit=&offset=`), création (`POST /api/sessions`), renommage (`PATCH`), suppression, « nouvelle conversation ».
3. **Conversation** : fil de messages en streaming ; cartes d'outils compactes (nom + aperçu, dépliables) ; bloc « réflexion » replié ; **carte d'approbation** avec boutons (Une fois / Session / Toujours / Refuser, selon `choices`) ; composeur texte + photo ; **bouton micro** (appui long = push-to-talk, tap = mode mains libres) ; bouton stop pendant une génération (`POST /v1/runs/{id}/stop`).
4. **Mode vocal plein écran** (« appel ») : visualiseur, transcription partielle en direct, réponse en texte au fil de l'eau, interruption par la voix (barge-in) ou par tap, raccrocher.
5. **Boîte de réception** : messages proactifs (cron) reçus par push, groupés par agent, avec « répondre » qui ouvre la session d'origine ou en crée une.
6. **Réglages** : agents (ajout par formulaire ou par **QR code** JSON `{name, baseURL, apiKey, voice}` affiché par le serveur), bridge (URL, clé), voix, STT (on-device / serveur), diagnostics (ping tailnet, latences mesurées : premier token, premier audio).

### A4. Flux texte (session Hermes)
- Envoi : `POST /api/sessions/{id}/chat/stream` avec `{"input": "...", "attachments"?: [...]}`, lecture SSE. Événements à gérer : `assistant.delta` (append), `assistant.commentary` (bulle grise temporaire), `tool.started` / `tool.completed` / `tool.failed` (cartes), `approval.request` (carte d'approbation, le run passe en `waiting_for_approval`), `run.completed` / `run.failed` / `run.cancelled` (clôture), lignes `: keepalive` à ignorer.
- Identifiant de run : relever `run_id` dans l'enveloppe des événements (chaque événement est un `_run_event(run_id, type, …)` ; vérifier le premier événement reçu sur l'instance réelle et noter le nom exact dans `HermesKit`).
- Approbation : `POST /v1/runs/{run_id}/approval` body `{"choice": "once|session|always|deny", "request_id"?: "…"}`. Alias acceptés : approve/approved/allow → once.
- Reprise après coupure (VPN iOS, mise en arrière-plan) : `GET /v1/runs/{run_id}` pour l'état, `GET /v1/runs/{run_id}/events` pour se réabonner, `GET /api/sessions/{id}/messages?inline_images=false` pour resynchroniser le fil. Tampons d'événements non consommés expirés après 5 min : au-delà, resynchroniser par `messages`.
- Images : `content` multimodal (`image_url` en `data:image/jpeg;base64,…`), réduire à ≤ 1600 px avant envoi.
- Multi-profils : chaque agent a sa propre base URL et sa propre clé (deux gateways distincts, pas de `/p/<profile>/`).

### A5. Flux vocal (objectif : < 1 s entre fin de parole et début d'audio hors temps modèle)
1. L'utilisateur parle. STT on-device produit des **transcriptions partielles** affichées en direct ; la détection de fin d'énoncé (silence ≈ 600–800 ms, réglable) déclenche l'envoi.
2. L'app envoie le texte final à Hermes (§A4) **et** ouvre/maintient une connexion WebSocket au bridge (`wss://server.example.ts.net:8643/v1/voice`) en lui transmettant `{agent, session_id, run_id, voice}`.
3. Le bridge s'abonne lui-même au flux d'événements du run (`/v1/runs/{id}/events`), découpe `assistant.delta` en phrases, synthétise chaque phrase avec Kyutai et renvoie à l'app des trames audio binaires (PCM int16 mono 24 kHz, ou Opus) précédées d'un en-tête JSON `{seq, sentence_index, text, final}`.
   Variante acceptable pour v1 : l'app fait elle-même le découpage et appelle `POST /v1/tts/sentence` du bridge par phrase (HTTP, plus simple), tant que la lecture démarre à la première phrase.
4. L'app lit les trames via `AVAudioPlayerNode` (schedule de buffers successifs, file d'attente). Aucune attente de la fin de génération.
5. **Barge-in** : si la capture détecte de la voix (niveau RMS + STT partiel non vide) pendant la lecture : stop immédiat de la lecture, `POST /v1/runs/{id}/stop`, message `{"type":"cancel"}` au bridge, nouvel énoncé. Nécessite l'annulation d'écho : `AVAudioSession` mode `.voiceChat` + `setVoiceProcessingEnabled(true)` sur le nœud d'entrée.
6. Mode mains libres : boucle écoute → envoi → lecture → écoute, tant que l'écran d'appel est ouvert. Indicateur visuel d'état (écoute / réflexion / parle).
7. Repli : si le bridge est injoignable, synthèse locale `AVSpeechSynthesizer` (voix fr) pour ne jamais rester muet.

### A6. Push et messages proactifs
- Au premier lancement l'app demande l'autorisation notifications, obtient le device token APNs et l'enregistre auprès du bridge : `POST /v1/devices {token, agent_ids, environment: sandbox|production}` (clé bridge en Bearer).
- Le bridge envoie : (a) **alerte** « nouveau message proactif » avec `thread-id` = agent, `mutable-content: 1` et payload `{outbox_id, agent, session_id?}` ; (b) **alerte** « approbation requise » catégorie `APPROVAL` (actions Approuver/Refuser) avec `{run_id, request_id, agent}` ; (c) **silencieuse** (`content-available`) pour resynchroniser.
- La **Notification Service Extension** récupère le texte complet (`GET /v1/outbox/{id}`) et, si disponible, l'audio pré-synthétisé (`GET /v1/outbox/{id}/audio` → mp3 en pièce jointe) sur le tailnet. Si le tailnet n'est pas joignable (VPN coupé), afficher le titre seul. Tailscale iOS doit être en **VPN on-demand**.
- Les actions de notification « Approuver / Refuser » appellent directement `POST /v1/runs/{run_id}/approval` (via l'extension/`UNNotificationAction`, en arrière-plan).
- Boîte de réception : `GET /v1/outbox?since=` au retour au premier plan.

### A7. Arrière-plan, robustesse, qualité
- Mode audio en arrière-plan (`UIBackgroundModes: audio`) pour continuer à lire une réponse écran éteint ; **CallKit** optionnel en v2 pour le mode appel depuis l'écran verrouillé.
- Reconnexion WebSocket/SSE avec backoff (0,5 s → 8 s), reprise par `run_id`.
- Mesures affichées dans Diagnostics : temps premier token, premier audio, durée STT ; journal exportable.
- Tests : `HermesKit` testé contre un **serveur SSE simulé** (fixtures des événements ci-dessus) ; `VoiceKit` testé sur le découpage en phrases et la file audio.
- Accessibilité : Dynamic Type, VoiceOver sur les cartes, haptique à la fin d'énoncé.
- Localisation : fr d'abord, chaînes externalisées.

### A8. Hors périmètre v1
Chiffrement bout en bout applicatif (Tailscale suffit), multi-utilisateurs, watchOS, CarPlay, édition de jobs cron depuis l'app (possible plus tard via `/api/jobs`), gestion de fichiers autres qu'images (l'api_server ne les accepte pas).

## B. Côté serveur

### B1. Activer l'api_server Hermes (un par profil)
- Dans `~/hermes-agent/hermes-home/profiles/wellness/.env` : `API_SERVER_ENABLED=true`, `API_SERVER_HOST=0.0.0.0` (indispensable : le conteneur est en réseau pasta, 127.0.0.1 ne serait pas publié), `API_SERVER_PORT=8642`, `API_SERVER_KEY=<clé longue aléatoire, ≥ 32 car.>`. Idem pour `vie` avec `API_SERVER_PORT=8644` et une **clé distincte**.
- Dans `gateway-run.sh` : ajouter `-p 127.0.0.1:8642:8642` (wellness) et `-p 127.0.0.1:8644:8644` (vie) aux `EXTRA_PORTS`. Bind sur 127.0.0.1 seulement : l'exposition se fait par `tailscale serve`.
- Redémarrer les unités (`systemctl --user restart hermes-gateway hermes-gateway-vie`) **hors tour en cours** (voir journal ; un SIGKILL en plein tour laisse un bail de session de 5 min).
- Vérifier : `curl -H "Authorization: Bearer $KEY" http://127.0.0.1:8642/v1/capabilities` doit annoncer `run_submission`, `run_events_sse`, `run_approval`, `session_*`.
- Option : `gateway.api_server.tool_progress_events` reste `true` ; `direct_model_requests` reste `false`.
- Jeu de toolsets pour la plateforme `api_server` : ajouter une entrée `platform_toolsets.api_server` alignée sur celle de `simplex` dans chaque profil (sinon le défaut s'applique).

### B2. Exposition HTTPS sur le tailnet
- `sudo tailscale serve --bg --https=8642 http://127.0.0.1:8642` ; idem `8644 → 8644`, `8643 → 8643` (bridge), `8645 → 8645` (ntfy). Les ports HTTPS doivent être acceptés par cette version de `tailscale serve` ; sinon repli sur 443/8443/10000 avec `--set-path` par service, ou un Caddy local avec `tailscale cert`.
- Résultat : `https://server.example.ts.net:8642/v1/...` joignable uniquement depuis le tailnet, certificat valide, aucune règle firewalld.
- Tester depuis le Mac : `curl https://server.example.ts.net:8642/health`.

### B3. Bridge vocal + push (nouveau service, port 8643, Python 3.12 + FastAPI + uvicorn, conteneur podman rootless ou venv + unité systemd user)
Responsabilités :
1. **TTS streaming par phrases** : `POST /v1/tts/sentence {text, voice}` → audio d'une phrase (Kyutai `/v1/audio/speech`, `response_format: wav`, 24 kHz) renvoyé en PCM int16 ou Opus ; et `WS /v1/voice` qui suit un run Hermes (`GET /v1/runs/{id}/events` avec la clé de l'agent, stockée côté bridge par agent) et pousse les phrases synthétisées à mesure. Découpage : fin de phrase (`.!?…`) ou 180 caractères ; normalisation (markdown retiré, listes lues naturellement). Pré-chauffage : phrase 1 envoyée à Kyutai dès qu'elle est complète ; les suivantes en file (Kyutai est sérialisé par un verrou : une requête à la fois, donc file FIFO, pas de parallélisme).
2. **STT serveur (optionnel, v2)** : `WS /v1/stt` recevant des trames PCM 16 kHz, transcription incrémentale (faster-whisper `large-v3-turbo` CPU 32 threads en premier ; Kyutai STT `stt-1b-en_fr` en streaming si un runtime ROCm/CPU fonctionne). Hors chemin critique de la v1.
3. **Push** : `POST /v1/devices` (enregistrement APNs), **abonnement au topic ntfy** de chaque agent (`GET http://127.0.0.1:8645/<topic>/json`, flux long), stockage dans une **outbox** SQLite (`id, agent, text, created_at, session_id?, audio_path?`), pré-synthèse audio du message (Kyutai, mp3), envoi APNs HTTP/2 (token `.p8`, Team ID, Key ID, bundle id ; librairie `aioapns` ou `httpx` + JWT). Routes `GET /v1/outbox?since=`, `GET /v1/outbox/{id}`, `GET /v1/outbox/{id}/audio`.
4. **Approbations en arrière-plan** : le bridge garde un suivi des runs qu'il observe ; sur `approval.request` sans client WebSocket connecté, il envoie le push catégorie `APPROVAL`.
5. Auth : Bearer unique « clé bridge » (différente des clés Hermes). Journal structuré. `GET /health`.
- Secrets du bridge (clés Hermes, `.p8` APNs) dans `~/.config/hermes-ios/` (mode 600), jamais dans le dépôt.

### B4. ntfy auto-hébergé (port 8645) et bascule des crons
- `podman run -d --name ntfy -p 127.0.0.1:8645:80 -v ~/ntfy:/var/cache/ntfy:z docker.io/binwiederhier/ntfy serve` (+ unité systemd user, `auth-default-access: deny-all` et un jeton par agent).
- Hermes : par profil, dans `.env` : `NTFY_SERVER_URL=http://host.containers.internal:8645`, `NTFY_TOPIC=<topic-in-inutilisé>`, `NTFY_PUBLISH_TOPIC=hermes-wellness-out` (resp. `hermes-vie-out`), `NTFY_HOME_CHANNEL=hermes-wellness-out`, `NTFY_TOKEN=…`, `NTFY_ALLOWED_USERS=<topic-in>`.
- Crons wellness « Check-in sommeil du matin » (08:15) et « Plan sommeil du soir » (20:00) : `deliver: simplex` → `deliver: simplex,ntfy` pendant la transition, puis `ntfy` seul. Les jobs techniques restent en `local`.
- Le texte délivré par Hermes est expurgé des secrets côté Hermes avant envoi.

### B5. Ordre de mise en service côté serveur
1. B1 + B2 (api_server + HTTPS) → l'app texte (§A4) devient testable.
2. B3 partie TTS → mode vocal.
3. B4 + B3 partie push → messages proactifs, puis extinction de SimpleX et Telegram pour ces agents.

## C. Contrat d'API (ce que l'app attend)

### C1. Hermes api_server (par agent, Bearer = clé de l'agent) — doc complète : `context/hermes-docs/api-server.md`, table des routes : `context/hermes-api-routes.txt`
- `GET /health`, `GET /v1/capabilities`, `GET /v1/models`
- Sessions : `GET/POST /api/sessions`, `GET/PATCH/DELETE /api/sessions/{id}`, `GET /api/sessions/{id}/messages?include_compacted=&inline_images=`, `POST /api/sessions/{id}/fork`, `POST /api/sessions/{id}/chat`, `POST /api/sessions/{id}/chat/stream` (SSE)
- Runs : `POST /v1/runs`, `GET /v1/runs/{id}`, `GET /v1/runs/{id}/events` (SSE), `POST /v1/runs/{id}/approval`, `POST /v1/runs/{id}/steer`, `POST /v1/runs/{id}/stop`
- Compat : `POST /v1/chat/completions` (SSE `chat.completion.chunk` + `event: hermes.tool.progress`, en-têtes `X-Hermes-Session-Id`, `X-Hermes-Session-Key`), `POST /v1/responses`
- Jobs cron : `GET/POST /api/jobs`, `GET/PATCH/DELETE /api/jobs/{id}`, `POST /api/jobs/{id}/{pause|resume|run}` (v2 de l'app)
- Événement d'approbation : voir `context/hermes-approval-and-events.txt` (`choices`, `command` expurgée, `request_id`).
- Limites : 10 runs concurrents (429 au-delà), tampons SSE non lus expirés après 5 min, pas d'upload de fichiers non-image.

### C2. Bridge (Bearer = clé bridge), base `https://server.example.ts.net:8643`
- `GET /health`
- `POST /v1/tts/sentence` `{text, voice, format: "pcm16"|"opus"}` → audio binaire, en-têtes `X-Sample-Rate: 24000`, `X-Channels: 1`
- `WS /v1/voice` : client → `{"type":"follow","agent":"wellness","run_id":"…","voice":"5476"}` ; serveur → trames : message texte JSON `{"type":"sentence","seq":n,"text":"…"}` suivi d'un message binaire audio ; `{"type":"done"}` ; client → `{"type":"cancel"}`.
- `WS /v1/stt` (v2) : client → binaire PCM16 16 kHz ; serveur → `{"type":"partial"|"final","text":"…"}`.
- `POST /v1/devices` `{token, environment, agent_ids}` ; `DELETE /v1/devices/{token}`
- `GET /v1/outbox?since=<iso>`, `GET /v1/outbox/{id}`, `GET /v1/outbox/{id}/audio` (audio/mpeg)
- Payloads APNs : `{"aps":{"alert":{"title":"Wellness","body":"…"},"thread-id":"wellness","mutable-content":1,"category":"MESSAGE"|"APPROVAL","sound":"default"},"outbox_id":"…","agent":"wellness","run_id":"…","request_id":"…"}`

### C3. Config d'un agent (QR / JSON)
`{"name":"Wellness","baseURL":"https://server.example.ts.net:8642","apiKey":"…","voice":"5476","bridgeURL":"https://server.example.ts.net:8643","bridgeKey":"…"}`

## D. Accès de Claude Code (Mac) aux configs Hermes sur le serveur

Le dossier `context/` (local, non publié dans le dépôt) contient déjà tout ce qui est nécessaire pour écrire l'app et le bridge sans toucher au serveur :
faits machine, docs Hermes de la version installée (api-server, cron, ntfy, open-webui, intégration programmatique),
table des routes, contrat d'approbation, source du serveur Kyutai, unités systemd.

Si un accès **en direct** est nécessaire (tester les endpoints, lire un fichier précis) :
1. **Partage de fichiers** local (SMB ou autre) entre le serveur et le Mac pour déposer des copies à lire.
2. **SSH sur le tailnet** : par clé autorisée (`ssh-copy-id` depuis le Mac) ou Tailscale SSH (`sudo tailscale set --ssh`). Une seule des deux.
3. **Ce que Claude Code (Mac) peut lire sur le serveur** (lecture seule) :
   - `~/hermes-agent/hermes-home/profiles/{wellness,vie}/config.yaml` (sections `gateway`, `platforms`, `platform_toolsets`, `approvals`, `tts`, `stt`), `.../cron/jobs.json`
   - le code Hermes installé : `podman exec hermes-gateway cat /opt/hermes-seed/hermes-agent/<chemin>` (notamment `gateway/platforms/api_server*.py`, `website/docs/...`)
   - `~/tts-lab/server/kyutai_server.py`, `systemctl --user cat <unité>`, `podman ps`, `ss -ltn`, `tailscale status`
4. **Interdits depuis le Mac** : lire ou copier `gateway-run.sh`, tout `.env`, `~/.config/hermes-secrets/`, `auth.json` (secrets en clair) ; redémarrer, arrêter ou modifier une unité systemd ou un conteneur ; modifier un fichier sous `~/hermes-agent/` ; toucher aux services des autres comptes de la machine. Les changements côté serveur (§B) passent par le propriétaire ou par le Claude Code du serveur, qui connaît les pièges (restart en plein tour, `hermes update` qui écrase les patchs).
5. **Tests d'intégration** depuis le Mac une fois §B1–B2 faits : `curl -H "Authorization: Bearer $KEY" https://server.example.ts.net:8642/v1/capabilities`, puis un `chat/stream` en SSE avec `curl -N`. Les clés sont transmises hors bande (jamais écrites dans le dépôt).

## E. Jalons et critères d'acceptation
1. **Texte** : lister les sessions des deux agents, streamer une réponse avec cartes d'outils, approuver un outil depuis la carte, reprendre un run après avoir tué l'app. Cible : premier token affiché < 300 ms après l'arrivée du premier `assistant.delta`.
2. **Voix** : énoncé de 5 s → transcription partielle visible pendant la parole ; audio de la première phrase qui démarre < 1,5 s après le premier `assistant.delta` ; barge-in qui coupe la lecture < 200 ms ; aucune coupure d'audio sur une réponse de 60 s.
3. **Proactif** : un cron de test `deliver: ntfy` toutes les 5 min arrive en push en < 10 s, avec l'audio joint ; « répondre » ouvre la bonne session.
4. **Robustesse** : passage LAN → 5G sans perte de session ; VPN on-demand coupé puis rétabli → resynchronisation automatique.
5. **Open source** : dépôt sans secret, README d'installation (Tailscale, api_server, bridge), licence MIT.
