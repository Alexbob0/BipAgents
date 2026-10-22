[🇬🇧 English](app-data.md) · 🇫🇷 Français

# Ce que les apps BipAgents envoient sur le réseau

**En bref :** les conversations, fichiers, transcriptions de notes vocales, données de santé, rappels et la mémoire
des agents ne vont qu’au serveur de la personne (son bridge et Hermes). Les services de Kerfy Studio reçoivent des
données de compte, d’achat et d’acheminement des notifications, ainsi que des paquets scellés qu’ils ne peuvent pas
ouvrir. Les apps n’intègrent aucun SDK de mesure d’audience ni de rapport de plantage. Apple et Google reçoivent ce
qu’exigent leurs boutiques, la connexion et les notifications, et la voix pour la dictée seulement si la personne a
accepté la dictée en ligne. Avec l’offre tout compris, l’adresse e-mail propre d’un agent garde ses e-mails sur le
serveur de messagerie de Kerfy Studio ; c’est le bridge de la place qui parle à ce serveur, pas l’app.

Cette page a été écrite à partir du code de l’app iPhone/iPad (`App/`, `Widgets/`, `NotificationService/`,
`BipVoices/`, `Packages/`) et de l’app Android (`android/`), dans l’état du 21/10/2026 (après le commit `4b6eb92`).
**Le code des apps est privé :** les chemins de fichiers des apps cités plus bas sont des références, pas des fichiers
publics. Les parties publiques sont : le bridge ([`bridge/`](../bridge/), [`bridge-rs/`](../bridge-rs/)),
l’installateur ([`installer/`](../installer/)), le relais de notifications ([`relay/`](../relay/)) et le code de
chiffrement du compte ([`App/Features/Account/AccountCrypto.swift`](../App/Features/Account/AccountCrypto.swift),
[`android/…/account/AccountCrypto.kt`](../android/app/src/main/java/io/github/bipagents/account/AccountCrypto.kt) et
son test, [`AccountCryptoTest.kt`](../android/app/src/test/java/io/github/bipagents/account/AccountCryptoTest.kt)).
C’est une lecture du code, pas une capture du trafic réseau. L’adresse e-mail d’agent (§1) a été ajoutée le 22/10/2026
d’après le contrat d’interface du bridge, avant le code des apps qui s’en sert.

## 0. Bibliothèques : aucun traceur

- **iOS** (`BipAgents.xcodeproj/.../Package.resolved`, `Packages/*/Package.swift`) : un seul paquet externe,
  `onnxruntime-swift-package-manager` 1.24.2, qui fait tourner les voix des Bips sur le téléphone. Aucun SDK de mesure
  d’audience, de plantage, de publicité ou d’attribution. Ni MetricKit, ni AdSupport, ni AppTrackingTransparency.
- **Android** (`android/app/build.gradle.kts`, `android/gradle/libs.versions.toml`) : AndroidX/Compose, OkHttp,
  kotlinx-serialization, CameraX, Media3, Glance, ZXing, ONNX Runtime, le client Health Connect, et ces bibliothèques
  Google : Play Billing, Play Asset Delivery, Play Integrity, Credential Manager + Google ID, le scanner de documents
  ML Kit (services Play) et **Firebase Cloud Messaging seul** (`firebase-messaging` ; ni Analytics, ni Crashlytics, ni
  plugin `google-services`).
- La télémétrie d’usage d’ONNX Runtime vers Microsoft est coupée deux fois sur Android : son `TelemetryInitializer` est
  retiré du manifeste (`AndroidManifest.xml`) et `PocketEngine.kt` appelle `setTelemetry(false)`. L’app iOS n’a pas
  d’appel équivalent.
- Firebase ne démarre qu’une fois les notifications activées (`firebase_messaging_auto_init_enabled=false`,
  `push/Push.kt`).
