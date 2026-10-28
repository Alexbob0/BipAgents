[🇬🇧 English](README.md) · 🇫🇷 Français

# Le navigateur visible des agents

Chaque agent peut avoir son propre navigateur : un vrai Google Chrome (Chromium sur ARM, où Google n'en publie pas pour
Linux) sur un bureau à lui. Tu le regardes en direct dans l'app, dans la conversation même ou dans le « Bureau » de
l'agent, et tu peux prendre la souris et le clavier (« Prendre la main », derrière Face ID ou le verrouillage du
téléphone) : pour passer un captcha, vérifier quelque chose ou finir une étape toi-même. C'est un navigateur
qu'on voit et qu'on peut piloter, pas un moyen de contourner les règles des sites : un agent qui tombe sur un captcha
te le demande, il ne le résout jamais.

## Comment ça s'assemble

```
 app ──(Face ID)── bridge ──VNC, 127.0.0.1──┐
                                            │   conteneur bipagents-browser-<agent>
 Hermes (l'agent) ──MCP par podman exec─────┼── stealth-browser-mcp ── Chrome sur son propre écran
          └── coffre, DevTools 127.0.0.1────┘        (Xvfb, openbox, x11vnc)
```

- **Un conteneur par agent** : son profil Chrome (cookies, connexions) est un dossier à lui sur l'hôte, monté dans ce
  seul conteneur. Une page qu'il ouvre, `file://` compris, voit le conteneur, jamais l'hôte et ses clés.
