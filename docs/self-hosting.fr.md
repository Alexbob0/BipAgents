[🇬🇧 English](self-hosting.md) · 🇫🇷 Français

# Héberger BipAgents chez soi : Mac mini, VPS ou serveur Linux

BipAgents n'a besoin d'**aucun GPU** : les voix des Bips tournent sur CPU (Kyutai Pocket TTS). Une machine qui fait
déjà tourner [Hermes Agent](https://github.com/NousResearch/hermes-agent) suffit, qu'il s'agisse d'un Mac mini, d'un VPS
ou d'un serveur Linux à la maison. L'installateur fait tout cela en une commande ([README](../README.fr.md)) ; ce
guide sert à le faire à la main, ou à comprendre ce qu'il a mis en place.

```
téléphone (BipAgents) ──Tailscale──▶ ta machine
                                      ├─ Hermes Agent, api_server   :8642 (un port par agent : 8644, …)
                                      ├─ bridge (bridge-rs/)        :8643  voix, fichiers, push, Boîte
                                      └─ Pocket TTS (pocket/)       :8098  voix des Bips (local, appelé par le bridge)
```

Rien n'est exposé sur Internet : tout passe par ton tailnet, en HTTPS avec un vrai certificat (`tailscale serve`).

## Quelle machine ?

| Machine | Ce qu'il faut | Remarques |
|---|---|---|
| **Mac mini** Apple Silicon | 16 Go conseillés | Mesuré sur M4 : premier son ≈ 50 ms, ≈ 6,5× le temps réel, ≈ 0,6 Go pour les 4 langues. Désactiver la mise en veille et activer l'ouverture de session automatique (les services sont des LaunchAgents). |
| **VPS** Linux | 4 vCPU (dont 2 pour Pocket), 8 Go | vCPU **dédiés** de préférence (les vCPU partagés ralentissent la voix). ARM (Ampere, Hetzner CAX) ou x86 avec `--quantize`. Hermes appelle son modèle de langage par API, il n'a pas besoin de GPU. Non mesuré : vérifier le premier son après installation. |
| **Serveur Linux** avec GPU | | Tout ce qui précède, plus le bonus ci-dessous : ton propre modèle de voix. |

## 1. Tailscale

Installer Tailscale sur la machine et sur ton téléphone, dans le même tailnet. Dans la console d'administration :
*DNS* → activer **MagicDNS** et **HTTPS Certificates**. La machine a alors un nom du type `mamachine.tailnet.ts.net`.

Sur Mac, la commande `tailscale` est fournie par l'app (*Réglages de Tailscale* → installer l'interface en ligne de
commande), ou par `brew install tailscale`.

## 2. Hermes : un api_server par agent

Chaque agent (profil Hermes) expose son api_server sur un port local. Dans le `.env` du profil :

```bash
API_SERVER_ENABLED=true
API_SERVER_HOST=127.0.0.1        # 0.0.0.0 si Hermes tourne dans un conteneur, publié sur 127.0.0.1
API_SERVER_PORT=8642             # 8644 pour le deuxième agent, etc.
API_SERVER_KEY=<clé aléatoire d'au moins 32 caractères, une par agent>
```

Vérifier : `curl -H "Authorization: Bearer $KEY" http://127.0.0.1:8642/v1/capabilities` doit annoncer les runs
(`run_submission`, `run_events_sse`, `run_approval`). Les questions de l'agent (outil `clarify`) demandent une version
de Hermes qui les branche sur l'api_server, cf. [`hermes-clarify-api.md`](hermes-clarify-api.fr.md).

## 3. Le dépôt et Pocket TTS (voix des Bips)

```bash
git clone https://github.com/Alexbob0/BipAgents ~/BipAgents
cd ~/BipAgents/pocket
python3 -m venv .venv && .venv/bin/pip install -r requirements.txt    # Python ≥ 3.10 (VPS : voir ci-dessous)
.venv/bin/python pocket_server.py --voices ../voices --languages fr,en,es,de,nl
curl -s http://127.0.0.1:8098/health
```

Le premier lancement télécharge les modèles (≈ 440 Mo par langue). Ne charger que les langues utiles accélère le
démarrage (`--languages fr`). Détails : [`pocket/README.md`](../pocket/README.fr.md).

### Sur un VPS

- **Système** : Ubuntu 24.04 ou Debian 12 conviennent (`sudo apt install python3-venv python3-pip git`).
- **PyTorch sans CUDA** : sur Linux x86, `pip` installe par défaut PyTorch avec CUDA (plusieurs Go inutiles sans
  GPU). Installer d'abord la version CPU, puis le reste :

  ```bash
  .venv/bin/pip install torch --index-url https://download.pytorch.org/whl/cpu
  .venv/bin/pip install -r requirements.txt
  ```

  (Sur ARM, Hetzner CAX ou Ampere, la version par défaut est déjà sans CUDA.)
