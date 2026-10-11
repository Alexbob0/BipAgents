[🇬🇧 English](README.md) · 🇫🇷 Français

# Le navigateur visible des agents

Chaque agent peut avoir son propre navigateur : un vrai Google Chrome (Chromium sur ARM, où Google n'en publie pas pour
Linux) sur un bureau à lui. Tu le regardes en direct dans l'app (« Bureau », derrière Face ID) et tu peux prendre la
souris et le clavier : pour passer un captcha, vérifier quelque chose ou finir une étape toi-même. C'est un navigateur
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
  `stealth`. Le serveur tourne dans le conteneur, à côté de Chrome ; Hermes le lance avec `podman exec`.
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
~/BipAgents/bridge/.venv/bin/python -m bipbridge browser add --agent <agent>   # ou --all ; `remove` pour retirer
systemctl --user restart bipagents-bridge
```

Le conteneur de chaque agent tourne en service utilisateur systemd, `bipagents-browser-<agent>`. Avec un GPU
(`/dev/dri`), Chrome s'en sert ; sinon, rendu logiciel. `[browser]` dans `bridge.toml` accepte aussi `language`
(ex. `fr-FR`), `timezone`, `gpu = "none"`, `first_port` (9600) et `runtime` (`podman` ou `docker`).

## Ce qui est à nous, ce qui est récupéré

Rien de stealth-browser-mcp n'est gardé dans ce dépôt. La construction de l'image le récupère sur son propre dépôt,
au commit testé avec BipAgents (`a7b9eea`, fixé dans le `Containerfile`), et applique notre correctif :

- `stealth.patch` : DevTools de Chrome sur un port fixe (`STEALTH_CDP_PORT`), pour que Hermes et le coffre pilotent
  le même navigateur ; les réglages du conteneur (profil, options, fuseau horaire) quand l'agent ne les donne pas ;
  le retrait d'options que le serveur ajoute dans les conteneurs et dont celui-ci se passe (`STEALTH_DROP_ARGS`).
- `requirements.txt` : ses paquets Python aux versions exactes testées, installés depuis PyPI. Parmi eux, **nodriver
  est sous GNU AGPL v3** et certifi sous MPL 2.0 : tous deux installés sans modification, comme paquets séparés.
- `entrypoint.sh` (l'écran, le serveur VNC), `devtools_relay.py` (les DevTools de Chrome depuis l'hôte, à une adresse
  qui survit aux redémarrages de Chrome), `mcp.sh` (le serveur avec seulement ce qu'il faut pour naviguer : ni copie
  d'éléments, ni extraction de fichiers, ni interception réseau, ni CDP brut, ni crochets de débogage) et la consigne
  qui explique à l'agent son navigateur (`bridge/bipbridge/skills/visible-browser/SKILL.md`, copiée dans son profil Hermes).

## Limites

- Linux seulement (l'écran est un serveur X dans le conteneur). Sur Mac, ça viendra avec un compte macOS séparé.
- Avec ses outils de page (`execute_script`), un agent peut lire ce qu'une page affiche, y compris un champ rempli
  par le coffre. Le coffre garde les mots de passe hors de la conversation et de la mémoire de l'agent, mais un agent
  qui veut un mot de passe peut regarder le champ. Ne donne un identifiant qu'aux agents à qui tu le confierais.
- Le port DevTools n'a pas de mot de passe : sur une machine partagée par plusieurs comptes, les programmes d'un autre
  compte pourraient s'y connecter sur 127.0.0.1 et piloter ce navigateur. Sur un serveur familial, ces comptes sont
  ceux de la famille.