- Transport : HTTPS uniquement dans les versions Android publiées (`res/xml/network_security.xml`, rien en clair). iOS
  n’a aucune exception App Transport Security (`Config/Info.plist`). Les serveurs de la personne, sur le réseau local ou
  sur Internet, utilisent des certificats auto-signés : un certificat n’est accepté que si son empreinte correspond à
  celle épinglée (`App/Model/LocalLink.swift`, `net/Http.kt`, `CloudPairing`).

## 1. Le serveur de la personne : bridge et Hermes

C’est soit la machine ou le VPS de la personne (joint par son tailnet, son réseau local ou sa « porte »), soit sa place
de l’offre tout compris. Tout ce qui est personnel va ici, avec la clé de ce serveur (`App/Model/BridgeClient.swift`,
`Packages/HermesKit/Sources/HermesKit/Client/HermesClient.swift`, `Input/DocumentUploader.swift`,
`android/.../net/Bridge.kt`, `net/Hermes.kt`, `net/Http.kt`). Code public : `bridge/`, `bridge-rs/`.

- **Conversations :** les messages, le texte des notes vocales et des appels Live (reconnu sur le téléphone, ou en
  ligne seulement si la personne l’a accepté, voir §4), les photos et fichiers (`/v1/runs`, `/v1/files`, `/v1/media`), les approbations et réponses, les
  événements en direct (SSE). **L’audio d’une note vocale reste sur le téléphone.** Les apps n’utilisent pas de
  WebSocket.
- **Agents :** réglages, modèle, langue, apparence, outils, services connectés, pages de mémoire (lues et modifiées),
  import de notes, pages, tâches de fond, tâches planifiées, partages entre agents, liens familiaux (`/v1/family/*`).
- **Secrets saisis par la personne :** mots de passe des boîtes mail (`/v1/mail/accounts`, et la boîte propre d’un
  agent chez le fournisseur de la personne dans `/v1/agents/{id}/email`), éléments du coffre (`/v1/vault`), clés SSH
  publiques (`/v1/ssh/keys`), réglages d’un serveur de voix.
- **Santé** (`App/Features/Settings/HealthSync.swift`, `android/.../health/HealthSync.kt`) : un résumé par jour,
  envoyé par `PUT /v1/health-data/days` à chaque bridge dont au moins un agent a « Santé » activé, avec seulement les
  données des catégories que lisent les agents de ce bridge (`shared` dans `GET /v1/health-data` ; la liste de chaque
  agent se règle par `PATCH /v1/agents/{id}/settings {"health_categories": […]}` depuis les détails de la tuile
  « Santé ») : activité (pas, énergie active, minutes d’exercice, séances, distance, étages, heures debout), sommeil
  (endormi/au lit, heures, phases, respiration, température du poignet), cœur (pouls au repos, VFC, pouls moyen/min/max
  du jour, oxygène dans le sang, VO2 max), corps (poids, masse grasse, IMC, tension), alimentation et eau (calories
  mangées, eau, caféine), bien-être (pleine conscience, état d’esprit d’iOS 18), et les plus sensibles, désactivées
  par défaut et laissées hors de « Tout partager » : glycémie (mmol/L) et cycle (flux, début des règles). Un
  interrupteur activé commence avec activité, sommeil et cœur. Heures debout, température du poignet, IMC et humeurs
  n’existent que sur iPhone ; la pleine conscience sur Android seulement là où Health Connect l’a. Chaque catégorie
  demande l’autorisation du système pour ses types quand on l’active. Seuls les jours nouveaux ou
  modifiés partent : les 7 derniers jours sont revérifiés à chaque fois, et l’envoi remonte jusqu’à 30 jours. L’iPhone
  envoie aussi en arrière-plan quand Santé a de nouvelles données (`enableBackgroundDelivery`, quotidien). Android ne
  lit que lorsque l’app est ouverte. Le téléphone ne garde que des empreintes de ce qu’il a envoyé. Sur le bridge, les
  journées ne gardent que les catégories qu’au moins un agent lit (une catégorie que plus aucun ne lit en est effacée),
  et chaque agent qui a l’interrupteur activé ne lit que ses catégories (`bridge-rs/src/health.rs`, `Day::restricted`).