- **Partager le CPU avec Hermes** : `--threads 2` réserve 2 cœurs à Pocket (même vitesse mesurée qu'avec tous les
  cœurs sur Apple M4). Sur x86, **mettre `--quantize`** (poids int8) : mesuré sur un serveur x86, le premier son passe de ≈ 140 ms à ≈ 55 ms ;
  `pip install "pocket-tts[quantize]"` ajoute torchao pour l'optimiser.
- **Mémoire et disque** : ≈ 0,6 à 1 Go de RAM pour les 4 langues, ≈ 2 Go de disque pour les modèles (dans
  `~/.cache/huggingface`, `HF_HOME` pour les mettre ailleurs) et ≈ 1 Go pour PyTorch CPU. Ajouter un peu de swap
  sur un VPS de 4 Go.
- **Réseau** : laisser Pocket sur `127.0.0.1` (option par défaut) et ne rien ouvrir au pare-feu : le bridge est sur
  la même machine, le téléphone passe par Tailscale. Avec `ufw` : `sudo ufw allow in on tailscale0`, rien d'autre.
- **Mesurer** une fois installé : `python3 pocket/bench.py` (premier son et vitesse). Visé : premier son sous
  ≈ 300 ms, au moins 1,5× le temps réel. Sinon : `--quantize` (x86), puis des vCPU dédiés.

## 4. Le bridge

Le bridge est un seul programme, écrit en Rust (`bridge-rs/`). Chaque version du dépôt a ses binaires dans la
release GitHub `bridge-<clé>`, la clé étant les 12 premiers caractères du SHA-256 de l'arbre `bridge-rs/` :

```bash
cd ~/BipAgents
key=$(printf '%s' "$(git rev-parse HEAD:bridge-rs)" | sha256sum | cut -c1-12)    # macOS : shasum -a 256
asset=bipbridge-linux-x86_64.tar.gz    # bipbridge-linux-aarch64.tar.gz (ARM), bipbridge-macos-universal.tar.gz (Mac)
mkdir -p ~/.local/bin
curl -fsSL "https://github.com/Alexbob0/BipAgents/releases/download/bridge-$key/$asset" | tar -xz -C ~/.local/bin bipbridge bip-browser-mcp
# Ou le compiler (Rust stable) : cd bridge-rs && cargo build --release && cp target/release/{bipbridge,bip-browser-mcp} ~/.local/bin/
mkdir -p ~/.config/hermes-ios && chmod 700 ~/.config/hermes-ios
cp bridge/bridge.example.toml ~/.config/hermes-ios/bridge.toml && chmod 600 ~/.config/hermes-ios/bridge.toml
~/.local/bin/bipbridge genkey      # → bridge_key (à saisir aussi dans l'app)
```

Le `SHA256SUMS` de la release donne l'empreinte de chaque archive.

Dans `~/.config/hermes-ios/bridge.toml`, pour une machine sans GPU :

```toml
[kyutai]
enabled = false                 # Kyutai 1.6B, si tu le fais tourner, est un [voice_server] comme un autre (cf. la fin)

[pocket]
url = "http://127.0.0.1:8098"
default_voice = "pocket:colibri"

[agents.wellness]               # une section par agent
display_name = "Wellness"
hermes_url = "http://127.0.0.1:8642"
hermes_key = "<API_SERVER_KEY de cet agent>"
upload_dir_host = "~/hermes/profiles/wellness/uploads"        # où le bridge dépose les fichiers envoyés
upload_dir_container = "/home/hermes/.hermes/profiles/wellness/uploads"   # le même dossier vu par l'agent
```

Les sections `[ntfy]` et `ntfy_topic` / `ntfy_token` sont facultatives (anciens messages proactifs). Les tâches
planifiées de Hermes arrivent dans la Boîte sans elles (`[cron]`). Puis `~/.local/bin/bipbridge check --config ~/.config/hermes-ios/bridge.toml`.

Toutes les options : [`bridge/README.md`](../bridge/README.fr.md).

### Le bac à sable des agents (Linux avec podman)

Quand les outils des agents tournent dans le bac à sable de Hermes (`terminal.backend: docker`, avec podman sans
root), le bridge le garde isolé : Hermes y fait alors le travail sans risque (`python -c`, `execute_code`,
`pip install`) sans te demander à chaque fois.