- **Piloté par [stealth-browser-mcp](https://github.com/vibheksoni/stealth-browser-mcp)** (MIT), un serveur MCP basé
  sur nodriver : l'agent reçoit ses outils (`spawn_browser`, `navigate`, `click_element`, `type_text`…) sous le nom
  `stealth`. Le serveur tourne dans le conteneur, à côté de Chrome ; Hermes le lance avec `podman exec`. Chrome n'est
  jamais l'enfant du serveur : `spawn_browser` le lance à part (`open_browser.sh`) et s'y connecte, il survit donc au
  serveur quand Hermes le redémarre, et l'instance de l'agent garde le même identifiant (et sa page). `close_instance`
  ferme les pages et laisse Chrome ; seul `quit_browser` le quitte.
- **Le coffre** remplit les mots de passe dans ce même Chrome : Hermes joint son port DevTools (`browser.cdp_url`) et
  remplit le champ sur le site exact de l'identifiant, sans que l'agent voie jamais le mot de passe. Le gestionnaire
  de mots de passe et la saisie automatique de Chrome sont coupés (règles dans l'image) : c'est le coffre qui les garde.
- **Le bureau** : le serveur VNC du conteneur a son propre mot de passe ; le bridge s'y connecte et relaie l'écran
  vers l'app. Les ports DevTools et VNC n'écoutent que sur 127.0.0.1, jamais sur le tailnet.

## Mise en place

L'installateur le propose sous Linux quand podman (ou docker) est là : il construit l'image, puis le bridge donne à
chaque agent son conteneur (`[browser] enabled = true` dans `bridge.toml` ; les agents créés ensuite depuis l'app en
ont un aussi). À la main :

```bash
podman build -t bipagents-browser ~/BipAgents/browser
bipagents bridge browser add --agent <agent>   # ou --all ; `remove` pour retirer
systemctl --user restart bipagents-bridge
```

Le conteneur de chaque agent tourne en service utilisateur systemd, `bipagents-browser-<agent>`, seulement quand il
sert : il démarre au premier appel de l'agent à son navigateur (quelques secondes) ou quand tu ouvres son « Bureau », et
s'arrête après 10 minutes sans usage (personne ne regarde le bureau, aucun appel, rien de connecté à ses DevTools). Son
profil reste, connexions et cookies compris. Sur une machine à plusieurs agents, seuls les navigateurs en service
prennent de la mémoire. Hermes garde branché un petit serveur MCP à nous (`bip-browser-mcp`, à côté du bridge : `bridge-rs/src/browser_mcp.rs`), qui
répond avec les outils que le serveur du navigateur a listés la dernière fois et le démarre quand un outil est appelé.
`idle_minutes = 0` dans `[browser]` laisse tous les navigateurs tourner, comme avant.

Avec un GPU (`/dev/dri`), Chrome s'en sert ; sinon, rendu logiciel. `[browser]` dans `bridge.toml` accepte aussi
`idle_minutes` (10), `language` (ex. `fr-FR`), `timezone`, `gpu = "none"`, `first_port` (9600) et `runtime` (`podman`
ou `docker`).

## Rester à jour

Chrome reçoit des correctifs de sécurité toutes les une ou deux semaines, et une image garde le Chrome avec lequel elle
a été construite. Chaque installation qui a des navigateurs vérifie donc chaque nuit, entre 1 h et 3 h, à part des
mises à jour de BipAgents (un minuteur utilisateur, `bipagents-refresh-browser` ;
`installer/bipinstall/browser_refresh.py`) : dnf, dans un conteneur jetable de l'image, dit si Chrome (Chromium sur
ARM) a une version plus récente ou si les paquets de l'image ont des mises à jour de sécurité. Sinon, rien ne se passe.
Si oui, l'image est reconstruite à côté, avec les paquets du jour et stealth-browser-mcp à la même version épinglée ;
elle ne remplace l'actuelle qu'une fois son Chrome démarré. Un navigateur en service n'est jamais interrompu : il
démarre sur la nouvelle image la fois suivante (celui qui ne s'arrête jamais, `idle_minutes = 0`, est redémarré après
30 minutes sans usage), profil et connexions gardés. Une nuit ratée (pas de réseau, dépôt de Google en panne) garde
l'image actuelle et réessaie la suivante. Les installations d'un serveur passent chacune leur tour, en priorité la plus
basse, et les anciennes images sont supprimées. `journalctl --user -u bipagents-refresh-browser` dit ce qu'il a fait ;
`bipagents browser-refresh` le fait tout de suite (`--force` : même sans nouveauté ; `--version` : quel Chrome a le
navigateur).

## Ce qui est à nous, ce qui est récupéré

Rien de stealth-browser-mcp n'est gardé dans ce dépôt. La construction de l'image le récupère sur son propre dépôt,
au commit testé avec BipAgents (`a7b9eea`, fixé dans le `Containerfile`), et applique notre correctif :

- `stealth.patch` : DevTools de Chrome sur un port fixe (`STEALTH_CDP_PORT`), pour que Hermes et le coffre pilotent
  le même navigateur ; les réglages du conteneur (profil, options, fuseau horaire) quand l'agent ne les donne pas ;
  le retrait d'options que le serveur ajoute dans les conteneurs et dont celui-ci se passe (`STEALTH_DROP_ARGS`) ;
  un navigateur toujours affiché sur l'écran du conteneur, même si l'agent le demande invisible (`STEALTH_HEADFUL`).
- `requirements.txt` : ses paquets Python aux versions exactes testées, installés depuis PyPI. Parmi eux, **nodriver
  est sous GNU AGPL v3** et certifi sous MPL 2.0 : tous deux installés sans modification, comme paquets séparés.
- `entrypoint.sh` (l'écran, le serveur VNC), `devtools_relay.py` (les DevTools de Chrome depuis l'hôte, à une adresse
  qui survit aux redémarrages de Chrome), `open_browser.sh` (Chrome lancé sur l'écran quand la personne ouvre le
  navigateur depuis l'app, ou par le `spawn_browser` de l'agent, détaché, avec les options de nodriver), `mcp.sh` (le serveur avec seulement ce qu'il faut pour naviguer : ni copie
  d'éléments, ni extraction de fichiers, ni interception réseau, ni CDP brut, ni crochets de débogage) et la consigne
  qui explique à l'agent son navigateur (`bridge-rs/assets/skills/visible-browser/SKILL.md`, copiée dans son profil Hermes).

## Limites

- Linux seulement (l'écran est un serveur X dans le conteneur). Sur Mac, ça viendra avec un compte macOS séparé.
- Avec ses outils de page (`execute_script`), un agent peut lire ce qu'une page affiche, y compris un champ rempli
  par le coffre. Le coffre garde les mots de passe hors de la conversation et de la mémoire de l'agent, mais un agent
  qui veut un mot de passe peut regarder le champ. Ne donne un identifiant qu'aux agents à qui tu le confierais.
- Le port DevTools demande un secret propre au navigateur (`cdp.token` à côté de `vnc.pass`, monté en lecture seule
  dans le conteneur ; le pont le montre en en-tête `X-Bip-Cdp`, l'adresse `browser.cdp_url` de Hermes le porte en
  `/t/<secret>/`) et refuse toute requête portant un `Origin` (celle d'une page web) : ni un autre compte d'une
  machine partagée, ni une page ouverte dans le navigateur ne peuvent le piloter. Le port VNC a son propre mot de passe
  (`vnc.pass`). Les deux fichiers ne sont lisibles que par le compte (0600).