- **Rappels de l’iPhone** (`App/Features/Settings/RemindersSync.swift`) : une copie des rappels part aux bridges choisis
  par la personne, et l’app exécute les demandes que les agents ont mises en attente. Cette fonction n’existe que sur
  iOS.
- **L’adresse e-mail d’un agent** (`GET/POST/PATCH/DELETE /v1/agents/{id}/email`,
  `POST /v1/agents/{id}/email/held/{mail}`) : l’activer (une adresse de Kerfy Studio sur une place de l’offre, ou la boîte
  de la personne chez son fournisseur, avec son adresse, son mot de passe et ses serveurs IMAP/SMTP), les adresses
  autorisées, la limite par jour, et approuver ou abandonner un e-mail que l’agent veut envoyer à une nouvelle adresse
  (l’app affiche ses destinataires et son objet). Pour une adresse de Kerfy Studio, c’est le bridge de la place qui
  demande lui-même la boîte au guichet, avec le jeton de sauvegarde de la place ; le guichet donne le mot de passe une
  seule fois au bridge, qui le garde dans les réglages de l’agent : **le téléphone ne le voit jamais**. Un e-mail en
  attente déclenche une notification (`email_held`).
- **Inscription aux notifications :** le jeton APNs ou Firebase (ou, à la place, un jeton de relais, voir §3), avec les
  identifiants des agents (`/v1/devices`). L’extension de notification (`NotificationService/NotificationService.swift`)
  et le service Android (`push/BipMessagingService.kt`) vont ensuite chercher sur le bridge le texte de la réponse, la
  commande à approuver et l’audio (`/v1/replies`, `/v1/outbox`, `/v1/approvals`). Le contenu des notifications ne passe
  pas par Apple ni par Google.
- **Voix :** les réponses des agents sont envoyées au bridge pour être lues (`/v1/tts/*`) quand le téléphone ne les lit
  pas lui-même.
- **Icônes des sites** devant les liens : demandées au bridge (`/v1/favicon/<hôte>`), qui interroge le service d’icônes
  de DuckDuckGo. Le téléphone ne contacte jamais ce service ni le site.
- **Le navigateur visible de l’agent** (`App/Features/Agents/DesktopView.swift`) : une vue web de la page de
  visualisation du bridge, sans données de site conservées.
- **Appairage d’une place de l’offre** (`App/Features/Cloud/CloudPairing.swift`, `android/.../cloud/CloudPairing.kt`) :
  `GET /v1/setup?nonce=`, puis `POST /v1/setup/claim` sur le certificat prouvé puis épinglé. La demande contient le
  modèle, **la clé d’IA que le guichet a créée pour cette place** (transmise par le téléphone), les agents à créer et la
  langue. Pour une restauration, elle contient à la place la clé de sauvegarde que le téléphone a gardée.
- **Sauvegardes :** le téléphone lit la clé et l’état de sauvegarde de la place (`/v1/backup`) et garde la clé (§7).
- **Nouvel appareil sur le compte :** le téléphone demande à chacune de ses places une nouvelle clé pour cet appareil
  (`/v1/device-keys`), puis les scelle (§2).

Les widgets ne font aucun appel réseau. Ils lisent ce que l’app a déjà enregistré dans l’App Group partagé (iOS,
`Widgets/BipWidgets.swift`) et ouvrent l’app par un lien (Android, `widget/AgentWidget.kt`).

## 2. Le guichet de Kerfy Studio (`https://guichet.kerfystudio.com`)