- **Aucun dossier de la machine dedans** : les fichiers que tu envoies à un agent sont copiés dans un volume podman à
  lui (`bipagents-files-<agent>`, en lecture seule dans `/fichiers`) ; le dossier qu'un agent partage est aussi un
  volume podman (`bipagents-share-<agent>`), dans `/partage` de son bac à sable et en lecture seule dans
  `/lecture/<agent>` chez les agents que tu autorises à le lire. Le bridge garde `<data>/shares/<agent>/` à jour avec
  lui (les sauvegardes et les exports prennent ce dossier) ; la première fois, le contenu de ce dossier est copié dans
  le volume et le dossier gardé sous le nom `<agent>.before-volume`. Les outils `shared_files_*` atteignent les mêmes
  dossiers (tu approuves les suppressions depuis l'app).
- **Pas d'internet, sauf les dépôts de paquets** : le bac à sable est sur `bipagents-sandbox`, un réseau podman sans
  route vers l'extérieur, et n'atteint que PyPI, npm, les crates Rust, Go et Debian, par un petit conteneur proxy
  (`bipagents-egress` ; `podman logs bipagents-egress` montre ce qu'il a laissé passer ou bloqué). La recherche web et
  le navigateur visible de l'agent sont d'autres outils et marchent toujours. Chaque agent peut aussi avoir **ses
  propres sites** (HTTPS seulement) : chaque bac à sable a ses propres identifiants sur le proxy, qui sait donc quel
  agent demande. Quand le proxy refuse un site aux programmes d'un agent, ou quand l'agent le demande d'avance
  (`sandbox_sites_request`), tu reçois une seule carte dans l'app, « Autoriser toujours / Refuser » ; la liste est
  dans les réglages de l'agent, « Sites autorisés », où tu retires un site ou en ajoutes un à la main
  (`*.example.org` pour ses sous-domaines). Un nom de ton propre réseau (`localhost`, `.local`, une adresse privée ou
  Tailscale) n'est jamais laissé passer.

Les agents existants passent à ce mode au prochain démarrage du bridge (`bipbridge sandbox` le fait tout de suite et
dit ce que chacun a reçu). La section `[sandbox]` de `bridge.example.toml` liste les réglages. Sur macOS, ou avec
docker au lieu de podman, les bacs à sable restent comme avant (dossiers montés, Hermes demande avant les commandes
risquées). Pour revenir en arrière sous Linux, mets `[sandbox] isolate = "off"` et redémarre le bridge : chaque bac à
sable retrouve ses dossiers montés (ce que les agents ont écrit dans les volumes y est recopié d'abord), et les
réglages de réseau et de proxy sont retirés ; les volumes restent.

Un bac à sable qui monte encore un dossier de cette machine ajouté à la main n'est pas isolé pour Hermes, qui
demanderait avant chaque script. Là où le bridge isole le réseau, il met aussi les accords de Hermes pour « le code que
l'agent écrit » (`execute_code`, `python -c`, les heredocs, `bash -c`) dans la `command_allowlist` de l'agent : il lit
et calcule dans son espace sans demander, tandis que les commandes risquées (supprimer, fichiers d'identifiants, un
téléchargement envoyé dans un shell…), les paiements, la suppression de fichiers partagés, les outils d'écriture des
services que tu as ajoutés et les nouveaux sites demandent toujours (`approvals.mode` reste `manual`).
`bipagents sandbox adopt <agent>` déplace ces dossiers faits à la main dans des volumes. `bipagents approvals [agent]
[--last N]` liste ce que les agents ont demandé, avec ta réponse (depuis les journaux du bridge, en lecture seule).

### Le Carnet (le carnet des agents)

Le bridge garde le Carnet de la personne dans son dossier de données (`notebook/carnet/`, du Markdown simple qui
s'ouvre dans Obsidian ; `notebook/private/` pour la santé ; `notebook/history/` pour chaque changement). Son rangement
de nuit appelle un petit modèle une fois par nuit au plus, seulement quand les conversations du jour contiennent
quelque chose que les agents n'ont pas noté, sous un plafond mensuel : `[notebook] model` (par défaut
le modèle des agents, chez leur fournisseur), `base_url` et `key` pour une autre API compatible OpenAI,
`price_in`/`price_out` (€ par million de tokens), `monthly_cap` (€, 0,10 par défaut) et `hour` (3 par défaut). Chaque
passage écrit `notebook pass` dans le journal avec ses tokens et son coût ; `GET /v1/notebook/runs` montre le mois.

Les travaux longs passent en tâche de fond, jamais dans la conversation. Un agent en lance une de lui-même seulement
quand la personne l'a demandé, dans la limite de `[jobs] direct_budget_eur` (€, 0,20 par défaut, compté aux prix de
`[notebook]`) ; au-delà, il propose la tâche et la personne la lance d'un geste. Un tour de conversation qui dépasse
`handoff_tool_calls` (15 par défaut) appels d'outils ou `handoff_seconds` (180 par défaut) reçoit une fois la consigne
de passer la suite en tâche de fond (`handoff = false` le coupe) ; le pont écrit
`long run: handing over to a background task (steer)` dans le journal. Jamais pendant qu'un sous-agent
(`delegate_task`) travaille pour ce tour, et ni ses appels ni son temps ne comptent. Un tour qui navigue avec le
navigateur visible de l'agent (un outil de navigateur parmi ses `handoff_browser_window` derniers appels, 5 par défaut)
se regarde en direct : seuls ses propres seuils comptent, `handoff_browser_tool_calls` et `handoff_browser_seconds`
(0 par défaut : jamais). Un tour qui a reçu la consigne et se termine quand même sans passer la main (ni vraie réponse)
voit sa tâche de fond lancée par le pont, avec la demande de la personne et ce qui a déjà été fait, dans la limite de
`direct_budget_eur` (`handoff_rescue = false` le coupe) ; si elle ne peut pas démarrer, elle est proposée
(« Démarrer » dans la conversation), et la conversation comme une notification le disent.

Dans une discussion entre les agents de la personne (« Discussion entre agents »), chaque agent répond dans son propre
Hermes, dans une conversation cachée propre à cette discussion. Qui répond : les agents nommés avec `@`, sinon ceux dont
le rôle correspond au message, sinon un appel à un petit modèle (`[rooms] router_model = false` le coupe ; le modèle de
`[notebook]`, au plus `router_daily_max` appels par jour, 300 par défaut), sinon l'agent qui a parlé en dernier. Un
agent qui en interpelle un autre avec `@` lui donne un tour de plus, jusqu'à `max_hops` (1 par défaut, 2 au plus) ; un
tour est arrêté après `turn_seconds` (300 par défaut), sauf s'il attend l'accord de la personne.

Le pont ne redémarre le Hermes d'un agent (et son navigateur) que s'il y a une raison, et seulement quand l'agent ne
fait rien (aucun tour, appel Live, tâche de fond, lecture de boîte mail, vérification de veilleur, ni routine prévue
dans les `routine_margin_minutes`, 30 par défaut) : il n'a pas répondu `unhealthy_pings` fois de suite (3 par défaut,
une toutes les 5 minutes), un autre composant l'a demandé, ou, à l'heure la plus calme de la nuit pour cet agent (entre
`window_start` 1 h et `window_end` 6 h, ou `hour`), son Hermes prend plus de `hermes_mb` (900) Mo ou `hermes_growth`
(2,5) fois sa taille après le démarrage, ou son navigateur plus de `chrome_mb` (1536) Mo. Au plus une fois par heure,
une fois par nuit pour la mémoire. `[agent_restart] enabled = false` le coupe ; il est coupé d'office sur un Hermes que
vous aviez avant BipAgents. Le dernier redémarrage et sa raison sont dans `GET /v1/status` (`last_restart`).

Les fiches (achats, abonnements, contrats, papiers, réservations) peuvent être remplies depuis une archive d'un agent :
`bipagents archive extract <agent> --archive <id> --sample 50` lit 50 documents probables avec le modèle de l'agent et
affiche ce qu'il ajouterait et l'estimation pour toute l'archive, sans rien écrire ; `--run --budget <€>` les remplit
ensuite (une tâche de fond, en pause au plafond), puis les nouveaux documents sont lus chaque nuit. `--model <id>`
choisit un autre modèle du fournisseur de l'agent pour ce passage (avec `--price-in`/`--price-out`, € par million de
jetons ; un modèle sur la machine est gratuit). `[archive_records]` : `models = [{id, label, price_in, price_out}]`
proposés dans l'app, `unknown_price_in`/`unknown_price_out` (3 € / 15 € par défaut, pour que le plafond tienne),
`sample_cap_eur` (1), `nightly_cap_eur` (0,05 par nuit et par archive). Le texte des documents part chez le fournisseur
du modèle.

