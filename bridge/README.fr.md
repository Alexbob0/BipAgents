[🇬🇧 English](README.md) · 🇫🇷 Français

# Bridge BipAgents (`bipbridge`)

Petit service Python (FastAPI + uvicorn) qui tourne sur le **serveur** à côté des gateways Hermes et
sert l'app iOS BipAgents sur le tailnet (SPEC §B3, §C2, addendum `docs/file-uploads.md`) :

| Fonction | Route | Ce que fait le bridge |
|---|---|---|
| TTS par phrase | `POST /v1/tts/sentence` | normalise le texte (markdown, liens), appelle Kyutai `/v1/audio/speech`, renvoie du PCM int16 mono 24 kHz (ou Opus/WAV) ; file FIFO globale + cache LRU |
| Voix en flux | `WS /v1/voice` | suit un run Hermes (`GET /v1/runs/{id}/events`), découpe les deltas en phrases, synthétise dans l'ordre, envoie `sentence` (JSON) + audio (binaire) |
| Fichiers | `POST /v1/files` | dépose PDF/Excel/texte… dans un dossier monté dans le conteneur Hermes et renvoie le chemin **vu par l'agent** |
| Push | `POST/DELETE /v1/devices` | enregistre les device tokens APNs (SQLite), envoie les push via HTTP/2 + jeton JWT ES256 |
| Messages proactifs | `GET /v1/outbox…` | s'abonne au topic ntfy de chaque agent, stocke les messages, pré-synthétise l'audio (mp3), pousse `MESSAGE` |
| Approbations | `POST /v1/watch`, `POST /v1/approve`, `GET /v1/approvals` | surveille un run en arrière-plan, pousse `APPROVAL` si personne ne le suit, relaie la réponse à Hermes |
| Santé | `GET /health` | sans authentification : version + Kyutai/ntfy joignables |

Le bridge écoute **uniquement sur `127.0.0.1:8643`** ; l'exposition HTTPS se fait par `tailscale serve`.
Il ne contient aucun secret : tout est dans `~/.config/hermes-ios/` (mode 600).

## 1. Installation sur le serveur (venv + systemd --user, recommandé)

Prérequis : Python ≥ 3.11 (3.12 visé ; le code tourne aussi en 3.9+), Kyutai sur `:8097`, ntfy sur `:8645`,
api_server Hermes sur `:8642` (wellness) et `:8644` (vie) — SPEC §B1, §B4.

```bash
# 1. Code : copier le dossier bridge/ du dépôt vers ~/hermes-bridge (git clone, scp ou SMB ~/Public)
mkdir -p ~/hermes-bridge && cp -r <dépôt>/bridge/. ~/hermes-bridge/
cd ~/hermes-bridge

# 2. venv (python3.12 si installé, sinon python3 du système s'il est >= 3.11)
python3.12 -m venv .venv || python3 -m venv .venv
.venv/bin/pip install --upgrade pip
.venv/bin/pip install -r requirements.txt

# 3. Configuration
mkdir -p ~/.config/hermes-ios && chmod 700 ~/.config/hermes-ios
cp bridge.example.toml ~/.config/hermes-ios/bridge.toml
chmod 600 ~/.config/hermes-ios/bridge.toml
.venv/bin/python -m bipbridge genkey          # -> à mettre dans bridge_key (et à donner à l'app)
$EDITOR ~/.config/hermes-ios/bridge.toml       # clés Hermes, jetons ntfy, APNs, dossiers d'upload
.venv/bin/python -m bipbridge check            # valide la config (refuse les valeurs CHANGE-ME)

# 4. Dossiers d'upload visibles par les agents (home Hermes monté dans les conteneurs)
mkdir -p ~/hermes-agent/hermes-home/profiles/{wellness,vie}/uploads

# 5. Unité systemd utilisateur
mkdir -p ~/.config/systemd/user
cp systemd/hermes-bridge.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now hermes-bridge
journalctl --user -u hermes-bridge -f          # journaux JSON

# 6. Vérifications locales
curl -s http://127.0.0.1:8643/health
curl -s -H "Authorization: Bearer $BRIDGE_KEY" http://127.0.0.1:8643/v1/agents

# 7. Exposition sur le tailnet (HTTPS, certificat réel, rien sur Internet)
sudo tailscale serve --bg --https=8643 http://127.0.0.1:8643
# depuis le Mac ou l'iPhone :
curl https://server.example.ts.net:8643/health
```