C’est le service de l’offre tout compris, avec les comptes et les liens familiaux (`App/Features/Cloud/Guichet.swift`,
`CloudOffer.swift`, `PlaceRescue.swift`, `App/Features/Account/BipAccount.swift`, `FamilyLinks.swift` ;
`android/.../cloud/Guichet.kt`, `CloudOffer.kt`, `PlaceRescue.kt`, `DeviceProof.kt`, `account/BipAccount.kt`,
`account/FamilyLinks.kt`). **Le code du guichet n’est pas dans le dépôt public.**

| Quand | Appel | Ce qui est envoyé |
|---|---|---|
| Quand la personne ouvre l’offre (pas sur l’écran d’accueil, pas en démo) | `GET /v1/where?country=` | le code pays de la boutique (storefront de l’App Store, configuration de facturation Play) |
| Écrans de l’offre, et choix du modèle d’un agent sur une place de l’offre | `GET /v1/models?region=` | la région seule |
| Avant l’achat | `POST /v1/offer` | forfait, région ; pour la semaine offerte : sur iOS un **jeton DeviceCheck**, sur Android un **jeton Play Integrity + le SHA-256 de `"bipagents-trial:"` + ANDROID_ID** (l’identifiant brut n’est pas envoyé) |
| Achat, restauration, changement de forfait | `POST /v1/activate` | iOS : la **transaction signée (JWS)** de l’App Store + un jeton DeviceCheck ; Android : le **jeton d’achat** Play ; région, ticket |
| Crédit d’IA acheté | `POST /v1/subscription/credit` | la preuve de la boutique, l’identifiant du produit |
| Suivi de l’abonnement et des places | `GET /v1/subscription`, `/v1/places/mine`, `POST …/claimed`, `…/withdraw` | le jeton d’abonnement ou le jeton de la place |
| Avis de panne | `POST /v1/subscription/phone`, `/v1/places/phone` | le **jeton de relais** (jamais le jeton APNs ou Firebase) |
| Connexion au compte | `POST /v1/account/email`, `/v1/account/signin` | adresse e-mail (et code reçu), ou **jeton d’identité Apple ou Google** ; nom de l’appareil (iOS : « iPhone »/« iPad » ; Android : `Build.MODEL`, par ex. « Pixel 8 »), plateforme, **clé publique P-256 de l’appareil**, langue |
| Compte | `GET/PUT /v1/account*` | jeton de relais, nom de l’appareil ; approuver ou retirer des appareils ; supprimer ou garder le compte |
| Approbation d’un nouvel appareil | `POST /v1/account/devices/{id}/approve` | un **paquet scellé** (ci-dessous) |
| Code de récupération | `PUT/POST /v1/account/recovery` | un **vérificateur** du code (jamais le code lui-même) et le **séquestre** (ci-dessous) |
| Invitation à une place partagée (Duo, Famille) | `POST /v1/account/invitations` | le numéro de la place, l’e-mail de la personne invitée, le prénom de celle qui invite (`who`), la langue de l’app. Rien sur la place elle-même : l’e-mail du guichet dit seulement d’ouvrir l’app et de se connecter avec cette adresse |
| Invitations reçues (compte connecté et approuvé) | `GET /v1/account/invitations/received` | le jeton du compte. La réponse contient les places offertes à cette adresse et encore prêtes à être appairées, avec ce qu’il faut pour l’appairage (adresse, port, empreinte du certificat, secret d’appairage, clé d’IA créée par le guichet, jeton de la place), en HTTPS. Une fois la place appairée, `POST /v1/account/places` avec ce jeton la rattache au compte |
| Liens familiaux | `POST /v1/account/family/devices` (corps `{email, siblings}`), `POST /v1/account/family/invitations` | l’e-mail de l’autre personne (dans le corps de la requête, pas dans l’URL), le nom du foyer (`from`), un **paquet scellé** par appareil de cette personne |

