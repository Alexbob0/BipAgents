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
   | Costaud | Serveur Linux ≥ 64 Go | ailleurs | Pocket + Kyutai (GPU Nvidia) | permanent, un par personne |

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

## Plusieurs personnes sur une machine

Chacun lance l'installateur dans **son propre compte** : ses agents, sa mémoire, ses fichiers et ses sessions de
navigateur, séparés par le système. Les voix et le modèle sont partagés. Chaque installation prend un bloc de dix
ports (8640 pour la première, puis 9200, 9210…).

Les notifications passent par Apple avec la clé de l'administrateur, qui reste dans son installation : son bridge
relaie celles des autres (une clé par installation).

L'administrateur prépare la place de chacun, depuis **son** compte :

```bash
bash ~/BipAgents/install.sh famille ajouter Léa
```

Sous Linux, ça prépare d'abord son compte (`lea`, ou celui donné par `--compte`), en affichant chaque commande `sudo`
avant de la lancer : le compte s'il n'existe pas, ses plages d'identifiants pour les conteneurs sans droits admin (le
navigateur des agents), et « linger », pour que ses services tournent quand elle n'est pas connectée. `--dry-run`
montre ces commandes sans rien changer.

Ensuite ça crée sa clé de relais, réserve son bloc de ports, et propose de lancer son installation tout de suite dans
**son** compte, depuis ton terminal : elle n'a rien à taper, les questions s'affichent chez toi. Sinon ça affiche la
commande pour plus tard. Enfin, les commandes `tailscale serve` à lancer une fois son installation finie. Redémarre
ton bridge, pour qu'il connaisse la nouvelle clé.

Sous macOS, crée son compte dans Réglages Système ; elle colle la ligne affichée dans **sa** session. Les services
tournent tant que le compte est ouvert (ouverture de session automatique sur un Mac mini qui sert de serveur).

## Fichiers

- `install.sh` (racine du dépôt) : récupère le dépôt et Python (uv), puis lance l'installateur.
- `bipinstall/` : `detect.py` (la machine), `models.py` (trouver et tester le modèle), `plan.py` (le profil et sa
  mémoire), `ports.py`, `hermes.py` (Hermes par ses propres commandes), `stack.py` (bridge, voix, services),
  genres d'agents dans `bridge/bipbridge/agent_templates.yaml` (partagés avec le bridge).
- Tests : `python -m pytest installer/tests` (sans réseau, rien d'installé).
