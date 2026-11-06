# Parcours de sortie — ce que la personne doit VOIR

Règle d'Alex (6/11) : « Il faudra éviter à tout prix des régressions de ce type-là quand on fait des releases, des
grosses releases. Très important. »

Ce qui l'a déclenchée : depuis le 3/11 (commit 40794026, l'envoi passe par le pont), chaque photo envoyée s'affiche
« 0 Ko » et ne s'affiche jamais. Les tests du pont vérifiaient seulement ce que le modèle reçoit ; les tests UI de
l'iPhone tournaient en démo, qui ne passe jamais par ce chemin. Personne ne regardait l'écran.

Donc, à chaque sortie : les gestes d'une personne, joués par les VRAIES apps contre un VRAI pont (la version de
sortie), avec un faux Hermes dont le « modèle » répond comme un agent (il écrit mot à mot, appelle des outils, rend des
fichiers, des pages, des cartes, pose des questions, demande des autorisations). Chaque parcours vérifie ce qui est À
L'ÉCRAN : la vignette a une image et une taille, le fichier s'ouvre, la carte montre ce qu'elle demande.

**Une seule commande** : `scripts/release-journeys.sh` (≈ 20 min). Elle imprime le tableau ci-dessous rempli
(✅ / ❌). **Une sortie ne part pas si un parcours échoue.**

## Les trois couches

