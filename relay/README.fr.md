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
| `GET /health` | | `{"status": "ok", "apns": true, "fcm": true, "family": {"enabled": true}}` |

Un jeton chiffré avec une autre clé, ou altéré, répond `{"status": 410, "reason": "BadRelayToken"}` ; un jeton
arrivé à sa fin, ou du premier format (`r1.…`, sans fin), `{"status": 410, "reason": "ExpiredRelayToken"}` (le bridge
l'oublie, l'app se réinscrit) ; un jeton d'appareil brut est refusé en HTTP 400 (un bridge réglé pour l'envoi direct,
pas un appareil disparu). Un jeton `guichet` envoyé sans la clé du guichet est refusé en HTTP 401 : une copie des
jetons du guichet ne sert à rien seule.

**Transition (jusqu'à `RELAY_R1_UNTIL`, le 2026-12-15 par défaut).** Les apps, bridges et le guichet sortis avant le
2026-10-28 ne connaissent que le premier format ; jusqu'à cette date : les jetons `r1.` marchent encore (tout
expéditeur, comme avant) ; une app qui s'inscrit sans `audience` (une ancienne) reçoit un jeton `r1.` ; une
inscription `bridge` renvoie aussi `legacy_token` (`r1.`), qu'une app récente donne à un bridge qui refuse `r2.` ; un
jeton `guichet` envoyé sans aucun `Authorization` passe (un ancien guichet). Chaque cas est journalisé (`warn`) et
`/health` ajoute `r1_until`, `r1_used`, `guichet_without_key` et un `warning`. Après cette date, tout ce qui précède
s'applique. `RELAY_R1_UNTIL=0` y met fin tout de suite.

Ce qui passe dans `aps` : jamais `alert.title` (les apps nomment l'agent d'après leur propre liste) ; les textes du
guichet (serveurs, compte, famille, conversations partagées) seulement sur un jeton `guichet` ; `category`
`MESSAGE`/`APPROVAL`, `sound` `default`, `interruption-level` `active`/`passive`, rien d'autre.

## Familles : deux foyers qui ne peuvent pas se joindre

Depuis le 15/11/2026, le relais permet aussi aux ponts de deux foyers de se relier et de se parler quand aucun ne
peut joindre l'autre (deux installations à la maison sur deux réseaux, ou une place de l'offre qui scanne
l'invitation d'une maison). Les deux se connectent vers le relais ; le rendez-vous est en mémoire (`src/family.rs`,
côté pont : `bridge-rs/src/family_rendezvous.rs`).

| Route | | |
| --- | --- | --- |
| `POST /v1/family/mailbox` | `{}`, `{"token": "fm1.…"}` (renouveler) ou `{"invite_until": <unix>}` (celle d'une invitation) | `{"mailbox": "<22 caractères>", "token": "fm1.…", "expires": …}` : l'identifiant scellé dans le jeton, rien de stocké |
| `POST /v1/family/pass` | le jeton de la boîte ; `{"for": "<la boîte de l'autre foyer>"}`, ou `{"renew": "fp1.…"}` (par la boîte qu'il ouvre seulement) | `{"pass": "fp1.…", "id": …, "expires": …}` : permet à cette boîte-là, et à aucune autre, d'appeler celle-ci (scellé : les deux boîtes, un identifiant, une fin) |
| `POST /v1/family/passes` | le jeton de la boîte ; `{"accept": [identifiants]}` | 204 : les laissez-passer qu'elle accepte désormais (un lien retiré) ; ses requêtes en attente le redisent (`x-bip-passes`) |
| `GET /v1/family/next` | le jeton de la boîte | l'appel suivant pour elle (`x-bip-call`, `x-bip-from`, `x-bip-kind`, ses octets), ou 204 après 25 s |
| `POST /v1/family/call/<boîte>` | le jeton de l'appelant, `Content-Length`, `x-bip-kind: call` avec `x-bip-pass`, ou `invite` (≤ 64 Kio, vers la boîte d'une invitation), les octets scellés (≤ 16 Mio) | la réponse, en flux ; 503 `away` si ce pont n'est pas connecté **ou si l'appelant n'a pas de laissez-passer** (la même réponse) |
| `POST /v1/family/answer/<appel>` | le jeton de la boîte qui a pris l'appel | 204 ; transmis en flux à l'appelant (≤ 64 Mio) |

**Seuls les foyers reliés appellent une boîte.** Au moment du lien, chaque pont donne à l'autre un laissez-passer pour
sa propre boîte, valable pour la boîte de l'autre seulement, scellé dans leur échange ; il redit à chaque requête en
attente les laissez-passer qu'il accepte encore, et en retire un tout de suite quand le lien est retiré. Seule la boîte
qu'il ouvre peut le renouveler. La boîte d'un foyer n'est jamais dans un QR code : une invitation a sa
propre boîte, qui finit avec elle et ne prend qu'une présentation (`invite`). Un inconnu qui apprend l'identifiant
d'une boîte ne peut ni l'appeler, ni épuiser ses quotas, ni savoir si son pont est allumé.

**Scellé de bout en bout** : les ponts scellent tout avec une clé propre à chaque lien, que seuls les deux foyers
détiennent (un échange X25519 au moment du lien, lié au code d'invitation à usage unique ; des trames
ChaCha20‑Poly1305, une clé par appel, par sens et par réponse, les rejeux refusés par le pont). Le relais **voit** les
adresses des ponts, quelle boîte appelle quelle boîte, quand, combien d'octets, et, tant qu'un pont garde sa requête
ouverte, qu'il est allumé et depuis quelle adresse. Il **ne garde** rien sur disque et ne journalise aucun appel
réussi (pas de trace de qui parle à qui) : seulement certains refus (quota, boîte bloquée, réponse coupée), avec une empreinte d'une boîte calculée avec une clé du relais (HMAC-SHA256, pas recalculable depuis
l'identifiant seul), et un total par heure. En mémoire : les identifiants des boîtes qui interrogent (jusqu'à une
heure après), les compteurs des quotas sous des empreintes SHA-256 sans clé (oubliés dans l'heure), les appels le
temps qu'ils passent. `/health` dit s'il est en marche, rien de son activité.

**Limites.** Les octets d'un appel sont réservés au fil de la lecture, jamais sur la longueur annoncée (256 Mio en
tout, tous appels confondus ; 32 Mio par appelant, par lien vers un appelé et par adresse) ; un corps qui s'arrête 5 s
est coupé, de même qu'un corps plus lent que 128 Kio/s en moyenne après sa première seconde (environ 1 Mbit/s : un
envoi plus lent ne fait passer que quelques centaines de Kio par le relais), et tout doit arriver en 30 s ; le corps
d'un appel refusé n'est jamais gardé. Les petites routes (`mailbox`, `pass`, `passes`) lisent 64 Kio au plus, en
10 s. Les listes de laissez-passer qu'une boîte accepte ne sont gardées que tant qu'elle interroge (et une minute
après), 250 000 identifiants en tout : au-delà, une liste ne peut que rétrécir (un laissez-passer retiré part
toujours ; un nouveau attend de la place). Quotas : appels et octets par appelant, et par laissez-passer (ce qu'un foyer peut envoyer
à un autre) ; boîtes et laissez-passer créés par adresse. Une boîte ou un laissez-passer peut être refusé par son
empreinte dans `RELAY_BLOCKED`. **Un seul** exemplaire pour les familles (le rendez-vous est en mémoire). Déploiement :
`RELAY_TRUST_FORWARDED_FOR=1` derrière un proxy, une limite de fichiers ouverts large (une requête en attente par
foyer relié par le relais), `flush_interval -1` dans Caddy, journaux gardés 14 jours.

