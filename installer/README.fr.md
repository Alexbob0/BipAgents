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
4. **Installe Hermes** avec son installateur officiel, en **une gateway par personne** (mode multiplex d'Hermes) :
   chaque agent est un profil joint à `/p/<agent>/` avec sa propre clé. Les nouveaux agents sont pris en compte sans
   redémarrage.
5. **Crée les premiers agents**, que la personne nomme elle-même, à partir de genres (Quotidien, Bien-être, Finances, Maison, Travail, Apprendre,
   Créatif, Tech), chacun avec une personnalité de départ (`SOUL.md`) et inscrit au Bot Mode pour qu'ils se parlent.
6. **Met en place les voix** (Pocket TTS, un serveur par machine, partagé par tous) et **le bridge** (sa config dans
   `~/.config/bipagents/bridge.toml`, clés générées, sa porte sur le réseau local), en services qui démarrent seuls :
   launchd sur macOS, services utilisateur systemd sur Linux.
7. **Publie l'installation sur le tailnet** (`tailscale serve`, jamais sur Internet). S'il n'en a pas le droit (le
   compte d'une autre personne sous Linux), il affiche les commandes pour l'administrateur.
8. **Affiche un seul QR code** : scanné dans l'app, il ajoute tous les agents de l'installation.

## Plusieurs personnes sur une machine

Chacun lance l'installateur dans **son propre compte** : ses agents, sa mémoire, ses fichiers et ses sessions de
navigateur, séparés par le système. Les voix et le modèle sont partagés. Chaque installation prend un bloc de dix
ports (8640 pour la première, puis 9200, 9210…).

Les notifications passent par Apple avec la clé de l'administrateur, qui reste dans son installation : son bridge
relaie celles des autres (`[relay]` dans son `bridge.toml`, une clé par installation), et les autres installent avec
`--relay-url https://<serveur>:8643 --relay-key <leur clé>`.

Sous Linux, pour que les services d'un compte tournent quand la personne n'est pas connectée, l'administrateur
l'active une fois : `sudo loginctl enable-linger <utilisateur>`. Sous macOS, les services tournent tant que le
compte est ouvert (ouverture de session automatique sur un Mac mini qui sert de serveur).

## Fichiers

- `install.sh` (racine du dépôt) : récupère le dépôt et Python (uv), puis lance l'installateur.
- `bipinstall/` : `detect.py` (la machine), `models.py` (trouver et tester le modèle), `plan.py` (le profil et sa
  mémoire), `ports.py`, `hermes.py` (Hermes par ses propres commandes), `stack.py` (bridge, voix, services),
  `templates.yaml` (modèles d'agents).
- Tests : `python -m pytest installer/tests` (sans réseau, rien d'installé).