| Colonne | Quoi | Où |
|---|---|---|
| **Pont** | Le parcours fait comme les apps le font en HTTP, et CHAQUE page de la conversation vérifiée « affichable » : chaque photo a sa taille et ses octets, chaque fichier se télécharge, chaque carte a ses champs, chaque ligne « En attente » a quelque chose à montrer, et chaque champ a le type que les deux apps savent lire (un seul champ faux et l'iPhone perd la page entière). | `bridge/tests_blackbox/test_journeys.py` + `bbkit/displayable.py` (dans la suite boîte noire complète aussi) |
| **iPhone** | L'app réelle (pas la démo), appairée par le QR code de l'installation au pont, sur un simulateur temporaire ; regarde l'écran. | `BipAgentsUITests/ReleaseJourneysTests.swift` (+ `App/Features/Conversation/JourneyProbe.swift`, DEBUG) |
| **Android** | L'app réelle (MainActivity, ChatScreen) sous Robolectric, contre le pont en vrai HTTP ; regarde l'écran (nœuds Compose). | `android/app/src/test/java/io/github/bipagents/journeys/ReleaseJourneysTest.kt` |

Le pont et son faux Hermes : `bridge/tests_blackbox/bbkit/journeys.py` (un pont « comme chez quelqu'un » : agent Vie,
outils isolés dans le bac à sable, ses fichiers servis, sa porte HTTPS) et `bbkit/model.py` (le « modèle »). Les
photos y sont gardées comme Hermes v0.22.5 les garde (un fichier du profil nommé dans le message, par le chemin
qu'Hermes voit dans son conteneur), pas comme le pont l'imagine. `serve_journeys.py` sert ce pont aux apps.

Mots-clés que le faux modèle comprend : `#fichier`, `#page`, `#carte`, `#outil`, `#question`, `#approbation`,
`#secret`, `#sites`, `#connecteur`, `#lent` (voir `bbkit/model.py`).

## Les parcours

Légende : **P** = colonne Pont, **i** = iPhone, **A** = Android ; « existant : … » = déjà couvert par un test qui
tourne à chaque sortie (pas contre ce pont-ci) ; **manuel** = à faire à la main sur TestFlight / Play interne avant
la bêta.

### Envoyer

| N° | Parcours | Ce que la personne doit voir | Automatisé |
|---|---|---|---|
| J01 | Envoyer un texte | Son message tout de suite ; la réponse arrive mot à mot (pas d'un bloc à la fin) | P, i, A |
| J02 | Envoyer une photo | La vignette avec l'IMAGE, jamais « 0 Ko » ; elle s'ouvre en plein écran ; toujours là en rouvrant (relue depuis le pont) | P, i, A |
| J03 | Envoyer plusieurs photos | Chacune avec son image, dans l'ordre, avant et après réouverture | P, i, A |
| J04 | Envoyer un document | Son nom et sa vraie taille ; il s'ouvre ; la conversation reste lisible ensuite | P, i, A |
| J05 | Envoyer une note vocale | La bulle vocale (durée, ses mots) ; elle se réécoute | P (ses mots) ; bulle et audio : manuel |
| J06 | Photo + texte | Les deux, dans la même bulle | P, i, A |

### Recevoir

| N° | Parcours | Ce que la personne doit voir | Automatisé |
|---|---|---|---|
| J07 | Une réponse avec un fichier | La ligne du fichier ; il s'ouvre | P, i, A |
| J08 | Une réponse avec une page | La page dans la conversation, avec son contenu | P, i |
| J09 | Une réponse avec une carte | La carte | P, i |
| J10 | Les étapes de l'agent (outils) | Ses étapes entre ses mots, dans l'ordre | P |

### Cartes

| N° | Parcours | Ce que la personne doit voir | Automatisé |
|---|---|---|---|
| J11 | Une question de l'agent | La carte avec la question et ses choix ; un toucher, l'agent continue avec la réponse | P, i, A |
| J12 | Une autorisation (commande) | Ce qu'elle demande ; « Oui, vas-y » ; l'agent continue ; la réponse reste dans la conversation | P, i, A |
| J13 | Une carte « mot de passe » (secret) | Dans la conversation pendant que l'agent attend ; elle s'en va une fois remplie, même ailleurs | P, i |
| J14 | Une carte « sites » | Les sites demandés ; elle s'en va une fois répondue | P |
| J15 | Une carte « connecteur » | La carte ; répondue, l'agent continue | P |
| J16 | Une carte répondue depuis la Boîte | Elle quitte la conversation ; la ligne de la Boîte est lue | P |
| J17 | Une carte après « Arrêter » | Plus de carte en attente ; la conversation continue | P |
| J21 | Une carte répondue tard (après la fin du tour) | L'agent reprend avec la réponse | existant : `test_cards_resume.py` |
| J22 | Une carte « e-mails » (déplacer, archiver) | Une seule carte par décision, dans la conversation et la Boîte | existant : `test_mail_cards.py`, `test_cards_inline.py` |

### Envoyé ou pas, en attente

| N° | Parcours | Ce que la personne doit voir | Automatisé |
|---|---|---|---|
| J18 | « Pas envoyé » puis renvoyer | « Pas envoyé · Toucher pour réessayer » sous un message qu'aucun tour n'a pris ; un toucher le renvoie, la réponse vient, le message une seule fois | P, i, A |
| J19 | « En attente » montre chaque sorte de message | Une photo, un document, un texte « Après » envoyés pendant une réponse : chacun dans « En attente » avec ce qu'il est (jamais une ligne vide) | P, i, A |

### Boîte, notifications, le reste

| N° | Parcours | Ce que la personne doit voir | Automatisé |
|---|---|---|---|
| J20 | Le rapport d'une routine | Dans la Boîte et la conversation, avec son fichier ; la pastille compte | P |
| J23 | La pastille de la Boîte | Le bon nombre, remis à zéro une fois lu | existant : `test_boite_badge.py` |
| J24 | Une notification ouvre le bon endroit | La conversation, sur le bon message, les plus récents dessous | existant : `test_notifications_journal.py`, iOS `RealBridgeTimelineTests`, Android `NotificationOpenTest` |
| J25 | Les salons (plusieurs agents) | Les messages de chacun, qui répond, les fichiers | existant : `test_rooms*.py` ; apps : manuel |
| J26 | Partager avec un ami (lecture) | Le lien s'ouvre chez l'ami ; « Vu par N personnes » ; la carte partagée | existant : `test_read_shares.py`, `test_read_share_live_count.py` ; apps : manuel |
| J27 | Une tâche de fond | Sa progression, puis son rapport dans la conversation | existant : `test_jobs.py`, `test_long_work.py` ; apps : manuel |
| J28 | Les routines | Créer, « Mode silencieux », glisser → 🗑 « Supprimer ? » | existant : `test_routines.py` ; iOS démo `AutonomousDelete/SwipeTests` ; réel : manuel |
| J29 | La copie hors ligne | La conversation s'ouvre sans réseau, telle qu'elle était | Android `OfflineCopyTest` (unitaire) ; iPhone : manuel |
| J30 | Changer d'agent | La conversation du bon agent, son nom, sa couleur | manuel |
| J31 | Le guide | Le guide répond et propose ses premiers pas | existant : `test_guide.py`, `test_guide_team.py` ; apps : manuel |

## Ce que la suite a trouvé (6/11)

- **Photos « 0 Ko », jamais affichées** (J02, J03, J06) : en échec sur main-next 4665c820, sur les trois couches —
  l'iPhone montre une puce « A0 · Zéro ko » à la place de l'image, Android rien ; côté pont la photo n'a pas de taille
  et son adresse répond 404 (Hermes v0.22.5 garde la photo dans un fichier de SON conteneur, que le pont ne lit pas).
  **Passent** une fois `queue-row-empty` fusionné (main-next 31258042).
- **Un document envoyé casse la conversation** (J04, pas corrigé) : une fois le message rangé par Hermes, le pont
  donne la taille du document en mots (`"size": "3 Ko"`, `threads/timeline.rs` `visible()`), les deux apps attendent
  un nombre : l'iPhone et Android rejettent la page ENTIÈRE (« Ça n’a pas marché », plus rien de neuf n'arrive dans
  cette conversation). En plus, sur Android, un document relu depuis le pont ne s'ouvre pas (pas d'`url`).
- **« Pas envoyé »** (J18, apps) : le pont fait juste ; l'iPhone garde « Pas envoyé » sous un message renvoyé avec
  succès et répondu ; Android ne montre jamais « Pas envoyé » dans une conversation ouverte (le pont ne prévient pas
  quand les 10 s passent, et la relecture `after=` ne relit pas l'élément). **Corrigé** par `not-sent-fix` : le pont
  redonne les messages pas encore rangés dans chaque relecture `after=` et prévient (`thread`, `lost`) quand les 10 s
  passent ; l'iPhone ne reporte plus « Pas envoyé » d'une relecture à l'autre.
- **« En attente »** (J19) : une photo ou un document seuls donnaient une ligne vide ; Android envoyait une photo
  pendant une réponse sans la photo. Corrigé par `queue-row-empty`.
- Petit : les messages « Pas envoyé » d'une conversation supprimée reviennent dans la nouvelle (le pont les garde
  3 jours dans `thread_sends`, sans lien avec la conversation effacée). **Corrigé** par `not-sent-fix`.
- À regarder (Android) : vider la conversation principale depuis le pont (`DELETE`), puis la rouvrir pendant une
  réponse, montre une conversation vide. Les parcours Android ont chacun leur propre conversation (pas de `DELETE`).

## Ce qu'il manque encore (prochaines étapes)

- Faire passer l'iPhone et Android sur J08–J10, J13–J17 et J20 (déjà vérifiés côté pont).
- Les parcours « existant » contre CE pont depuis les apps : salons, partages, tâches de fond, routines, notification
  ouverte depuis une vraie poussée.
- Un vrai Hermes derrière le pont (le serveur de build en a un : `~/qc`), une fois par grosse sortie, pour vérifier
  que le faux modèle garde les choses comme lui.

## Lancer

```sh
scripts/release-journeys.sh                 # tout : pont + iPhone + Android (≈ 20 min)
scripts/release-journeys.sh --no-ios        # sans le simulateur
scripts/release-journeys.sh --expected J02,J03,J06   # rapport : ces échecs connus marqués ⚠️ (jamais pour une sortie)
```

Le pont est construit et servi sur le serveur de build (`JOURNEYS_REMOTE`, par défaut `build@100.91.112.69`), dans
son propre dossier effacé après ; Android y tourne aussi ; le Mac ne fait que l'iPhone (un simulateur temporaire,
effacé après, sous les créneaux iOS du Mac). Résultats et journaux : `.build/release-journeys/<date>/`.

Le simulateur joint le pont par un tunnel ssh sur le `127.0.0.1` du Mac : lancé depuis Claude Code, le script doit
tourner HORS du bac à sable de Claude (le simulateur n'atteint pas un port ouvert dans le bac à sable : la requête
reste suspendue). Les apps de test ne sont pas signées : sous `-journeyBridge`, la clé du pont est gardée en mémoire
(pas de Trousseau, `App/Model/Keychain.swift`, DEBUG seulement).

Avant de supprimer un chemin de code : [la liste](../internal/notes/removal-checklist.md).