**Ce qui est chiffré de bout en bout (code public) :** `AccountCrypto`. Un paquet est scellé sur un
appareil pour la clé P-256 d’un autre : clé ECDH éphémère, HKDF-SHA256 (info `bipagents-box-v1`), puis AES-256-GCM.
Seul l’appareil qui détient la clé privée peut l’ouvrir. Les paquets contiennent la passation entre appareils (pour
chaque place : adresse, empreinte du certificat, clé de bridge du nouvel appareil, clé de sauvegarde, jeton de la
place, et la clé du séquestre) ainsi que les invitations familiales. Le **séquestre** (la clé de sauvegarde de chaque
place) est scellé en AES-256-GCM avec une clé dérivée du code de récupération de 24 caractères (HKDF, 120 bits). Le
guichet ne garde qu’une autre valeur dérivée du code.

**Code de comparaison d’un nouvel appareil** (`AccountCrypto.deviceCode`) : l’appareil en attente affiche un code de
12 chiffres tiré de sa propre clé publique ; l’appareil qui l’approuve affiche le code de la clé que le guichet lui a
donnée, dans l’alerte et à côté d’« Approuver », et demande de n’approuver que si les deux sont identiques. Le code est
un SHA-256 de `"bipagents-device-code-v1"` ‖ 0x00 ‖ l’identifiant de l’appareil ‖ 0x00 ‖ sa clé publique (65 octets),
haché 10 000 fois en tout ; les 8 premiers octets, modulo 10¹², donnent les 12 chiffres. Une clé qui n’est pas une clé
P-256 valable n’a ni code ni bouton « Approuver ». Un vecteur de référence est vérifié dans les deux apps
(`AccountCryptoTest.kt`, et une assertion des versions de débogage dans `AccountCrypto.swift`).

**Jamais envoyé au guichet** (aucun appel de ce type n’existe dans le code) : conversations, fichiers, mémoire,
données de santé, rappels, clés de bridge en clair, le code de récupération lui-même. Les boîtes e-mail des agents
sont demandées par le bridge de la place, pas par les apps (§1).

## 3. Le relais de notifications de Kerfy Studio (public : `relay/`)

Il n’est utilisé que si la version de l’app définit un hôte de relais (`BIPPushRelayHost` / `push.relayHost`)
(`App/Features/Push/PushRelay.swift`, `AppDelegate.swift`, `android/.../push/Push.kt`).

- `POST /v1/register` `{platform, token, environment}` : le jeton APNs ou Firebase, échangé contre un **jeton de
  relais**, c’est-à-dire ce jeton scellé en XChaCha20-Poly1305 avec une clé que seul le relais détient
  (`relay/src/seal.rs`). Le jeton de relais est donné aux bridges qui n’ont pas leur propre clé de notification, et au
  guichet. L’inscription a lieu une fois les notifications autorisées, et seulement si un bridge, l’offre ou le compte
  en a besoin.
- `POST /v1/report` (« Signaler cette réponse ») : **le texte de la réponse signalée**, le motif, le commentaire
  facultatif, la catégorie de l’agent, la version de l’app, le modèle du téléphone (iOS : identifiant matériel comme
  `iPhone17,1` ; Android : fabricant + modèle), la plateforme et la langue. Il n’y a ni nom, ni adresse, ni compte. Le
  relais transmet le tout à `/v1/reports` sur le guichet (`relay/src/routes.rs`). Si le relais n’est pas configuré ou
  ne répond pas, l’app ouvre à la place un e-mail à support@kerfystudio.com avec le même texte. **C’est le seul cas où
  un texte de conversation arrive chez Kerfy Studio, et seulement quand la personne l’envoie.**
- Les notifications elles-mêmes sont envoyées par le bridge ou le guichet, pas par l’app. Le relais ne garde que des
  identifiants, le nom de l’agent dans le titre et une liste fixe de textes génériques (`relay/src/payload.rs`).

## 4. Apple et Google

- **Boutiques :** StoreKit 2 (produits, achats, `Transaction.updates`, codes d’offre, `Storefront`), Play Billing
  (achats, pays de la configuration de facturation). Les boutiques voient l’achat. Le guichet reçoit la preuve (§2).
