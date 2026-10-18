[🇬🇧 English](README.md) · 🇫🇷 Français

# L'installateur BipAgents

Une commande, lancée dans le compte de la personne à qui sont destinés les agents, sans droits d'administrateur :

```bash
curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash
```

Ajouter `--dry-run` pour voir chaque étape et chaque commande sans rien modifier.

## Ce qu'il fait

1. **Regarde la machine** : système, mémoire, processeur, GPU, Tailscale.
2. **Demande où tourne le modèle**, le plus souvent *pas* sur cette machine :
   - une autre machine du réseau (un PC avec GPU, un Spark, un Mac) : il trouve tout seul les serveurs compatibles
     OpenAI (Ollama, LM Studio, vLLM, llama.cpp…) et liste leurs modèles ;
   - une API dans le cloud (OpenRouter, OpenAI, Anthropic, Mistral) avec ta clé ;
   - cette machine, quand elle a la place.

   Il vérifie que le modèle répond, et s'il lit les images (les photos lui sont alors envoyées telles quelles).
3. **Choisit une installation adaptée** :

   | Profil | Machine type | Modèle | Voix | Navigateur des agents |
   |---|---|---|---|---|
   | Léger (par défaut) | Mac mini 16 Go | ailleurs (réseau ou API) | Pocket (processeur) | à la demande, un à la fois |
   | Local | Mac 24-32 Go, PC avec GPU | sur cette machine | Pocket | à la demande |
   | Costaud | Serveur Linux ≥ 64 Go | ailleurs | Pocket + ton modèle de voix (s'il tourne déjà) | permanent, un par personne |

   Sur un Mac mini 16 Go avec le modèle ailleurs : environ 1 Go par personne plus les voix, une famille y tient.
4. **Installe Hermes** avec son installateur officiel, dans la version testée avec BipAgents (`517b5e10f`), y applique
   les patches de `hermes/patches/` (questions de l'agent dans l'app, photos gardées pour les questions de suivi), en **une gateway par personne** (mode multiplex d'Hermes) :
   chaque agent est un profil joint à `/p/<agent>/` avec sa propre clé. Les nouveaux agents sont pris en compte sans
   redémarrage.
5. **Crée les premiers agents**, que la personne nomme elle-même, à partir de genres (Quotidien, Bien-être, Finances, Maison, Travail, Apprendre,
   Créatif, Tech) ou « Autre » sur mesure (son rôle, son ton, ses règles, sa langue et sa couleur, décrits par la personne), chacun avec une personnalité de départ (`SOUL.md`) et inscrit au Bot Mode pour qu'ils se parlent.
6. **Met en place les voix** (Pocket TTS, un serveur par machine, partagé par tous) et **le bridge** (sa config dans
   `~/.config/bipagents/bridge.toml`, clés générées, sa porte sur le réseau local), en services qui démarrent seuls :
   launchd sur macOS, services utilisateur systemd sur Linux.
7. **Donne à chaque agent son navigateur visible** (Linux avec podman, si tu le veux) : un vrai Chrome sur un bureau à
   lui, que tu regardes en direct et reprends en main depuis l'app (Bureau, derrière Face ID), et dans lequel le coffre
   remplit les mots de passe sans que l'agent les voie. Voir [`browser/`](../browser/README.md).
8. **Publie l'installation sur le tailnet** (`tailscale serve`, jamais sur Internet). S'il n'en a pas le droit (le
   compte d'une autre personne sous Linux), il affiche les commandes pour l'administrateur.
9. **Affiche un seul QR code** : scanné dans l'app, il ajoute tous les agents de l'installation.

## Sur un serveur loué (VPS)

L'installateur reconnaît un serveur loué (Hetzner, OVH, Scaleway, AWS…) : pas de porte sur un réseau local, et le
modèle dans le cloud proposé en premier. Le serveur se prépare d'avance avec un fichier cloud-init : voir
[BipAgents sur un serveur loué](../docs/vps.fr.md).

## Sur Windows

Windows 11 (22H2 ou plus récent), dans PowerShell ouvert en administrateur :

```powershell
irm https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.ps1 | iex
```

Les agents vivent dans **WSL2** (Ubuntu), installés exactement comme sur un serveur Linux — bacs à sable, voix et
navigateurs compris. `install.ps1` prépare ce dont Windows a besoin, chaque étape seulement si nécessaire, puis y lance
cet installateur :

- WSL2 et Ubuntu (Ubuntu demande un nom d'utilisateur et un mot de passe Linux la première fois ; Windows peut
  demander un redémarrage : relance la commande ensuite) ;
- le réseau de WSL est le **miroir** de celui de Windows (`networkingMode=mirrored` dans `%USERPROFILE%\.wslconfig`) :
  le réseau local atteint la porte du bridge, et un serveur de modèle qui tourne sous Windows (LM Studio, Ollama…) est
  trouvé sur `127.0.0.1` ;
- dans Ubuntu : systemd, git, curl, podman, et le « linger » ;
- le **Tailscale de Windows**, connecté (installé avec winget si tu es d'accord) : il publie les agents sur ton
  tailnet — l'installateur le pilote depuis WSL ;
- une **tâche « BipAgents » à l'ouverture de session** qui garde WSL éveillé (Windows l'arrête dès qu'aucune fenêtre
  ne s'en sert).

Garde le PC allumé et éveillé (Paramètres › Système › Alimentation : jamais de veille sur secteur) ; les agents
démarrent quand tu ouvres ta session Windows. Mise à jour : `wsl -d Ubuntu-24.04 -- bash -lc 'bipagents update'`.
Une autre distribution : `$env:BIPAGENTS_DISTRO = '…'` avant la commande. Enlever : désinstalle BipAgents dans Ubuntu
(plus bas), puis `Unregister-ScheduledTask BipAgents` (et `wsl --unregister Ubuntu-24.04` pour supprimer Ubuntu avec
tout ce qu'il contient).

Un Hermes installé sous Windows même (l'assistant d'agents du RTX Spark, l'`install.ps1` de Hermes) n'est pas
utilisé : ses agents tournent sans bac à sable, et le bridge ne tourne pas encore sous Windows. Il reste tel quel, à
côté de BipAgents.

## Si tu as déjà Hermes

L'installateur le voit et s'y branche **sans toucher à tes réglages** : pas de question sur le modèle, et ni ton
modèle, ni tes outils, ni tes plateformes (Telegram…), ni tes agents ne changent. Il te demande quels agents (profils)
afficher dans l'app, ton profil principal compris, et te montre avant de commencer tout ce qu'il va changer :

- l'API locale d'Hermes, si elle est coupée : activée sur 127.0.0.1, sur un port de ton bloc, avec une nouvelle clé ;
- une clé pour chaque agent affiché qui n'en a pas ;
- les correctifs BipAgents dans le code d'Hermes, si tu les veux et si ta version est celle testée ;
- un service pour ta gateway si elle ne tourne pas, ou un redémarrage si elle tourne déjà en service (à la main,
  tu la redémarres toi-même).

Tout ça est noté dans `~/.config/bipagents/existing-hermes.json` : la désinstallation remet chaque réglage comme
avant et retire les correctifs. `--all` ne supprime jamais un Hermes qui était là avant BipAgents. Si les outils de
tes agents ne tournent pas dans un bac à sable, le bridge ne garde pas de mots de passe (pas de coffre) : il te le
dit, sans changer leurs outils. Il faut une version d'Hermes qui sert plusieurs agents à la même adresse
(`hermes update` sinon).

## Mettre BipAgents à jour

`bipagents update` (ou `curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash -s --
update`, comme `bash ~/BipAgents/install.sh update`) met à jour l'installation de ce compte. `bipagents` seul liste ses
commandes (update, voice, hermes-update, family add, export, resume, uninstall) ; l'installateur le met dans `~/.local/bin`. Il montre ce qui est nouveau, relance
seulement ce qui a changé (le bridge, les voix des Bips quand c'est ce compte qui les fait tourner, les navigateurs des
agents quand leur image a changé : reconstruits, profils gardés), et applique les réglages qu'une nouvelle
installation pose maintenant (`installer/bipinstall/migrations.py` : par exemple les commandes à risque qui te demandent
toujours ton accord). Il propose ensuite de mettre à jour aussi les autres
installations de la machine (celles de la famille), chacune dans son compte, après ton mot de passe sudo, demandé une
seule fois (`--all` : sans demander) ; lancé depuis un compte administrateur sans installation à lui, il met à jour les
voix des Bips de ce compte et les autres. Ce qui ne peut pas se faire ainsi arrive sous forme de commandes à lancer (la
mise à jour de Hermes). Des modifications locales dans `~/BipAgents` qui la bloquent sont montrées avec les commandes pour les mettre
de côté.

La première fois, il propose de le faire chaque nuit à 4 h 30 (un minuteur de ce compte, `install.sh update --auto` :
sans questions ; `systemctl --user disable --now bipagents-update.timer` l'arrête). `--check` dit seulement ce qui est
nouveau. Un Hermes que tu avais avant BipAgents garde ses réglages, sauf avec `--apply-to-my-hermes`.

## Déménager sur une autre machine

D'un serveur loué vers un ordinateur à la maison, d'un hébergeur vers un moins cher, ou l'inverse : tout suit. Le
caractère, la mémoire, les compétences, les conversations, les routines et les fichiers de tes agents. Les réglages de
Hermes et la clé de ton modèle. Les clés des téléphones et les notifications. Le coffre, les données partagées, les liens
de la famille et les connexions des navigateurs.

1. Sur l'ancienne machine : `bipagents export --moving`. Il écrit un seul fichier chiffré
   (`~/bipagents-<machine>-<date>.bipagents`), affiche une fois sa phrase secrète, puis arrête l'installation à cet
   endroit, pour qu'aucune routine ne tourne en double. `bipagents resume` la relance si tu changes d'avis.
2. Pour que tes téléphones n'aient rien à faire : dans l'admin de Tailscale, renomme l'ancienne machine (`aibox-old`)
   et donne son nom à la nouvelle. Même nom et mêmes ports : l'app retrouve ses agents toute seule. Sinon, tu scannes un
   nouveau QR code à la fin.
3. Copie le fichier sur la nouvelle machine (`scp`), puis lance dessus, dans le compte qui gardera les agents :
   `curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash -s -- import <fichier>`. Il
   demande la phrase secrète et installe BipAgents et Hermes dans les mêmes versions, pour le processeur de cette
   machine. Il remet tout en place en ajustant les chemins, les ports, le nom sur le tailnet et le réseau local,
   démarre tout et vérifie que les agents répondent.
4. Une fois que tout marche sur la nouvelle machine, `bipagents uninstall --all` sur l'ancienne efface l'ancienne copie.

Le fichier va d'une machine à l'autre comme tu le copies : rien ne passe par quelqu'un d'autre. Qui a le fichier **et**
sa phrase secrète peut faire tourner tes agents : garde-les séparés. Sans `--moving`, `bipagents export` est une
sauvegarde : l'installation continue de tourner. Ne sont pas copiés : le code et les environnements de Hermes
(réinstallés), les caches, les journaux et les caches des navigateurs.

Ce qui peut te demander quelque chose après :
- **Un modèle sur ton réseau local.** S'il n'est pas joignable depuis la nouvelle machine, c'est signalé à la fin : choisis-en un autre dans
  l'app.
- **Des sites qui redemandent une connexion.** Certains le font depuis une nouvelle adresse.
- **La perte du bac à sable.** Vers un Mac, ou un Linux sans podman, les agents n'ont plus de terminal, d'accès aux
  fichiers, de coffre ni de navigateur tant qu'il n'y en a pas.

## Mettre Hermes à jour

Quand tu veux : `bash ~/BipAgents/install.sh hermes-update`. Avant de toucher à quoi que ce soit, il dit quels
correctifs BipAgents s'appliquent encore sur la nouvelle version (vérifié sur une copie). Puis il retire nos
correctifs, lance `hermes update` (sa sauvegarde, ses dépendances), remet les correctifs qui s'appliquent,
redémarre les agents (le hub et leurs espaces) et vérifie que chacun répond. Si un agent ne répond plus, il propose
de revenir à la version précédente. `--check` vérifie sans rien changer.

## Désinstaller

`python -m bipinstall uninstall` (depuis `~/BipAgents`, avec `PYTHONPATH=installer`) retire les services, la publication
sur le tailnet et la config du bridge, en gardant les données des agents dans `~/.hermes`. `--all` supprime aussi
Hermes et ces données, après confirmation. `--dry-run` montre ce qui serait retiré.

## Les voix : les Bips, et ton propre modèle de voix

Les voix des Bips (Kyutai Pocket TTS, sur le processeur, en français, anglais, espagnol et allemand) sont toujours
installées : elles sont légères et répondent presque instantanément. **Ton propre modèle de voix** peut parler à leur
place pour certains usages : tout serveur qui tourne déjà sur la machine ou le réseau local avec le
`POST /v1/audio/speech` d'OpenAI — Fish Audio Drama 3 sur audio.cpp (`tts/drama3/`), Kokoro, Kyutai… Rien n'est
installé pour lui : l'installateur en cherche un (port 8099 d'audio.cpp, 8880 de Kokoro, 8097 de Kyutai) et demande
pour quoi il parle — le **Live**, les **réponses vocales** à tes notes vocales, la **lecture** des messages et des
rapports. Les Bips parlent pour le reste, et prennent le relais quand il ne répond pas.

- `--voice-server http://192.168.1.20:8880` donne son adresse ; `--voice-uses live,replies` ce pour quoi il parle.
- `bash ~/BipAgents/install.sh voice` en branche un plus tard (ou le change, `--off` pour revenir aux Bips), dans
  chaque compte dont le bridge doit s'en servir. Dans l'app : chaque agent › Voix › « Ton modèle de voix » (« Voix 1,
  Voix 2… »), et Réglages › Voix › « Ton modèle de voix » pour ce pour quoi il parle.

## Plusieurs personnes sur une machine

**Une installation par personne, chacune dans son propre compte.** Chacun a son bridge, son Hermes, ses agents, sa
mémoire, ses fichiers et ses sessions de navigateur, séparés par le système : les agents de l'un ne peuvent pas lire
ceux de l'autre. Toutes les installations tournent en même temps sur la même machine, chacune sur son bloc de dix
ports (8640 pour la première, puis 9200, 9210…).

Ce qui est partagé : le modèle, les voix (Pocket, et un modèle de voix qui tourne déjà sur la machine ou le réseau, voir
plus haut) et les notifications : elles passent par Apple avec la
clé de l'administrateur, qui ne quitte jamais son installation ; son bridge relaie celles des autres (une clé de
relais par installation).

### Qui lance quoi, depuis quel compte

| | Qui | Quel compte | Quoi |
|---|---|---|---|
| Une fois | L'administrateur | Le sien (avec `sudo`) | `bash ~/BipAgents/install.sh` — sa propre installation, comme d'habitude |
| Pour chaque personne | L'administrateur | **Le sien** (avec `sudo`), jamais celui de la personne | `bash ~/BipAgents/install.sh family add Léa` |
| La personne | — | — | Rien sur le serveur : Tailscale dans ton tailnet, l'app, son QR code |

### `family add`, étape par étape (Linux)

À lancer **dans le terminal de l'administrateur**. La commande affiche chaque étape `[n/5]` :

1. **Son compte Linux** (`lea`, ou celui donné par `--account`) : créé s'il n'existe pas, avec ses plages
   d'identifiants pour les conteneurs sans droits admin (le navigateur des agents) et « linger » (ses services
   continuent quand elle n'est pas connectée). Chaque commande `sudo` est affichée et confirmée avant ; rien n'est
   demandé si le compte est déjà prêt. `--dry-run` ne fait qu'afficher ces commandes.
2. **Redémarre ton bridge**, pour qu'il connaisse sa clé de relais : la commande propose de le faire (`y`), ou
   affiche la commande à lancer dans un autre terminal de **ton** compte, avant l'étape 3 — en général
   `systemctl --user restart bipagents-bridge`.
3. **Son installation, dans son compte** : pas besoin de te connecter à son compte (pas de `ssh lea@…`). Réponds
   `y` et elle s'y lance par `sudo`, ses questions s'affichant **dans ton terminal** (le modèle, ses premiers agents,
   le navigateur visible). Elle n'a rien à taper. Elle se termine par son **QR code**. Réponds `N` pour obtenir la
   commande et la lancer plus tard.
4. **Publier ses ports sur le tailnet** : la commande propose de lancer les `sudo tailscale serve` (`y`), ou les
   affiche.
5. **Son iPhone, dans TON tailnet** : celui où est le serveur. Un compte Tailscale à elle crée un tailnet à elle, où
   l'app ne trouve pas le serveur. Donc :
   1. invite-la dans la [console Tailscale](https://login.tailscale.com/admin/users) › Invite users (son e-mail) ;
   2. sur son iPhone, elle accepte l'invitation, puis dans l'app Tailscale : icône de profil › Add account › connexion
      avec l'e-mail invité › choisir ton tailnet ;
   3. une fois le serveur listé dans l'app Tailscale : l'app BipAgents (TestFlight ou App Store) › « + » › Scanner le
      QR code.

   Plus simple, mais son iPhone devient alors un de tes appareils : connecter Tailscale avec ton propre compte. Le QR
   code à nouveau :
   `sudo -iu lea bash -c 'cd ~/BipAgents/bridge && .venv/bin/python -m bipbridge qr --config ~/.config/bipagents/bridge.toml'`.

L'installateur parle anglais. Les agents répondent dans la langue qu'on leur écrit ; celle dans laquelle ils
écrivent d'eux-mêmes (routines, comptes rendus) se règle par agent dans l'app, celle de l'iPhone par défaut.

Sous macOS, crée son compte dans Réglages Système ; elle colle la ligne affichée dans **sa propre** session. Les
services tournent tant que le compte est ouvert (ouverture de session automatique sur un Mac mini qui sert de serveur).

## Fichiers

- `install.sh` (racine du dépôt) : récupère le dépôt et Python (uv), puis lance l'installateur.
- `bipinstall/` : `detect.py` (la machine), `models.py` (trouver et tester le modèle), `plan.py` (le profil et sa
  mémoire), `ports.py`, `hermes.py` (Hermes par ses propres commandes), `stack.py` (bridge, voix, services),
  genres d'agents dans `bridge/bipbridge/agent_templates.yaml` (partagés avec le bridge).
- Tests : `python -m pytest installer/tests` (sans réseau, rien d'installé).