## Réglages (environnement seulement)

| Variable | |
| --- | --- |
| `RELAY_SEAL_KEYS` | Obligatoire. Clés base64 de 32 octets, séparées par des virgules ; la première chiffre, toutes ouvrent (`bipagents-relay keygen`). Pour changer de clé : mettre la nouvelle en premier, garder l'ancienne quelques mois. |
| `APNS_KEY` ou `APNS_KEY_FILE`, `APNS_KEY_ID`, `APNS_TEAM_ID`, `APNS_TOPIC` | Apple : la clé .p8 de l'équipe et l'identifiant de l'app. Sans elles, les appareils Apple sont refusés. |
| `FCM_SERVICE_ACCOUNT` ou `FCM_SERVICE_ACCOUNT_FILE` | Google : le compte de service Firebase (JSON). Sans lui, les appareils Android sont refusés. |
| `PORT` ou `RELAY_LISTEN` | `8080` / `0.0.0.0:8080` |
| `RELAY_PUSHES_PER_DEVICE_HOUR`, `RELAY_PUSHES_PER_IP_HOUR`, `RELAY_REGISTRATIONS_PER_IP_HOUR` | `300`, `20000`, `60` |
| `RELAY_TRUST_FORWARDED_FOR` | `1` derrière un proxy qui ajoute à `X-Forwarded-For` (sa dernière adresse compte ; IPv6 par /64) |
| `RELAY_REPORT_URL`, `RELAY_REPORT_SECRET` ou `RELAY_REPORT_SECRET_FILE`, `RELAY_REPORTS_PER_IP_HOUR` | où vont les signalements (le `/v1/reports` du guichet), le secret qu'il vérifie, `20` |
| `RELAY_BLOCKED` ou `RELAY_BLOCKED_FILE` | appareils refusés pour abus (conditions d'utilisation), par l'empreinte de 8 caractères hexadécimaux que montrent les journaux, ou un jeton de relais qui a fuité, par son identifiant de 16 caractères (`token` dans les journaux) ; redémarrer pour appliquer |
| `RELAY_GUICHET_KEY` ou `RELAY_GUICHET_KEY_FILE` | la clé avec laquelle le guichet envoie ses notifications (séparées par des virgules pour en changer : l'ancienne et la nouvelle passent) ; sans elle, les jetons `guichet` sont refusés |
| `RELAY_FAMILY` | `1` ; `0` coupe le rendez-vous des familles (ses routes répondent 404) |
| `RELAY_FAMILY_REQUEST_MB`, `RELAY_FAMILY_ANSWER_MB` | `16`, `64` : la taille d'un appel, de sa réponse |
| `RELAY_FAMILY_BODY_KIB_PER_S` | `128` : le débit minimal du corps d'un appel, après sa première seconde (`0` : aucun) |
| `RELAY_FAMILY_CALLS_PER_HOUR`, `RELAY_FAMILY_MB_PER_HOUR`, `RELAY_FAMILY_MAILBOXES_PER_IP_HOUR` | `3000` par boîte comme appelant et comme appelé, `2048` Mio par boîte, `20` boîtes créées par adresse |
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