- **Contrôle de la semaine offerte :** DeviceCheck (`DCDevice.generateToken`, `CloudOffer.swift`) ; Play Integrity
  (`DeviceProof.kt`, seulement si `play.cloudProject` est défini).
- **Connexion :** Sign in with Apple (portée : e-mail) (`AccountSignIn.swift`). Sur iOS, Google passe par le flux avec
  code d’autorisation et PKCE dans une session d’authentification web : `accounts.google.com/o/oauth2/v2/auth`, puis
  `oauth2.googleapis.com/token`, avec la portée `openid email` (`AccountView.swift`, `GoogleSignIn`). Sur Android,
  Google passe par Credential Manager (`GetSignInWithGoogleOption`, `ui/AccountScreen.kt`). Le jeton d’identité part
  ensuite au guichet.
- **Notifications :** inscription APNs (`registerForRemoteNotifications`) et Firebase Messaging (le jeton, plus
  l’identifiant d’installation propre à Firebase). Les contenus portent des identifiants, pas de texte (§3) ; un bridge
  qui a sa propre clé de notification envoie directement.
- **Voix :** les paquets de voix des Bips sont des asset packs hébergés par Apple (`BipVoices/`, `VoicePackages.swift` ;
  iOS 26.4 et plus) et des packs Play Asset Delivery. Ils sont téléchargés à la demande et ne contiennent aucune donnée
  de la personne. Les modèles vocaux d’Apple sur l’appareil peuvent aussi être téléchargés (`AssetInventory`).
- **La reconnaissance vocale reste sur le téléphone par défaut.** iOS : `SpeechAnalyzer` (iOS 26 et plus, modèle de la
  langue installé), sinon `SFSpeechRecognizer` avec `requiresOnDeviceRecognition`. Pour une langue que le téléphone ne
  sait pas reconnaître seul, l’app demande une fois, « Dictée en ligne ? », si la voix peut passer par les serveurs
  d’Apple ; tant que la personne n’a pas dit oui, la dictée est indisponible pour cette langue et l’écrit reste possible
  (`Packages/VoiceKit/…/SpeechRecognizerBackend.swift`, `App/Features/Conversation/OnlineDictation.swift`). Android : le
  reconnaisseur sur l’appareil quand la langue est installée (Android 13 et plus) ; si le système peut télécharger la
  langue, le téléchargement est proposé une fois (Android le demande à la personne) ; sinon, la même question, pour les
  serveurs de Google, et seul un oui permet d’utiliser le reconnaisseur par défaut du téléphone
  (`SpeechRecognizer.createSpeechRecognizer`, en général celui de Google, qui peut fonctionner en ligne)
  (`voice/OnlineDictation.kt`, `voice/Recorder.kt`, `voice/Live.kt`). La réponse est gardée ; le réglage « Dictée en
  ligne (Apple) » / « (Google) », désactivé par défaut, permet de la changer. Cela vaut pour les notes vocales, les
  notes vocales rapides et les appels Live.
- **Scanner de documents ML Kit sur Android** (services Play) : la numérisation se fait sur le téléphone, et le PDF est
  joint comme n’importe quel fichier. Selon Google, ML Kit peut envoyer des données d’usage et de diagnostic. L’app ne
  modifie pas ce comportement.
- Les rapports de plantage et d’usage d’Apple et de Google (partagés avec les développeurs si la personne l’a accepté
  dans les réglages du système) dépendent d’un réglage du système, pas d’un code de l’app.

## 5. Les fournisseurs de modèles d’IA

