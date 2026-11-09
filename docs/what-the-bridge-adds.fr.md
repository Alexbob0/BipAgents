[🇬🇧 English](what-the-bridge-adds.md) · 🇫🇷 Français · [🇩🇪 Deutsch](what-the-bridge-adds.de.md) · [🇪🇸 Español](what-the-bridge-adds.es.md) · [🇳🇱 Nederlands](what-the-bridge-adds.nl.md)

# Ce que le bridge ajoute à Hermes Agent

BipAgents est **construit sur [Hermes Agent](https://github.com/NousResearch/hermes-agent)** de Nous Research. Hermes
fait la réflexion : la boucle du modèle, les outils, la mémoire, les compétences, les routines, les approbations, ses
passerelles et son API Runs. Hermes est pensé pour un terminal et des plateformes de discussion, et chaque client
Hermes (la CLI, l'app de bureau officielle, la passerelle Telegram, d'autres apps) montre ce que fait Hermes.
BipAgents vise autre chose : **plusieurs agents par personne, utilisés depuis un téléphone par des gens qui n'ouvrent
jamais de terminal, assez sûrs pour tenir leur courrier, leur agenda et leurs cartes, et assez économes pour être
vendus en abonnement mensuel.** Il faut pour cela une couche entre les apps et Hermes : le **bridge**
([`bridge-rs/`](../bridge-rs/), un seul programme Rust, `bipbridge`). Cette page liste ce qu'il ajoute, domaine par
domaine, avec les modules (`bridge-rs/src/<nom>.rs`) qui le font, et ce qu'il laisse volontairement à Hermes.

## En un coup d'œil

| Domaine | Hermes seul | Hermes + autres clients | Bridge BipAgents |
|---|---|---|---|
| Lui parler | terminal, passerelles de discussion, API Runs | une fenêtre de discussion (app de bureau, Telegram…) | apps iPhone, iPad et Android, notes vocales, appels Live mains libres, console web |
| Voix | aucune | aucune, ou celle de la plateforme | une voix par agent, phrase par phrase pendant la réponse, texte préparé pour la parole en 5 langues |
| Hors de l'app | les messages de la passerelle | les notifications de la plateforme | notifications dont le texte vient de ton serveur, approbations et questions depuis l'écran verrouillé, Live Activities |
| Plusieurs agents | profils, réglés à la main | souvent un agent à la fois | agents créés en 2 s depuis l'app, chacun avec son bac à sable, son navigateur, ses outils, sa voix et la langue de sa mémoire |
| Navigation | un outil navigateur | idem | le vrai Chrome de chaque agent, regardé en direct et repris en main depuis le téléphone |
| Sécurité | approbations, `approvals.deny`, bac à sable docker | idem | approbations toujours demandées et dites simplement, bacs à sable sans tes dossiers, envois vers des sites inconnus plafonnés, échanges entre agents limités, coffre en écriture seule, frein anti-boucle |
| Mémoire | fichiers de mémoire, compétences | idem | un Carnet lisible, éditeur de mémoire, mémoire commune, discussions de groupe, agents de la famille |
| Tes données | ce que les outils atteignent | idem | boîtes mail, agendas, contacts, rappels, santé, imports de notes et WhatsApp, connecteurs avec OAuth |
| Résultats | texte et fichiers | texte et fichiers | pages verrouillées hors ligne, cartes, podcasts, fichiers remis où que l'agent les ait écrits |
| Travail en autonomie | tâches cron | idem | tâches de fond avec budget, veilleurs, réponse de chaque routine dans l'app, une liste de tout ce qui tourne seul |
| Coût | ce que coûte le modèle | idem | régime de jetons, fournisseur fixé pour le cache, consommation par appel, crédit, budgets, mise en veille |
| Installer et garder | installation, puis réglages à la main | idem | une commande, mises à jour, sauvegardes, restaurations, déménagements entre serveurs |
| Offre hébergée | — | — | un espace par personne sur les serveurs de Kerfy Studio : son certificat, une porte sur le port 443, crédit et sauvegardes |

## Le téléphone d'abord

- **Voix.** `live_calls.rs` mène les appels Live ; `textproc.rs` découpe le
  markdown en phrases ; `tts.rs` ordonne les moteurs (Pocket TTS sur CPU, ou le serveur de voix de la
  personne) ; `voices.rs` donne sa voix à chaque agent. Le texte est préparé pour la parole : `numbers_fr.rs`,
  `speech_intl.rs` (anglais, espagnol, allemand, néerlandais), `durations.rs`, `pronounce.rs`, `pronunciations.rs`.
- **Appels Live.** `live_brief.rs` démarre un appel depuis un court résumé de la conversation au lieu de 100k jetons
  d'historique ; `live_calls.rs` remet ce que l'appel a fait dans la conversation.
- **Notifications.** `push.rs` (APNs, `apns.rs` ; Android, `fcm.rs`) envoie une alerte générique et le téléphone va
  chercher le texte auprès du bridge : Apple et Google ne le voient jamais ; `relay.rs` et le [`relay/`](../relay/)
  séparé transmettent les notifications sans contenu. `runs.rs` transforme une approbation que personne ne regarde en
  notification ; `agent_notify.rs` règle cela par agent ; `live_activities.rs` affiche un long travail sur l'écran
  verrouillé et la Dynamic Island.
- **Messages pendant le travail.** `msg_queue.rs` : un message envoyé pendant que l'agent travaille entre dans le tour en
  cours (le steer de Hermes, après ses appels d'outils du moment) ; « Après » le garde plutôt dans la file de l'agent, sur le
  pont, la même sur tous les appareils, lancée comme tour suivant à la fin du travail (ordre, retrait, envoi immédiat,
  pause), une seule notification à la fin.
- **Rien de perdu.** `outbox.rs` et `cronwatch.rs` mettent les messages des agents et la réponse de chaque routine dans
  une boîte d'envoi (`store.rs`), audio prêt.
- **Joindre la maison.** Par Tailscale, ou `lan.rs` : une porte HTTPS épinglée sur le réseau local quand Internet est
  coupé. `device_keys.rs` donne une clé à chaque appareil ; `guard.rs` compte les mauvaises clés ; `console.rs` est la
  console web.
- **Des mots simples.** `approval_text.rs`, `approval_command.rs`, `no_reply.rs`, `names.rs`, `answer_style.rs` et
  `welcome.rs` traduisent les événements techniques de Hermes en phrases courtes dans la langue de la personne.

## Plusieurs agents par personne

- `agents_admin.rs` crée, modifie et supprime les agents depuis l'app ; `agent_pool.rs` garde des profils de réserve
  pour qu'un nouvel agent soit prêt en 2 secondes environ ; `souls.rs` donne à chaque type d'agent sa personnalité de
  départ ; `guide.rs` et `guide_team.rs` proposent les premiers agents ou toute une équipe en une carte ;
  `agent_groups.rs` les range (Perso, Pro…).
- `spaces.rs` installe un agent dans son propre Hermes quand il doit garder des secrets ; `own_space.rs` le fait tout
  seul au premier secret. `sandbox.rs` donne à chaque agent un bac à sable isolé (aucun dossier de la machine, seulement
  les dépôts de paquets).
- `browsers.rs` lance le vrai Chrome de chaque agent dans son conteneur ; `browser_mcp.rs` démarre ses outils à
  l'usage ; `desktop.rs`, `desktop_page.rs` et `desktop_read.rs` le montrent dans l'app et laissent la personne
  reprendre la main.
- Entre agents : `peers.rs` (seulement par les liens que la personne a activés), `agent_messages.rs`,
  `agent_pairs.rs` (une conversation « A ⇄ B » à part), `agent_files.rs` (des fichiers, que le `message_agent` de
  Hermes ne transporte pas), `shares.rs` (données en lecture seule).

## Sécurité

- **Approbations.** Le bridge laisse les approbations de Hermes en mode manuel (`sandbox.rs`), les formule
  simplement, garde celles déjà répondues dans la conversation (`approvals_log.rs`) et suit le protocole de demandes
  de Hermes 0.22.5 (`hermes_prompts.rs`).
- **Injection de consignes.** La personnalité de chaque agent dit que ce qu'il lit (pages, mails, fichiers, autres
  agents) est une information, jamais un ordre (`souls.rs`). Les autres agents n'atteignent qu'un « Bot Chat »
  (`peers.rs`). Les tours d'invités ont des outils fermés (patch 0018).
- **Ce qui sort.** `sandbox_sites.rs` et `send_guard.rs` : lire le web est libre, mais envoyer des données à des sites
  que la personne n'a pas validés est plafonné (64 Ko par site et par heure), puis demandé ; `heavy_work.rs` fait
  demander les agents avant une grosse installation ou compilation sur un serveur partagé ; `body_limit.rs` limite la
  taille des requêtes ; `beneath.rs` garde l'accès aux fichiers d'un bac à sable sous son dossier.
- **Coffre.** `vault.rs` garde les identifiants chiffrés là où aucun agent ne peut les lire ; Hermes remplit les
  connexions sans que le modèle voie le mot de passe. `vault_logins.rs` crée des comptes avec des mots de passe que
  l'agent ne voit jamais ; `vault_contacts.rs` garde les coordonnées ; `secrets.rs` garde les secrets d'un connecteur.
- **Paiements.** `cards.rs`, `card_pay.rs`, `card_fill.rs` : cartes virtuelles plafonnées, chaque paiement approuvé ;
  `handover.rs` : « l'agent prépare, tu paies » ; `link_wallet.rs` : le portefeuille Link de Stripe aux États-Unis et
  au Canada.
- **Freins.** `loop_brake.rs` arrête un tour qui tourne en rond (un modèle a fait un jour 267 appels en un seul
  tour) ; `agent_restart.rs` ne redémarre le Hermes d'un agent que s'il y a une raison.

## Mémoire, Carnet, salons et famille

- `notebook/` est le Carnet : les personnes, sujets, tâches, dossiers et voyages dont la personne a parlé à ses
  agents, rangés et lisibles ; `archives/` indexe de gros tas de documents ; `message_search.rs` cherche dans toutes
  les conversations.
- `memory_edit.rs` montre et modifie la mémoire de Hermes dans l'app ; `shared_memory.rs` ajoute un « ce que mes
  agents savent de moi » commun ; `memory_language.rs` garde la mémoire dans la langue de l'agent ; `own_skills.rs`
  liste les compétences que l'agent a écrites lui-même.
- `rooms.rs`, `rooms_handoff.rs`, `rooms_memory.rs`, `rooms_remote.rs` : discussions de groupe entre agents, y compris
  l'agent d'un proche.
- `households.rs` (avec `door_tls.rs`) relie volontairement deux installations ; `family_agent.rs`,
  `family_news.rs`, `agent_shares.rs`, `meet.rs` : un agent commun à la famille, des nouvelles transmises seulement
  après accord, des agents partagés, deux agents qui trouvent une date. `read_shares.rs`, `project_agent.rs`,
  `friends_cards.rs` (avec `shared_expenses.rs`) et `templates.rs` (avec `templates_scrub.rs`) partagent une conversation en lecture, l'agent d'une
  occasion ou un agent comme modèle.

## Connecteurs et tes données

- Courrier : `mailbox.rs` (toute boîte IMAP), `mail_scan.rs`, `mail_tidy.rs` ; l'adresse propre d'un agent :
  `agent_email.rs`, `agent_email_smtp.rs`.
- Données du téléphone, partagées seulement avec les agents choisis : `calendar.rs`, `caldav.rs`, `contacts.rs`,
  `reminders.rs`, `health.rs`, `phone_asks.rs`, `phone_rights.rs`.
- Imports : `notes_import.rs`, `whatsapp_import/`, `assistant_import.rs` (un agent venu d'un autre assistant).
- Services : `services.rs` (serveurs MCP du catalogue de l'app), `connections.rs` (connexion OAuth 2.1, aucune clé à
  coller), `connectors.rs` et `connectors_run.rs` (connecteurs écrits par un agent, lancés dans son bac à sable),
  `watchers.rs`.
- Outils servis en MCP à chaque agent (`agent_tools.rs`, `mcp_sessions.rs`) : audio et podcasts, `weather.rs`,
  `routes.rs`, cartes.

## Fichiers, pages et cartes

- `files.rs` range ce que la personne envoie (feuille de partage comprise) là où le bac à sable de l'agent peut le
  lire ; `files_kept.rs` règle la durée de conservation. `media.rs`, `sent_files.rs`, `give_file.rs`, `made_files.rs`
  et `cited_files.rs` font qu'un fichier fait ou cité par l'agent arrive vraiment à la personne.
- `pages.rs` : tableaux de bord, suivis et petits outils que l'agent écrit en un fichier HTML, verrouillés par une
  Content-Security-Policy (pas de réseau, pas de formulaires) avant d'être affichés.
- `maps.rs`, `map_tiles.rs`, `map_route.rs` : cartes OpenStreetMap via le bridge, cartes interactives, vrais trajets
  par les rues ; `favicons.rs` récupère les icônes des sites à la place du téléphone.

## Travail en autonomie

- `jobs.rs` : tâches de fond avec un budget en euros choisi avant, compté comme le crédit IA (lectures du cache à
  leur prix, `task_cost.rs`), pause et reprise ; `handback.rs`
  rapporte le compte rendu dans la conversation d'origine.
- `cronwatch.rs` livre la réponse de chaque routine dans l'app ; `autonomous.rs` liste tout ce que les agents font sans
  qu'on le leur demande ; `storage_watch.rs` fait demander aux agents quoi garder ; `space_tidy.rs` efface tout seul ce
  qui est temporaire dans leur espace (vieux brouillons, dossier d'une tâche finie, vieux téléchargements, caches),
  jamais les fichiers de la personne ; `feature_tips.rs` fait découvrir les fonctions au fil du temps.

## Maîtrise des coûts

- `tool_diet.rs` range les outils peu utilisés derrière la recherche d'outils de Hermes, selon le type d'agent
  (mesuré : environ 19k jetons d'entrée par appel, dont 11.8k de schémas d'outils).
- `model_route.rs` fixe un seul fournisseur chez EUrouter pour que son cache serve (entrée en cache à environ 17,5 %
  du prix) ; `model_choice.rs` laisse choisir un modèle par agent à qui s'auto-héberge.
- Le patch 0020 donne la consommation de chaque appel au modèle, dont `jobs.rs` et `runs.rs` se servent pour arrêter
  une tâche avant qu'elle dépasse son budget. `credit.rs` met en pause ce qui tourne seul quand le crédit du mois est
  épuisé, jamais les conversations.
- `sleep.rs` arrête Hermes sur un espace inutilisé depuis des jours et le réveille à la requête suivante.

## Installation, sauvegardes et déménagements

- L'installateur ([`installer/`](../installer/)) met en place Hermes, le bridge, les voix et les bacs à sable en une
  commande, et les met à jour. `place_backup.rs` expose les sauvegardes et leur clé ; `place_import.rs` fait venir ton
  propre Hermes ; `place_move.rs` déménage un espace vers un autre serveur ; `ssh_keys.rs` permet à la personne de se
  connecter à son propre compte.
- `space.rs` : l'espace chiffré au repos (désactivé par défaut).

## L'offre tout compris

Sur les serveurs de Kerfy Studio, chaque personne a un **espace** : son compte, son Hermes et son bridge.
`cloud_setup.rs` installe un nouvel espace à partir d'un serveur prêt ; `place_cert.rs` lui obtient un vrai certificat
Let's Encrypt sous `<label>.p.bipagents.com` ; `proxy_header.rs` permet à chaque espace de répondre sur le port 443
derrière un routeur SNI (beaucoup de réseaux bloquent les autres ports) ; `credit.rs` et `beta_measures.rs` parlent au
guichet, le service de comptes et de facturation de Kerfy Studio (un dépôt privé, hors de celui-ci).

## Les patches Hermes que nous portons

Quelques changements dont les apps ont besoin ne sont pas encore dans Hermes. Ils sont dans
[`hermes/patches/`](../hermes/patches/README.md), au-dessus de la version stable épinglée (`BASE` : v0.22.5), et
l'installateur les applique.

| Patch | Pourquoi |
|---|---|
| 0005 le coffre remplit après un redémarrage du navigateur | les connexions marchent encore après la fermeture du navigateur de l'agent |
| 0006 réponse synchrone d'un pair | un agent interrogé par un autre répond dans la conversation de celui qui demande |
| 0007 `browser.vault_only` | les agents avec leur propre navigateur visible gardent le coffre sans le navigateur de Hermes |
| 0008 fuseau horaire, routines, heure murale | les routines tournent à l'heure de la maison, une fois par heure murale |
| 0009 recherche d'outils `!nom`, catégories de compétences compactes | le régime de jetons : certains outils gardés en direct, les compétences rares par leur nom seulement |
| 0010 réponses vides | les appels d'outils écrits en texte par un modèle deviennent de vrais appels ; les nouvelles tentatives changent la requête |
| 0011 titres avec GLM | GLM répond vide aux appels sans réflexion, donc les titres demandent une réflexion « low » |
| 0013 résultats MCP : mots de passe du coffre effacés | un mot de passe ne revient jamais au modèle par un résultat d'outil |
| 0015 ancre temporelle | l'heure actuelle voyage avec chaque message |
| 0016 un seul `message_agent`, celui du bridge | chaque message entre agents passe par les contrôles du bridge |
| 0017 coordonnées | les adresses du coffre portent un nom, un téléphone et un e-mail |
| 0018 tours d'invités | le tour d'un proche n'a que les outils de l'invité et pas de USER.md |
| 0019 les routines répondent dans leur conversation | une routine créée dans une conversation y répond |
| 0020 consommation par appel dans l'API Runs | le vrai coût de chaque appel au modèle, pour les budgets |
| 0021 la conversation principale reste celle de l'app | la conversation principale de chaque agent garde son affichage en direct et son historique sur la v0.22.5 |

**Comment nous les proposons en amont.** La série est une branche git au-dessus de Hermes, exportée avec
`scripts/hermes-patches.sh` (`branch`, `export`, `check`) : un commit par patch, des noms stables, les mêmes octets à
chaque fois. Passer à un Hermes plus récent, c'est rebaser cette branche, puis une nouvelle `BASE`. Un patch disparaît
dès que Hermes fait la même chose : 0001 (clarify dans l'API Runs) et 0004 (images gardées pour les questions
suivantes) sont partis quand la v0.22.5 a eu les siens. 0005 est proposé en amont (NousResearch/hermes-agent#136660) ;
les petits patches généraux (le correctif de 0008, 0009, 0011, 0013, 0017, les libellés de 0019) sont les
prochains candidats. Le détail de chaque patch : [`hermes/patches/README.md`](../hermes/patches/README.md).

## Ce que nous laissons volontairement à Hermes

La boucle du modèle et ses outils, les fichiers de mémoire et les compétences, le planificateur cron, le moteur
d'approbations, le stockage du coffre et le remplissage des formulaires, le Bot Mode, les profils, le bac à sable
docker du terminal, le Bot Desktop, les sessions et leur base, et les passerelles. Le bridge les règle (par la CLI et
les fichiers de configuration de Hermes) et parle à Hermes par son API Runs ; il ne crée jamais de fork de Hermes. Ce
qui est assez général est proposé en amont plutôt que gardé ici.
