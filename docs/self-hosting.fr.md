[🇬🇧 English](self-hosting.md) · 🇫🇷 Français

# Héberger BipAgents chez soi : Mac mini, VPS ou serveur Linux

BipAgents n'a besoin d'**aucun GPU** : les voix des Bips tournent sur CPU (Kyutai Pocket TTS). Une machine qui fait
déjà tourner [Hermes Agent](https://github.com/NousResearch/hermes-agent) suffit, qu'il s'agisse d'un Mac mini, d'un VPS
ou d'un serveur Linux à la maison.

```
iPhone (BipAgents) ──Tailscale──▶ ta machine
                                   ├─ Hermes Agent, api_server   :8642 (un port par agent : 8644, …)
                                   ├─ bridge (bridge/)           :8643  voix, fichiers, push, Boîte
                                   └─ Pocket TTS (pocket/)       :8098  voix des Bips (local, appelé par le bridge)
```

Rien n'est exposé sur Internet : tout passe par ton tailnet, en HTTPS avec un vrai certificat (`tailscale serve`).

## Quelle machine ?

| Machine | Ce qu'il faut | Remarques |
|---|---|---|
| **Mac mini** Apple Silicon | 16 Go conseillés | Mesuré sur M4 : premier son ≈ 50 ms, ≈ 6,5× le temps réel, ≈ 0,6 Go pour les 4 langues. Désactiver la mise en veille et activer l'ouverture de session automatique (les services sont des LaunchAgents). |
| **VPS** Linux | 4 vCPU (dont 2 pour Pocket), 8 Go | vCPU **dédiés** de préférence (les vCPU partagés ralentissent la voix). ARM (Ampere, Hetzner CAX) ou x86 avec `--quantize`. Hermes appelle son modèle de langage par API, il n'a pas besoin de GPU. Non mesuré : vérifier le premier son après installation. |
| **Serveur Linux** avec GPU Nvidia | | Tout ce qui précède, plus le bonus Kyutai 1.6B (voix humaine, podcast) ci-dessous. |

## 1. Tailscale

Installer Tailscale sur la machine et sur l'iPhone, dans le même tailnet. Dans la console d'administration :
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
.venv/bin/python pocket_server.py --voices ../voices --languages fr,en,es,de
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
  la même machine, l'iPhone passe par Tailscale. Avec `ufw` : `sudo ufw allow in on tailscale0`, rien d'autre.
- **Mesurer** une fois installé : `python3 pocket/bench.py` (premier son et vitesse). Visé : premier son sous
  ≈ 300 ms, au moins 1,5× le temps réel. Sinon : `--quantize` (x86), puis des vCPU dédiés.

## 4. Le bridge

```bash
cd ~/BipAgents/bridge
python3 -m venv .venv && .venv/bin/pip install -r requirements.txt    # Python ≥ 3.11 conseillé
mkdir -p ~/.config/hermes-ios && chmod 700 ~/.config/hermes-ios
cp bridge.example.toml ~/.config/hermes-ios/bridge.toml && chmod 600 ~/.config/hermes-ios/bridge.toml
.venv/bin/python -m bipbridge genkey      # → bridge_key (à saisir aussi dans l'app)
```

Dans `~/.config/hermes-ios/bridge.toml`, pour une machine sans GPU :

```toml
[kyutai]
enabled = false                 # pas de Kyutai 1.6B : les Bips font tout

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
planifiées de Hermes arrivent dans la Boîte sans elles (`[cron]`). Puis `.venv/bin/python -m bipbridge check`.

Toutes les options : [`bridge/README.md`](../bridge/README.fr.md).

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
cp ~/BipAgents/bridge/systemd/hermes-bridge.service ~/.config/systemd/user/   # adapter WorkingDirectory/ExecStart à ~/BipAgents/bridge
systemctl --user daemon-reload && systemctl --user enable --now pocket-tts hermes-bridge
sudo loginctl enable-linger $USER     # services actifs sans session ouverte
```

Mise à jour : `git pull`, `pip install -r requirements.txt` dans chaque venv, redémarrer les deux services.

## 6. Ouvrir sur le tailnet

```bash
sudo tailscale serve --bg --https=8642 http://127.0.0.1:8642   # un par agent (8644, …)
sudo tailscale serve --bg --https=8643 http://127.0.0.1:8643   # le bridge
```

Depuis l'iPhone ou un autre appareil du tailnet : `https://mamachine.tailnet.ts.net:8643/health`. Pocket (8098)
reste local, seul le bridge l'appelle.

## 7. L'app

- Compiler avec Xcode sur un iPhone : `echo 'DEVELOPMENT_TEAM = <ton Team ID>' > Config/Local.xcconfig`, puis lancer
  le schéma BipAgents.
- Sous ton propre compte Apple, choisis ton identifiant d'app (il doit être unique chez Apple) et ajoute-le à
  `Config/Local.xcconfig` :

  ```
  DEVELOPMENT_TEAM = <ton Team ID>
  BIP_BUNDLE_ID = com.tonnom.bipagents
  ```

  L'extension (`….NotificationService`), l'App Group (`group.…`) et le groupe de trousseau (`….shared`) en découlent ;
  Xcode les crée au premier build (*Automatically manage signing*). Reporter le même identifiant dans
  `[apns] bundle_id` du bridge.
- Dans l'app : Réglages › Ajouter un agent → **Scanner le QR code**. Sur le serveur, renseigner `public_url` (adresses
  tailnet du bridge et de chaque agent) dans `bridge.toml`, puis afficher le code avec `python -m bipbridge qr` (un par
  agent, `--agent vie` pour un seul, `--png vie.png` pour l'enregistrer). Il contient les clés de l'agent : ne le
  montrer qu'à son propre téléphone. Sinon, à la main : adresse `https://mamachine.tailnet.ts.net:8642`, clé
  api_server, adresse du bridge `https://mamachine.tailnet.ts.net:8643` et sa clé. Choisir sa langue et son Bip.

**Sans Tailscale (optionnel)** : avec `[lan] enabled = true`, le bridge ouvre aussi une porte HTTPS sur le réseau local
(port 8650, à n'ouvrir que sur le réseau local) et le QR code contient son adresse et l'empreinte de son certificat.
Quand le tailnet ne répond pas, l'app passe par cette porte, en n'acceptant que ce certificat, et affiche « En ligne ·
réseau local ». Elle apprend seule une nouvelle adresse locale tant que le tailnet marche ; si l'adresse a changé
pendant une coupure, elle propose de scanner un nouveau QR code. Les notifications demandent toujours Internet (Apple).

**Notifications** (réponses écran verrouillé, accords, Boîte) : elles passent par Apple et demandent un compte
Apple Developer payant. Créer une clé APNs (.p8) et remplir `[apns]`, cf. [`bridge/README.md`](../bridge/README.fr.md#apns).
Sans elle, tout le reste fonctionne, sans les notifications.

## Bonus : la voix humaine et le podcast (Kyutai 1.6B, GPU)

Sur une machine avec un GPU Nvidia, le serveur [Kyutai TTS 1.6B](https://github.com/kyutai-labs/delayed-streams-modeling)
ajoute une voix humaine posée (« Voix classique » dans l'app, en français) et sert au podcast du matin (une skill
Hermes à part, hors de ce dépôt). Le lancer sur `:8097`, puis dans le bridge :

```toml
[kyutai]
enabled = true
url = "http://127.0.0.1:8097"
default_voice = "5476"
```

En français, si Pocket tombe en panne, le bridge se rabat alors sur cette voix. Rien d'autre n'en dépend : les
Bips, les quatre langues, la Boîte et les notifications marchent sans.