**Les apps n’appellent jamais un fournisseur de modèle** : aucune adresse d’OpenAI, Anthropic, EUrouter, OpenRouter ou
autre n’apparaît dans le code des apps. Les messages arrivent au modèle par l’Hermes de la personne. Le fournisseur
voit ce que l’agent met dans sa requête : la conversation, les fichiers et les résultats de ses outils, y compris **les
résumés de santé quand un agent qui a « Santé » utilise son outil `health`**. Sur l’offre, l’app ne permet d’activer
« Santé » qu’avec un modèle que le guichet marque `eu_hosted`, et quand le modèle de l’agent est changé dans l’app pour
un modèle qui ne l’est pas, elle désactive aussitôt « Santé » sur le bridge et le dit (`AgentSettingsView.swift`,
`AgentSettingsScreen.kt`, `health/HealthSync.kt`).
La clé d’IA d’une place de l’offre est créée par le guichet et passe par le téléphone pour aller au bridge (§1).

## 6. Le reste

- **Liens touchés par la personne :** liens des réponses, textes juridiques
  (`github.com/Alexbob0/BipAgents/blob/main/docs/legal/…`), Hermes Agent et le code source sur GitHub, CC BY 4.0, pages
  d’abonnement et d’échange de code de Play. Ils s’ouvrent dans le navigateur ou la boutique ; rien d’autre n’est
  envoyé que l’adresse elle-même.
- **Commandes d’installation** (`raw.githubusercontent.com/…/install.sh`, `install.ps1`) : affichées, copiées ou
  partagées. L’app ne les télécharge pas.
- **Pages créées par les agents** (`AgentPages.swift`) : affichées dans une vue web verrouillée. Tout chargement
  `http(s)`, `ws(s)`, `ftp` ou `file` est bloqué, de même que la navigation, les fenêtres surgissantes et les données
  conservées.
- **E-mail au support** (à l’initiative de la personne) : un diagnostic avec les versions, les réglages et l’état de
  chaque agent ; ni conversation, ni nom, ni adresse, ni clé (`Support.swift`, `ui/Support.kt`).
- **Adresses de test réservées au débogage** (un guichet local, des paquets de voix locaux) : les réglages qui les
  définissent (`guichetURL` sur iOS et Android, `voicePackagesURL` sur Android) ne sont lus que dans les versions de
  débogage ; les versions publiées utilisent toujours `guichet.kerfystudio.com`.

## 7. Ce qui reste sur le téléphone

- Trousseau iOS, `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly` (`App/Model/Keychain.swift`, `OfferVault` dans
  `Guichet.swift`) : les clés des agents, les jetons du guichet, la clé privée P-256 du compte, la clé du séquestre, et
  les places avec leurs clés de sauvegarde. Ces éléments ne vont ni dans le trousseau iCloud ni dans les sauvegardes
  transférables vers un autre appareil.
- Android : les mêmes secrets dans les préférences privées de l’app, chiffrés avec une clé AES qui reste dans
  l’Android Keystore (`model/Secrets.kt`). `allowBackup="false"`.
- Les caches de conversation, les enregistrements des notes vocales et les voix téléchargées restent dans le stockage
  propre à l’app. Le cache du jeton de relais et les empreintes de santé sont dans UserDefaults ou SharedPreferences.

## 8. Le mode démo

La démo (`App/Features/Demo/`, `AgentStore.tour`, `ui/DemoViews.kt`) utilise le fichier `demo.json` et les
enregistrements fournis avec l’app. Ses agents pointent vers `https://demo.invalid`. En démo, l’app ne demande ni les
notifications, ni la synchronisation de la santé, des rappels ou des sauvegardes, ni le compte, ni les vérifications de
places du guichet (`BipAgentsApp.swift`, `HealthSync`) ; les conversations et les appels Live jouent des répliques
enregistrées. Les écrans de l’offre ne demandent rien au guichet (`OfferFlow.swift`, `OfferScreen.kt`) : les prix
viennent de la boutique seule (produits et storefront StoreKit, produits et pays de Play Billing, ou région du
téléphone).

## 9. Limites et écarts, sans détour