Mise à jour : recopier `bipbridge/` (et `requirements.txt`), `pip install -r requirements.txt`,
`systemctl --user restart hermes-bridge`. Le redémarrage du bridge n'affecte pas Hermes (aucun tour en cours n'est tué) ;
une conversation vocale en cours est simplement coupée et l'app peut refaire `follow` avec `from_seq`.

### Droits sur les fichiers déposés (à vérifier une fois)

Les fichiers sont créés par l'utilisateur du bridge en mode `640` (dossiers `750`). Ils sont lisibles dans le
conteneur seulement si l'utilisateur de l'agent y est mappé sur cet utilisateur (ex. `--userns=keep-id`) :

```bash
podman exec hermes-gateway id
podman exec hermes-gateway ls -ln /home/hermes/.hermes/profiles/wellness/uploads/
# après un envoi de test depuis l'app :
podman exec hermes-gateway cat /home/hermes/.hermes/profiles/wellness/uploads/<AAAA-MM>/<fichier>
```

Si le conteneur ne peut pas lire : mettre `upload_file_mode = "644"` et `upload_dir_mode = "755"` dans `[limits]`
(ou une ACL `setfacl` pour l'UID mappé). Les fichiers de plus de 30 jours sont purgés automatiquement
(seulement ceux écrits par le bridge : `AAAA-MM/<12 hex>-<nom>`).

### ntfy : jeton de lecture pour le bridge

Le bridge **lit** les topics `hermes-wellness-out` / `hermes-vie-out` (où Hermes publie via `deliver: ntfy`) :

```bash
podman exec -it ntfy ntfy user add bridge
podman exec -it ntfy ntfy access bridge 'hermes-*-out' read-only
podman exec -it ntfy ntfy token add bridge        # -> ntfy_token de chaque agent
```

Le dernier id ntfy lu est mémorisé (SQLite) et renvoyé en `since=` à la reconnexion (backoff 0,5 → 8 s) :
aucun message perdu tant qu'il est dans le cache ntfy (12 h par défaut).

### APNs

Dans Apple Developer : *Keys* → nouvelle clé avec *Apple Push Notifications service (APNs)* → télécharger
`AuthKey_<KEYID>.p8` (une seule fois). La copier dans `~/.config/hermes-ios/` (mode 600) et renseigner `[apns]`
(`team_id`, `key_id`, `p8_path`, `bundle_id = "io.github.bipagents"`). `environment` est le défaut pour les appareils
qui n'en précisent pas : `sandbox` pour une app lancée depuis Xcode, `production` pour TestFlight/App Store
(l'app envoie le sien à l'enregistrement). Sans `[apns]` rempli, le bridge tourne sans push.

### Option : conteneur podman

```bash
podman build -t localhost/hermes-bridge:0.1.0 -f Containerfile .
podman run -d --name hermes-bridge --network=host --userns=keep-id \
  -v ~/.config/hermes-ios:$HOME/.config/hermes-ios:ro -e BRIDGE_CONFIG=$HOME/.config/hermes-ios/bridge.toml \
  -v ~/.local/share/hermes-bridge:$HOME/.local/share/hermes-bridge \
  -v ~/hermes-agent/hermes-home/profiles:$HOME/hermes-agent/hermes-home/profiles \
  -e HOME=$HOME localhost/hermes-bridge:0.1.0
```

`--network=host` est nécessaire pour joindre Kyutai/ntfy/Hermes sur 127.0.0.1 ; les dossiers sont montés **au même
chemin** que sur l'hôte pour que la config reste identique. Pas de `:Z` sur `hermes-home` (le relabel casserait
l'accès des conteneurs Hermes). Dans `bridge.toml`, `p8_path` doit alors pointer sous `/config/`.

## 2. Configuration (`~/.config/hermes-ios/bridge.toml`)

Voir `bridge.example.toml` (commenté). Chemin modifiable par `BRIDGE_CONFIG`. Un avertissement est journalisé si le
fichier est lisible par le groupe ou les autres. Chaque secret accepte aussi la forme `<nom>_file = "chemin"`.

| Clé | Défaut | Rôle |
|---|---|---|
| `bridge_key` | — (obligatoire) | clé Bearer unique de l'app (≥ 32 car., `python -m bipbridge genkey`) |
| `host`, `port` | `127.0.0.1`, `8643` | écoute locale |
| `[kyutai] enabled`, `url`, `default_voice` | `true`, `http://127.0.0.1:8097`, `5476` | TTS Kyutai 1.6B (GPU) ; `enabled = false` : Pocket seul |
| `[pocket] url`, `default_voice` | absent (désactivé), `pocket:colibri` | Kyutai Pocket TTS (`pocket/`) pour les voix des Bips ; `default_voice` sert quand Kyutai est désactivé |
| `[ntfy] url` | `http://127.0.0.1:8645` | |
| `[apns] team_id, key_id, p8_path, bundle_id, environment` | push désactivé si vide | |
| `[push] previews` | `false` | `false` : corps de notification générique, le texte transite par le tailnet |
| `[push] watch_followed_runs` | `true` | un run suivi en vocal reste surveillé (approbations) après fermeture du WebSocket |
| `[outbox] db_path, audio_dir, retention_days, audio_max_chars, audio_wait_seconds` | `~/.local/share/hermes-bridge/…`, 90 j, 2000, 8 s | |
| `[limits] upload_max_mb, upload_retention_days, upload_file_mode, sentence_max_chars, tts_max_chars, watch_max_minutes` | 50, 30, `640`, 180, 1000, 120 | |
| `[agents.<id>] display_name, hermes_url, hermes_key, ntfy_topic, ntfy_token, upload_dir_host, upload_dir_container, voice` | | `<id>` en minuscules = identifiant utilisé par l'app (`wellness`, `vie`) |

## 3. Ce que l'iPhone appelle

Base : `https://server.example.ts.net:8643`, en-tête `Authorization: Bearer <clé bridge>` partout sauf `/health`
(401 + `WWW-Authenticate: Bearer` sinon ; WebSocket refusé avec 403 / code 1008). L'identifiant d'agent est
insensible à la casse (`wellness`, `vie`).

- **Lancement / Réglages** : `GET /health` (pastille), `GET /v1/agents` (agents connus du bridge).
- **Notifications** : au démarrage, `POST /v1/devices {token, environment, agent_ids}` ; à la déconnexion
  `DELETE /v1/devices/{token}`.
- **Mode vocal** : après avoir lancé le tour Hermes (chat/stream) et relevé `run_id`, ouvrir `WS /v1/voice`, envoyer
  `follow`, jouer chaque binaire reçu après son en-tête `sentence` ; barge-in : `{"type":"cancel"}` + `POST /v1/runs/{id}/stop`
  côté Hermes. Variante simple : `POST /v1/tts/sentence` par phrase.
- **Passage en arrière-plan pendant un run** : `POST /v1/watch {agent, run_id}` → push `APPROVAL` si une approbation arrive,
  push silencieux `run_finished` à la fin.
- **Notification Service Extension** : `MESSAGE` → `GET /v1/outbox/{outbox_id}` + `GET /v1/outbox/{id}/audio` (mp3) ;
  `APPROVAL` → `GET /v1/approvals?agent=` pour afficher la commande ; actions Approuver/Refuser → `POST /v1/approve`.
- **Boîte de réception** : `GET /v1/outbox?since=<created_at du dernier élément>` au retour au premier plan.
- **Fichiers** : `POST /v1/files` (multipart `agent`, `file`) puis ligne `[Pièce jointe : … → <path>]` dans le message.

Le contrat détaillé (payloads exacts) est dans la section suivante.

## 4. Contrat d'API

### `GET /health` (public)
`{"ok": true, "version": "0.1.0", "kyutai": bool, "ntfy": bool, "apns": bool, "tts_queue": n}`

### `GET /v1/agents`
`{"agents": [{"id": "wellness", "name": "Wellness", "voice": "5476", "uploads": true, "inbox": true}]}`

### `POST /v1/tts/sentence`
Corps : `{"text": "…", "voice"?: "5476", "format"?: "pcm16"|"opus"|"wav" (défaut pcm16), "agent"?: "wellness"}`
(voix : `voice`, sinon celle de l'agent, sinon `default_voice`). Réponse 200 binaire :
- `pcm16` : `Content-Type: application/octet-stream`, PCM signé 16 bits little-endian, mono ;
  en-têtes `X-Sample-Rate: 24000`, `X-Channels: 1`, `X-Sample-Format: s16le`
- `opus` : `audio/ogg` (Ogg Opus) ; `wav` : `audio/wav`
- toujours `X-Cache: hit|miss`. Erreurs : 400 (texte vide après normalisation, format), 404 (agent), 413 (> 1000 car.),
  502 (Kyutai injoignable / erreur).

### `POST /v1/tts/stream`
Même corps que `/v1/tts/sentence` (le format est toujours PCM16). Relaie Kyutai `POST /v1/audio/stream` :
réponse 200 en transfert *chunked*, PCM s16le mono 24 kHz envoyé à mesure qu'il est produit (premier son ≈ 0,75 s,
quelle que soit la longueur du texte), en-têtes `X-Sample-Rate`, `X-Channels`, `X-Sample-Format: s16le`.
Les erreurs survenues avant le premier octet audio sont des erreurs HTTP classiques (400, 404, 413 au-delà de
8000 car., 502). Un Kyutai sans route de streaming (404) est remplacé par une synthèse en un bloc. Un flux complet
est mis en cache. L'app l'utilise en Live et retombe sur `/v1/tts/sentence` si le bridge ne connaît pas la route.

### Voix et moteurs
`voice` (corps des routes TTS, ou `voice` d'un agent dans la config) : une voix Kyutai (`5476`, `4193`, `5207`, chemin
`cml-tts/fr/…`) passe par Kyutai 1.6B (GPU) ; une voix `pocket:<nom>` (`pocket:colibri`, `pocket:mousse`, `pocket:lumen`,
`pocket:galet`, `pocket:ours`, cf. `voices/french/`) passe par Kyutai Pocket TTS (CPU, section `[pocket]`), avec sa
propre file d'attente. Si Pocket n'est pas configuré, est injoignable ou échoue avant le premier son, la requête
retombe sur la voix Kyutai par défaut (cet audio-là n'est pas mis en cache). `/health` indique `"pocket": true|false|null`.

Autres langues : `pocket:<langue>/<nom>` (`pocket:en/colibri`, `pocket:es/ours`, `pocket:de/galet`, cf.
`voices/english|spanish|german/`) passe au modèle Pocket de cette langue ; le serveur Pocket reçoit `voice` =
`en/colibri`. Le texte est préparé dans la langue de la voix (`bipbridge/speech_intl.py`, nombres via `num2words` :
« $5 » → « five dollars », « 23:15 Uhr » → « dreiundzwanzig Uhr fünfzehn »). Hors français, un échec de Pocket ne
retombe pas sur Kyutai (voix française) : le bridge répond 502/503 et l'app lit le texte avec la voix de l'iPhone.

### Tâches planifiées et fichiers des agents
- Le bridge relit toutes les minutes les sessions `cron_…` de chaque agent et met la réponse finale de chaque
  tâche dans la Boîte (`[SILENT]` ignoré, section `[cron]`). `GET /v1/cron-jobs?agent=` liste les tâches vues
  (`{agent, job, name, notify, last_seen}`) ; `PUT /v1/cron-jobs/{agent}/{job}` `{"notify": false}` les range
  dans la Boîte sans notification.
- **Bot Chat** (Hermes Bot Mode) : le même surveillant relit la « Bot Chat » de chaque agent. Un tour que l'agent
  prend de lui-même (réponse d'un coéquipier via `message_agent`, routine) est poussé comme « réponse prête »,
  sauf si l'app suivait ce tour ou si la conversation est ouverte sur le téléphone. L'app signale qu'une
  conversation est ouverte en interrogeant `GET /v1/sessions/{id}/state?agent=` (→ `message_count`) toutes les
  quelques secondes, ce qui lui sert aussi à afficher ces tours sans recharger à la main.
- `GET /v1/media?path=/home/hermes/.hermes/media/…` sert un fichier désigné par une ligne `MEDIA:<chemin>` d'un agent, s'il est
  sous un dossier de `[media.roots]` et a une extension média (audio, image, PDF). Dans un message de la Boîte,
  la ligne est retirée du texte et un mp3 devient l'audio du message.

### `WS /v1/voice`
Client → serveur (texte JSON) :
- `{"type":"follow","agent":"wellness","run_id":"…","voice"?:"5476","format"?:"pcm16"|"opus"|"wav","from_seq"?:0}`
  (`from_seq` : reprise après reconnexion, les phrases de numéro < `from_seq` ne sont ni synthétisées ni renvoyées ;
  un nouveau `follow` annule le précédent)
- `{"type":"cancel"}` · `{"type":"ping"}`

Serveur → client :
- `{"type":"following","run_id","agent","format","sample_rate":24000,"channels":1}`
- pour chaque phrase, dans l'ordre : `{"type":"sentence","seq":n,"text":"…","format","sample_rate","channels","bytes":len}`
  **immédiatement suivi** d'un message **binaire** (l'audio de la phrase, PCM16 brut par défaut)
- `{"type":"done","run_id","reason":"completed"|"failed"|"cancelled"|"interrupted"|"error"|"stream_closed"}`
  (après `cancel` : `{"type":"done","run_id":null,"reason":"cancelled"}` si un suivi était actif)
- `{"type":"error","code","message"[,"seq"]}` — codes : `unknown_agent`, `invalid_run_id`, `invalid_format`,
  `invalid_json`, `run_not_found`, `hermes_refused`, `hermes_unreachable`, `tts_failed` (phrase sautée, le flux continue),
  `internal` · `{"type":"pong"}`

Découpage : fin de phrase `.!?…` suivie d'un blanc (jamais `3.5`, ni `M.`/`etc.`/initiales/`1.` de liste), saut de ligne,
ou ~180 caractères sur une limite de mot (de préférence après une virgule). Le texte en attente est aussi prononcé à
`tool.started` / `approval.request`. Markdown retiré, liens → leur texte, URL nues supprimées, blocs de code ignorés,
éléments de liste terminés par un point. Si aucun delta n'est arrivé, le texte final de `run.completed` est lu.

### `POST /v1/files` (multipart/form-data : `agent`, `file`)
200 : `{"path": "/home/hermes/.hermes/profiles/wellness/uploads/2026-10/0a1b2c3d4e5f-Releve_oct.pdf",
"filename": "Releve_oct.pdf", "size": 220512, "content_type": "application/pdf"}`
(`path` = chemin **dans le conteneur Hermes** ; sur l'hôte : `{upload_dir_host}/AAAA-MM/<12 hex>-<nom>`, mode 640).
Nom assaini : base seule, sans caractères de contrôle, espaces et caractères shell → `_`, pas de point initial, ≤ 120 car.
Erreurs : 400 (multipart invalide / pas de `file`), 404 (agent inconnu), 409 (uploads non configurés pour l'agent),
413 (> 50 Mo).

### `POST /v1/devices`
Corps : `{"token": "<hex APNs>", "environment"?: "sandbox"|"production", "agent_ids"?: ["wellness","vie"]}`
(`agent_ids` vide = tous les agents ; `environment` absent = `[apns].environment`). Réponse 200 :
`{"token","environment","agent_ids","created_at","updated_at"}`. Ré-enregistrer met à jour. 400 : token non hexadécimal,
environnement invalide, `{"detail":{"error":"unknown agent_ids","agent_ids":[…]}}`.

### `DELETE /v1/devices/{token}` → 204 (idempotent)

### `GET /v1/outbox?since=<ISO 8601>&agent=<id>&limit=<1..500, défaut 100>`
`{"items": [Item…], "server_time": "2026-10-04T08:15:02.120Z"}`, trié par `created_at` croissant, `since` exclusif
(ISO avec `Z`/décalage, ou secondes Unix). Item :
```json
{"id": "9f3c…(32 hex)", "agent": "wellness", "title": "Check-in", "text": "texte complet (markdown)",
 "created_at": "2026-10-04T08:15:01.532Z", "sent_at": "2026-10-04T08:15:01.000Z", "session_id": null,
 "has_audio": true, "audio_url": "/v1/outbox/9f3c…/audio"}
```
`created_at` = réception par le bridge (UTC, ms, format fixe) ; `sent_at` = horodatage ntfy ; `session_id` vient d'un tag
ntfy `session:<id>` s'il existe. Pour la pagination, repasser le `created_at` du dernier élément en `since`.

### `GET /v1/outbox/{id}` → Item (404 sinon)

### `GET /v1/outbox/{id}/audio`
200 `audio/mpeg` ; si la synthèse est encore en cours (attente ≤ 20 s) : 202 `{"status":"pending"}` + `Retry-After: 3` ;
404 si pas d'audio (texte > `audio_max_chars`) ; 503 si Kyutai a échoué.

### `POST /v1/watch`
Corps : `{"agent":"wellness","run_id":"…"}` → `{"watching": true, "agent", "run_id", "followers": n, "finished": bool}`.
Le bridge s'abonne au run (un seul abonnement Hermes par run, partagé avec le WebSocket, avec rejeu pour un suivi
tardif) pendant au plus `watch_max_minutes`. Sur `approval.request` sans client attaché (WebSocket vocal ou flux de
l'app) : push `APPROVAL` (une fois par `request_id`). À la fin du run sans client attaché : push « réponse prête »
(`run.completed` avec une réponse, catégorie `MESSAGE`, `kind: "reply"`, `session_id` si connu), push silencieux sinon.

### `GET /v1/runs/{run_id}/events?agent=…&session_id=…`
Flux SSE du run pour l'app, relayé depuis l'abonnement unique du bridge à Hermes (Hermes ne livre chaque événement
qu'à un seul abonné) : mêmes événements que Hermes (`event: <type>`, `data: <json>`), `: keepalive` toutes les 10 s.
Le run est rejoué depuis son début à chaque connexion (historique gardé 15 min après la fin), donc une reconnexion
reconstruit la réponse à l'identique. Vaut `POST /v1/watch` : tant que l'app écoute, aucun push ; quand elle part
(conversation quittée, écran verrouillé), le bridge pousse l'approbation ou la réponse prête, qui rouvre la
conversation `session_id`. `bridge.error` (`run_not_found`, `hermes_refused`, `hermes_unreachable`, `watch_timeout`)
signale un problème côté bridge.

### `POST /v1/approve`
Corps : `{"agent":"wellness","run_id":"…","choice":"once"|"session"|"always"|"deny","request_id"?:"…"}` → relayé à
Hermes `POST /v1/runs/{run_id}/approval` avec la clé de l'agent ; statut et JSON de Hermes renvoyés tels quels
(200 `{"object":"hermes.run.approval_response",…}`, 409 `approval_not_pending`, …). 502 si Hermes injoignable.

### `GET /v1/approvals?agent=<id>`
`{"items":[{"agent","run_id","request_id","command","description","choices":["once","session","deny"],"created_at"}]}` —
approbations vues par le bridge et pas encore résolues (commande déjà expurgée par Hermes).

### Payloads APNs
```json
MESSAGE  {"aps":{"alert":{"title":"Wellness","body":"Nouveau message"},"thread-id":"wellness","mutable-content":1,
          "category":"MESSAGE","sound":"default"},"outbox_id":"…","agent":"wellness","session_id":"…"}
APPROVAL {"aps":{"alert":{"title":"Wellness","body":"Approbation requise"},"thread-id":"wellness","mutable-content":1,
          "category":"APPROVAL","sound":"default"},"agent":"wellness","run_id":"…","request_id":"…",
          "choices":["once","session","deny"]}
SILENT   {"aps":{"content-available":1},"agent":"wellness","reason":"run_finished","run_id":"…","status":"run.completed"}
```
`apns-push-type` : `alert` (priorité 10) ou `background` (priorité 5) ; `apns-collapse-id` = `request_id` pour les
approbations ; `apns-topic` = `bundle_id`. Jeton JWT ES256 renouvelé toutes les 50 min (et sur `ExpiredProviderToken`).
Un token en 410, ou 400 `BadDeviceToken`/`DeviceTokenNotForTopic`, est supprimé. Avec `previews = true`, le corps
contient un extrait du texte (ou de la commande).

## 5. Fonctionnement interne

- **Kyutai sérialisé** : une seule requête à la fois côté bridge aussi, servie par ordre d'arrivée ; priorité
  « interactive » (voix, `/v1/tts/sentence`) devant « arrière-plan » (audio de l'outbox) — jamais d'interruption d'une
  synthèse déjà lancée. Une requête annulée (barge-in) quitte la file. Cache LRU (voix, format, texte normalisé).
- **Prérequête** : chaque phrase est mise en file Kyutai dès qu'elle est complète, l'envoi au client reste dans l'ordre.
- **SSE tolérant** : type lu dans le JSON (`type` ou `event`) sinon la ligne `event:` ; `: keepalive` ignoré ;
  texte pris dans `delta` / `text` / `content`. Événements de texte : `message.delta`, `assistant.delta`.
- **Journaux** : JSON sur stdout (journald), jamais de secret ni de texte de message ; tokens d'appareil tronqués ;
  pas de journal d'accès uvicorn (les chemins contiennent des tokens).

Limites connues : `WS /v1/stt` (STT serveur, v2) n'est pas implémenté. Si la connexion locale bridge → Hermes coupe
au milieu d'un run et que Hermes rejoue les événements à la réabonnement, une phrase pourrait être répétée
(cas improbable : les deux sont sur 127.0.0.1).

## 6. Développement et tests

```bash
cd bridge
python3 -m venv .venv && .venv/bin/pip install -r requirements-dev.txt
.venv/bin/python -m pytest -q
```

Les tests n'appellent aucun service réel : Kyutai, Hermes (SSE des runs, approbation), ntfy et APNs sont simulés
par `httpx.MockTransport` (`tests/conftest.py`). Ils couvrent l'authentification, l'ordre FIFO du TTS, le découpage en
phrases, `/v1/files` (chemin, taille, noms), les appareils, l'ingestion ntfy → outbox → push, la décision de push
d'approbation, `/v1/approve`, le client APNs (JWT, renouvellement) et le WebSocket vocal (suivi, annulation, erreurs).
