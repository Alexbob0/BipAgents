[🇬🇧 English](README.md) · 🇫🇷 Français

# Relais de notifications BipAgents

Chacun fait tourner BipAgents sur son propre serveur ; son bridge ne peut pas détenir la clé Apple ni le compte
Firebase de l'éditeur. Le relais, si : il transmet leurs notifications à Apple et à Google. C'est un petit programme
Rust (un binaire de 4,6 Mo, ~4 Mo de mémoire au repos) qui **ne stocke rien** et **ne transmet aucun contenu**.

## Fonctionnement

1. L'app inscrit son jeton Apple ou Firebase : `POST /v1/register` → un **jeton de relais** (`r2.…`), le jeton de
   l'appareil chiffré avec une clé que seul le relais détient (XChaCha20-Poly1305), avec son identifiant, une fin
   (90 jours) et qui peut s'en servir (`audience` : `bridge`, ou `guichet`). L'app le donne à son bridge (ou au
   guichet), et en prend un nouveau tous les 30 jours.
2. Le bridge envoie sa notification avec le jeton de relais : `POST /v1/relay/push` (même forme que le relais des
   bridges). Le relais ouvre le jeton, nettoie la notification et l'envoie à Apple (HTTP/2, jeton ES256) ou à Google
   (FCM v1).
3. La réponse d'Apple ou de Google revient au bridge ; `status: 410` veut dire que l'appareil n'existe plus, et le
   bridge l'oublie.

Rien à sauvegarder, pas de base de données : autant d'exemplaires qu'on veut côte à côte, qui peuvent s'arrêter quand
personne ne s'en sert.

## Ce qu'il ne voit jamais, ce qu'il écarte

- **Aucun texte de message.** Un bridge réglé pour afficher des aperçus met une ligne du message dans la notification ;
  le relais remplace tout texte qui n'est pas l'un des textes génériques et traduits de l'app (`loc-key`) par
  « Nouveau message ». À côté de `aps`, seules passent de courtes valeurs de type identifiant (agent, message,
  conversation…). L'extension de notification de l'app va chercher le vrai texte sur le bridge de la personne.
- **Aucun jeton d'appareil en clair**, ni stocké (il n'y a pas de stockage) ni dans les journaux (une empreinte de
  4 octets seulement).
- **Quotas** : par appareil (300 notifications par heure), par adresse (20 000 notifications, 60 inscriptions par heure).

## API

| Route | Corps | Réponse |
| --- | --- | --- |
| `POST /v1/register` | `{"platform": "apns"\|"fcm", "token": "…", "environment": "sandbox"\|"production", "audience": "bridge"\|"guichet"}` | `{"relay_token": "r2.…", "expires": 1800000000}` |
| `POST /v1/relay/push` (ou `/v1/push`) | `{"token": "r2.…", "payload": {…}, "push_type": "alert"\|"background"\|"liveactivity", "priority": 5\|10, "collapse_id": "…"}` ; un jeton `guichet` avec `Authorization: Bearer <RELAY_GUICHET_KEY>` | `{"status": 200, "reason": null, "apns_id": "…"}` |
| `POST /v1/report` | `{"reason", "comment", "text", "category", "app", "model", "platform": "ios"\|"android", "lang"}` | `202 {"ok": true}` : « Signaler cette réponse » transmis à `RELAY_REPORT_URL` avec le secret, gardé nulle part ; 503 s'il n'est pas réglé, 502 s'il n'a pas pu partir (l'app propose alors un e-mail) |
| `GET /health` | | `{"status": "ok", "apns": true, "fcm": true}` |

Un jeton chiffré avec une autre clé, ou altéré, répond `{"status": 410, "reason": "BadRelayToken"}` ; un jeton
arrivé à sa fin, ou du premier format (`r1.…`, sans fin), `{"status": 410, "reason": "ExpiredRelayToken"}` (le bridge
l'oublie, l'app se réinscrit) ; un jeton d'appareil brut est refusé en HTTP 400 (un bridge réglé pour l'envoi direct,
pas un appareil disparu). Un jeton `guichet` envoyé sans la clé du guichet est refusé en HTTP 401 : une copie des
jetons du guichet ne sert à rien seule.