- **Le code de comparaison d’un appareil rend la substitution de clé coûteuse, pas impossible.** Les clés publiques des
  appareils viennent toujours du guichet. Un guichet malhonnête qui donnerait sa propre clé pour « le nouvel appareil »
  devrait trouver une clé dont le code est le même que celui de la vraie (12 chiffres, 10 000 tours de SHA-256 par
  essai), et le code ne protège que si la personne le compare. Un échange complet « engagement puis révélation », où
  aucun des deux côtés ne peut choisir sa clé après avoir vu celle de l’autre, est prévu.
- **Les invitations familiales n’ont pas de code de comparaison.** Elles sont scellées pour les clés des appareils de
  l’autre personne telles que le guichet les donne (`POST /v1/account/family/devices`) ; un guichet malhonnête pourrait
  donner sa propre clé et ouvrir l’invitation.
- Une récupération sans le code (après 24 heures) permet au guichet de faire réappairer une place avec un nouvel
  appareil. C’est voulu, et c’est un pouvoir que détient le guichet.
- Sur une place de l’offre, le serveur est installé pour Kerfy Studio et tourne chez un hébergeur. Le chiffrement de
  l’app ne protège pas contre celui qui exploite ce serveur.
- **Invitations à une place partagée :** le guichet détient le secret d’appairage, la clé d’IA et le jeton de la place
  (c’est lui qui les a créés) et les donne, en HTTPS, à l’app connectée avec l’adresse invitée. Qui peut se connecter
  avec cette adresse (par exemple en lisant ses codes reçus par e-mail) peut prendre la place tant qu’elle n’est pas
  appairée.
- **« Partager le lien » d’une place** (Duo, Famille) met toujours l’adresse, le port, le secret d’appairage, la clé
  d’IA et le jeton de la place, en base64 et sans chiffrement, après le `#` du lien. C’est la personne qui choisit à
  qui l’envoyer ; le lien ne sert qu’une fois.
- **« Signaler cette réponse »** envoie le texte de la réponse à Kerfy Studio (par le relais jusqu’au guichet, ou par
  e-mail si le relais ne répond pas), seulement quand la personne envoie le signalement (§3).
- **La dictée en ligne, une fois acceptée,** envoie la voix à Apple ou au reconnaisseur par défaut du téléphone (en
  général Google) pour les langues que le téléphone ne sait pas reconnaître seul (§4). Sur Android 12 et avant, l’app
  ne peut pas vérifier la reconnaissance sur l’appareil : la dictée n’y fonctionne qu’avec ce oui.
- **Le scanner de documents ML Kit** sur Android peut envoyer des données d’usage et de diagnostic à Google, selon
  Google. L’app ne modifie pas ce comportement.
- **« Santé » et le modèle :** la vérification « hébergé dans l’UE » est faite par les apps et repose sur la marque
  `eu_hosted` du guichet. Elle a lieu quand on active l’interrupteur et quand on change le modèle dans l’app ; le bridge
  ne la fait pas, donc un modèle changé autrement ne désactive pas « Santé ».
- **L’adresse e-mail de Kerfy Studio d’un agent** garde ses e-mails sur le serveur de messagerie de Kerfy Studio (un
  serveur chez Hetzner, à part des places), stockés comme sur tout serveur de messagerie, sans chiffrement pour la
  personne : celui qui exploite ce serveur peut les lire. Les e-mails sont effacés au bout de 90 jours, et la boîte
  entière quand l’adresse disparaît. Entre serveurs de messagerie, un e-mail n’est chiffré pendant le trajet que si
  l’autre serveur le permet.
- **Le code du guichet est privé.** Ce qu’il garde et fait de ce qu’il reçoit ne peut pas être vérifié dans un code
  public. Le relais et le bridge sont publics.
- **Le code des apps est privé lui aussi :** cette page et les fichiers publics `AccountCrypto` sont ce qui peut être
  vérifié ; le reste décrit un code que les lecteurs ne peuvent pas voir.