## 5. Lancer au démarrage

**macOS (Mac mini)** : deux LaunchAgents.

```bash
cd ~/BipAgents
for f in pocket/launchd/io.github.bipagents.pocket.plist bridge/launchd/io.github.bipagents.bridge.plist; do
  sed "s#__HOME__#$HOME#g" "$f" > ~/Library/LaunchAgents/$(basename "$f")
  launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/$(basename "$f")
done
tail -f ~/Library/Logs/bipagents-*.log
```

Pour les arrêter : `launchctl bootout gui/$(id -u)/io.github.bipagents.pocket` (même chose pour `.bridge`).

**Linux (VPS, serveur)** : deux services systemd utilisateur.

```bash
mkdir -p ~/.config/systemd/user
cp ~/BipAgents/pocket/systemd/pocket-tts.service ~/.config/systemd/user/
cp ~/BipAgents/bridge/systemd/hermes-bridge.service ~/.config/systemd/user/   # lance ~/.local/bin/bipbridge
systemctl --user daemon-reload && systemctl --user enable --now pocket-tts hermes-bridge
sudo loginctl enable-linger $USER     # services actifs sans session ouverte
```

Mise à jour : `git pull`, `pip install -r requirements.txt` dans le venv de Pocket, le binaire du bridge de la
nouvelle clé (mêmes commandes que plus haut, quand `bridge-rs/` a changé), redémarrer les deux services. Si tu as
ajouté le navigateur des agents à la main (`browser/README.fr.md`), `bipagents browser-refresh` garde son Chrome à jour :
lance-le depuis un minuteur de nuit (`installer/bipinstall/browser_refresh.py` en écrit un : « Rester à jour » là-bas).