Ce qui passe dans `aps` : jamais `alert.title` (les apps nomment l'agent d'après leur propre liste) ; les textes du
guichet (serveurs, compte, famille, conversations partagées) seulement sur un jeton `guichet` ; `category`
`MESSAGE`/`APPROVAL`, `sound` `default`, `interruption-level` `active`/`passive`, rien d'autre.

## Réglages (environnement seulement)

| Variable | |
| --- | --- |
| `RELAY_SEAL_KEYS` | Obligatoire. Clés base64 de 32 octets, séparées par des virgules ; la première chiffre, toutes ouvrent (`bipagents-relay keygen`). Pour changer de clé : mettre la nouvelle en premier, garder l'ancienne quelques mois. |
| `APNS_KEY` ou `APNS_KEY_FILE`, `APNS_KEY_ID`, `APNS_TEAM_ID`, `APNS_TOPIC` | Apple : la clé .p8 de l'équipe et l'identifiant de l'app. Sans elles, les appareils Apple sont refusés. |
| `FCM_SERVICE_ACCOUNT` ou `FCM_SERVICE_ACCOUNT_FILE` | Google : le compte de service Firebase (JSON). Sans lui, les appareils Android sont refusés. |
| `PORT` ou `RELAY_LISTEN` | `8080` / `0.0.0.0:8080` |
| `RELAY_PUSHES_PER_DEVICE_HOUR`, `RELAY_PUSHES_PER_IP_HOUR`, `RELAY_REGISTRATIONS_PER_IP_HOUR` | `300`, `20000`, `60` |
| `RELAY_TRUST_FORWARDED_FOR` | `1` derrière un proxy qui pose `X-Forwarded-For` |
| `RELAY_REPORT_URL`, `RELAY_REPORT_SECRET` ou `RELAY_REPORT_SECRET_FILE`, `RELAY_REPORTS_PER_IP_HOUR` | où vont les signalements (le `/v1/reports` du guichet), le secret qu'il vérifie, `20` |
| `RELAY_BLOCKED` ou `RELAY_BLOCKED_FILE` | appareils refusés pour abus (conditions d'utilisation), par l'empreinte de 8 caractères hexadécimaux que montrent les journaux, ou un jeton de relais qui a fuité, par son identifiant de 16 caractères (`token` dans les journaux) ; redémarrer pour appliquer |
| `RELAY_GUICHET_KEY` ou `RELAY_GUICHET_KEY_FILE` | la clé avec laquelle le guichet envoie ses notifications (séparées par des virgules pour en changer : l'ancienne et la nouvelle passent) ; sans elle, les jetons `guichet` sont refusés |
| `RUST_LOG` | `info` (lignes JSON) |

## Le lancer

```bash
cargo run --release -- keygen                      # une clé de chiffrement
RELAY_SEAL_KEYS=… APNS_KEY_FILE=AuthKey.p8 APNS_KEY_ID=… APNS_TEAM_ID=… APNS_TOPIC=com.kerfystudio.bipagents \
  cargo run --release
```

En conteneur (une image de 7,5 Mo qui ne contient que le binaire) : `podman build -t bipagents-relay relay/`.

Il lui faut du HTTPS devant : une offre de conteneurs « serverless » (il démarre en quelques millisecondes et peut
s'éteindre au repos), ou une petite VM avec Caddy. Un exemplaire suffit pour un grand nombre d'installations ; on en
ajoute derrière la même adresse si besoin.

## Développer

`cargo test` (Apple et Google simulés par un serveur local), `cargo clippy --all-targets`, `cargo fmt --check`.