## 6. Ouvrir sur le tailnet

```bash
sudo tailscale serve --bg --https=8642 http://127.0.0.1:8642   # un par agent (8644, …)
sudo tailscale serve --bg --https=8643 http://127.0.0.1:8643   # le bridge
```

Depuis ton téléphone ou un autre appareil du tailnet : `https://mamachine.tailnet.ts.net:8643/health`. Pocket (8098)
reste local, seul le bridge l'appelle.

## 7. L'app

- Les apps (iPhone, iPad, Android) viennent de l'App Store et de Google Play *(bientôt : elles sont en test)*. En
  attendant, la console web ouvre tes agents dans le navigateur d'un ordinateur : `~/.local/bin/bipbridge console
  --config ~/.config/hermes-ios/bridge.toml` donne un lien à usage unique (sur le tailnet).
- Dans l'app : **＋ › Scanner le QR code**. Sur le serveur, renseigner `public_url` (adresses tailnet du bridge et de
  chaque agent) dans `bridge.toml`, puis afficher le code avec `~/.local/bin/bipbridge qr --config
  ~/.config/hermes-ios/bridge.toml` (un par agent, `--agent <id>` pour un seul, `--png <fichier>` pour l'enregistrer).
  Il contient les clés de l'agent : ne le montrer qu'à son propre téléphone. Choisir sa langue et son Bip.

**Sans Tailscale (optionnel)** : avec `[lan] enabled = true`, le bridge ouvre aussi une porte HTTPS sur le réseau local
(port 8650, à n'ouvrir que sur le réseau local) et le QR code contient son adresse et l'empreinte de son certificat.
Quand le tailnet ne répond pas, l'app passe par cette porte, en n'acceptant que ce certificat, et affiche « En ligne ·
réseau local ». Elle apprend seule une nouvelle adresse locale tant que le tailnet marche ; si l'adresse a changé
pendant une coupure, elle propose de scanner un nouveau QR code. Les notifications demandent toujours Internet (Apple).

**Notifications** (réponses écran verrouillé, accords, Boîte) : l'app de l'éditeur ne se joint par les services
d'Apple et de Google qu'avec les clés de l'éditeur, donc ton bridge les envoie par le
[relais de notifications](../relay/README.fr.md) de Kerfy Studio (`[push] service_url`), qui ne transmet aucun texte de
message ; l'installateur le règle dès que le relais est en ligne. Sans lui, tout le reste fonctionne, sans les
notifications.

## Bonus : ton propre modèle de voix (GPU)

Avec un GPU, un serveur de voix plus expressif peut parler à la place des Bips pour certains usages : le Live, les
réponses vocales, la lecture des messages. N'importe quel serveur doté du `POST /v1/audio/speech` d'OpenAI convient :
Fish Audio Drama 3 sur audio.cpp (recette dans `tts/drama3/`), Kokoro, Kyutai TTS 1.6B… Le lancer, puis dans le
bridge (ou `install.sh voice`, qui le trouve sur le réseau) :

```toml
[voice_server]
url = "http://127.0.0.1:8880"
live = true
replies = true
reading = true
```

Ses voix apparaissent dans l'app sous « Voix 1, Voix 2… » (agent › Voix). S'il ne répond pas, les Bips prennent le
relais. Rien d'autre n'en dépend : les Bips, les quatre langues, la Boîte et les notifications marchent sans.
