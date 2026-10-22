[English](privacy.en.md) · Français · [Deutsch](privacy.de.md) · [Español](privacy.es.md)

# Politique de confidentialité de BipAgents

> Dernière mise à jour : [date].

## En bref

- BipAgents relie ton téléphone à **tes propres agents IA**, construits sur Hermes Agent. Ils tournent soit **sur ton
  ordinateur ou ton serveur**, soit, avec l'**offre tout compris**, sur **ta place**, un compte sur un serveur que Kerfy
  Studio loue à un hébergeur partenaire. Tes conversations, tes fichiers, ta voix et la mémoire de tes agents restent
  sur ce serveur et chez le fournisseur de modèle d'IA qui leur répond. **Kerfy Studio, l'éditeur de l'app, ne les lit
  pas.**
- Pas de publicité, pas de statistiques d'usage, pas de pistage. Pas de compte avec ton propre serveur ; un compte pour
  l'offre tout compris.
- Avec ton propre serveur, Kerfy Studio ne peut recevoir que trois choses :
  1. de quoi acheminer tes **notifications** par son relais : un jeton d'appareil scellé, jamais le texte de tes
     messages ;
  2. ce que tu choisis de nous envoyer : un **e-mail** au support, ou le **signalement** d'une réponse depuis l'app ;
  3. des **rapports de plantage**, seulement si tu as activé leur partage dans les réglages de ton téléphone.
- Avec l'offre tout compris, Kerfy Studio traite aussi ton compte, l'état de ton abonnement, des données techniques sur
  ta place (jamais son contenu) et tes sauvegardes, chiffrées avec une clé qu'il n'a pas ; il héberge ta place sans y
  accéder (section 5).

## 1. Qui est responsable

**Kerfy Studio**, SAS au capital de [capital] €, dont le siège est situé [adresse du siège], immatriculée au RCS de
[ville du greffe] sous le numéro [SIREN] (« Kerfy Studio », « nous »). Contact : **support@kerfystudio.com**.

Kerfy Studio n'a pas désigné de délégué à la protection des données : ce n'est pas obligatoire pour son activité
(article 37 du RGPD). Pour toute question sur tes données : support@kerfystudio.com. Pour la loi de Singapour (Personal
Data Protection Act), qui demande une personne chargée de la protection des données, c'est le président de Kerfy
Studio, [Prénom Nom], joignable à la même adresse. Il est aussi la personne responsable de la protection des
renseignements personnels au sens de la loi du Québec, et la personne chargée de veiller au respect de la loi
canadienne (Loi sur la protection des renseignements personnels et les documents électroniques), à la même adresse.

Kerfy Studio est responsable des traitements que l'app réalise pour fonctionner, tels qu'il les a conçus
(sections 3, 4 et 7), même lorsqu'ils restent sur ton téléphone et qu'il n'y a pas accès, et de ceux de l'offre tout
compris (section 5). Quand tu utilises ton propre serveur, il n'est ni responsable ni sous-traitant des données que
traitent ce serveur, tes agents et ton fournisseur de modèle (section 2).

## 2. Comment marche BipAgents, et qui voit quoi

Avec ton propre ordinateur ou ton propre serveur :

| Où | Ce qui s'y trouve | Qui en décide |
|---|---|---|
| **Ton serveur** (ton ordinateur, ou un serveur que tu loues) | Hermes Agent, tes agents, tes conversations, tes fichiers, leur mémoire, leurs tâches planifiées, le coffre à mots de passe | Toi |
| **Ton fournisseur de modèle** (une API dans le cloud, par exemple OpenRouter, ou un modèle qui tourne chez toi) | Chaque message que tes agents lui envoient pour y répondre | Toi, selon les conditions de ce fournisseur |
| **Ton téléphone** | Ce que l'app garde pour fonctionner (section 3) | Toi |
| **Kerfy Studio** | Uniquement ce qui est décrit à la section 4 | Kerfy Studio |

- **BipAgents est construit sur Hermes Agent, logiciel libre de Nous Research**
  (https://github.com/NousResearch/hermes-agent), que tu installes sur ta propre machine. L'app s'y connecte
  directement, par **Tailscale** (un réseau privé entre tes appareils) ou par ton **réseau local**. Kerfy Studio
  n'héberge pas tes agents et n'a aucun accès à ton serveur. Pour ces données, Kerfy Studio n'est ni responsable de
  traitement ni sous-traitant.
- **Si tu loues un serveur**, son hébergeur a techniquement accès à son disque. Choisis-en un en qui tu as confiance
  (par exemple dans l'Union européenne).
- **Ton fournisseur de modèle** reçoit les messages auxquels il répond et les traite selon ses propres conditions et
  sa propre politique de confidentialité. Certains proposent de ne rien conserver (« zero data retention ») : c'est
  à toi de le choisir. Avec un modèle qui tourne chez toi, rien ne sort de ton réseau.
- **Les services que tu relies à tes agents** (messagerie, agenda, sites web, maison connectée…) reçoivent ce que tes
  agents leur envoient, selon leurs propres règles.
- **Plusieurs personnes sur un même serveur** (fonction famille) : chaque personne a sa propre installation, séparée
  des autres. La personne qui administre le serveur en est responsable vis-à-vis des autres.

**Avec l'offre tout compris**, ta place remplace ton serveur, et Kerfy Studio choisit l'hébergeur et le routeur d'IA :
la section 5 dit qui voit quoi.

## 3. Ce que l'app garde sur ton téléphone

Ces données restent sur ton téléphone et ne sont **pas** envoyées à Kerfy Studio. Ce qui est dit ici de l'iPhone vaut
aussi pour l'iPad.

- **Tes agents** : leur nom, leurs adresses sur ton serveur, l'empreinte du certificat de ton réseau local, leurs
  réglages (mascotte, voix, langue).
- **Les clés d'accès à tes agents** : dans le Trousseau de l'iPhone (limitées à cet appareil) ou chiffrées par une clé
  du Keystore d'Android.
- **Une copie de tes conversations récentes**, pour les lire hors connexion, tes brouillons, les messages en cours
  d'envoi et les réponses arrivées pendant ton absence (la Boîte).
- **Tes notes vocales** (enregistrements et transcriptions), les fichiers que tu envoies et ceux que tes agents
  t'envoient.
- **Le widget**, si tu l'ajoutes, affiche le dernier message d'un agent, y compris sur l'écran verrouillé de l'iPhone
  si tu l'y places.
- **Sur iPhone**, les notifications de tes agents s'affichent comme des messages : iOS garde sur l'appareil le nom de
  l'agent et le texte affiché, comme pour une messagerie.
- **Le navigateur de l'agent** (vue Bureau) : sur iPhone, rien n'en est conservé ; sur Android, ses cookies restent
  dans l'app.

**Pourquoi.** Ces traitements servent uniquement à te fournir les fonctions que tu utilises (article 6.1.b du RGPD).
Ces données restent sur ton téléphone jusqu'à ce que tu supprimes l'agent ou l'app.

**Sauvegardes.** Sur iPhone, ces données font partie de ta sauvegarde iCloud ou de celle de ton ordinateur si tu
l'as activée, sauf les clés, qui restent sur l'appareil. Sur Android, l'app exclut ses données des sauvegardes ; un
transfert direct vers un nouveau téléphone peut en copier une partie, mais pas les clés, qui y restent illisibles.

**Effacer.** Supprime un agent dans l'app, ou désinstalle l'app pour tout effacer du téléphone. Cela n'efface pas ce
qui se trouve sur ton serveur.

**Partages que tu déclenches.** Le rapport de diagnostic (Réglages › Diagnostics et latences › Exporter le rapport)
contient le nom et l'adresse de tes agents ; il ne part que vers l'app ou la personne que tu choisis dans la feuille
de partage.

**Ta santé, si tu l'actives (« Santé »).** Pour chaque agent, tu peux activer « Santé » dans ses outils. Avec ton
autorisation dans Apple Santé (iPhone, y compris les données de ton Apple Watch) ou Health Connect (Android), l'app
lit, sans jamais rien y écrire, un résumé de chaque journée, **seulement pour les catégories que tu as choisies pour
cet agent** : activité (pas, énergie active, minutes d'exercice, séances avec leur type, durée, distance et énergie,
distance à pied et en courant, étages montés, heures debout) ; sommeil (temps endormi et au lit, heures de coucher et de
lever, phases du sommeil, respiration et température du poignet la nuit) ; cœur (fréquence cardiaque au repos,
variabilité cardiaque, pouls moyen, le plus bas et le plus haut de la journée, oxygène dans le sang, VO2 max) ; corps
(poids, masse grasse, IMC, tension) ; alimentation et eau (calories mangées, eau, caféine) ; bien-être (minutes de
pleine conscience, humeurs notées sur iPhone) ; et, **désactivées tant que tu ne les actives pas une par une**, la
glycémie et le cycle (flux des règles, début des règles), plus sensibles. Un agent qui reçoit « Santé » commence avec
activité, sommeil et cœur ; « Tout partager » laisse la glycémie et le cycle de côté. Certaines données n'existent que
sur iPhone (heures debout, température du poignet, IMC, humeurs). Elle envoie les 30 derniers jours la première fois,
puis les jours qui ont changé, **uniquement à ton serveur** (ou à ta place avec l'offre tout compris), où ils sont
gardés un fichier par jour, 400 jours au plus, avec seulement les catégories qu'au moins un agent y lit (une catégorie
que plus aucun agent ne lit est effacée des journées gardées). Chaque agent ne lit que ses catégories, pour t'aider sur
ton sommeil, ton activité, ton entraînement et tes habitudes ; ils ne posent pas de diagnostic. Le modèle d'IA de l'agent reçoit ce que l'agent lit pour te répondre : avec l'offre tout compris, « Santé »
n'est possible qu'avec un modèle qui tourne dans l'Union européenne. Le téléphone ne garde que les dates et une
empreinte de ce qu'il a déjà envoyé, jamais les valeurs, et rien dans iCloud. Kerfy Studio ne les reçoit pas ; rien
n'est vendu ni utilisé pour de la publicité. **Base légale** : ton consentement explicite (articles 6.1.a et 9.2.a du
RGPD), que tu donnes en activant « Santé » et en l'autorisant dans Apple Santé ou Health Connect. **Arrêter** :
désactive « Santé » ; quand plus aucun agent ne l'a, les journées gardées sur ton serveur sont effacées. Elles le sont
aussi avec l'agent, avec ta place, ou en retirant l'autorisation dans Apple Santé ou Health Connect (plus rien n'est
envoyé).

**Ton agenda, si tu le partages (« Calendrier »).** Pour chaque agent, tu peux activer « Calendrier » dans ses
outils. Avec ton autorisation (Calendriers sur l'iPhone, Agenda sur Android), l'app lit les agendas de ton téléphone,
y compris les comptes ajoutés dessus (Google, Outlook…), et envoie une copie de leurs rendez-vous, d'une semaine avant
à deux mois après aujourd'hui (titre, horaires, lieu, notes), **uniquement à ton serveur** (ou à ta place avec l'offre
tout compris), quand l'app s'ouvre. Cette copie y reste, remplacée à chaque envoi. Seuls les agents que tu as choisis
la lisent ; quand ils te demandent d'ajouter, de modifier ou de supprimer un rendez-vous, c'est ton téléphone qui le
fait. Le modèle d'IA de l'agent reçoit les rendez-vous que l'agent lit pour te répondre. Kerfy Studio ne les reçoit
pas. **Base légale** : ton consentement (article 6.1.a du RGPD), que tu donnes en activant « Calendrier » et en
l'autorisant sur ton téléphone. **Arrêter** : désactive « Calendrier » pour l'agent, ou retire l'autorisation dans les
réglages du téléphone (plus rien n'est envoyé). Pour une boîte mail que tu as connectée, tu peux aussi activer
« Utiliser aussi son agenda » : ton serveur lit alors les agendas de ce compte (CalDAV, avec les identifiants de la
boîte, qui restent chiffrés sur ton serveur) quand un agent de cette boîte les consulte, et y ajoute ou supprime les
rendez-vous qu'il demande ; rien n'en est gardé à part la liste de ces agendas. Désactive-le pour arrêter.

**Tes contacts, si tu les partages (« Contacts »).** Pour chaque agent, tu peux activer « Contacts » dans ses outils.
Avec ton autorisation (Contacts sur l'iPhone et sur Android), l'app lit le carnet d'adresses de ton téléphone, y
compris les comptes ajoutés dessus (iCloud, Google…), et en envoie une copie **uniquement à ton serveur** (ou à ta
place avec l'offre tout compris), quand l'app s'ouvre : tout le carnet ou seulement les groupes (libellés) que tu
choisis pour ce serveur, avec pour chaque contact son nom, son surnom, ses numéros, ses adresses e-mail et postales,
son anniversaire, son entreprise et son poste, les personnes qui lui sont liées, ses groupes, et ses notes seulement si
tu coches « notes » (désactivé par défaut). Les photos ne sont jamais envoyées. Cette copie y reste, remplacée à chaque
envoi ; ce que ton choix ne couvre plus en est retiré aussitôt, et toute la copie est effacée quand plus aucun agent ne
peut la lire. Seuls les agents que tu as choisis la lisent ; quand ils te demandent d'ajouter ou de modifier un
contact, c'est ton téléphone qui le fait (rien n'est supprimé de cette façon). Le modèle d'IA de l'agent reçoit les
contacts que l'agent lit pour te répondre. Kerfy Studio ne les reçoit pas. **Tes contacts sont d'autres personnes** :
les agents ont pour consigne de ne jamais leur écrire, les appeler ou transmettre leurs coordonnées sans ton accord, et
tu restes responsable de l'usage que tu en fais. **Base légale** : ton consentement (article 6.1.a du RGPD), que tu
donnes en activant « Contacts » et en l'autorisant sur ton téléphone. **Arrêter** : désactive « Contacts » pour
l'agent, ou retire l'autorisation dans les réglages du téléphone (plus rien n'est envoyé).

**Les rappels par lieu et ta position.** Quand tu demandes à un agent « rappelle-moi le lait en arrivant au
supermarché », il demande un rappel avec le nom du lieu (ou ses coordonnées). Sur l'iPhone, c'est l'app Rappels
d'Apple qui surveille le lieu : BipAgents n'a pas accès à ta position ; le nom du lieu est cherché dans Plans d'Apple
pour trouver ses coordonnées. Sur Android, qui n'a pas d'app Rappels, BipAgents garde lui-même ces rappels : il te
demande la permission de position (y compris en arrière-plan) seulement quand un agent en a créé un, cherche le lieu
avec le service de géocodage du téléphone, et confie le cercle autour du lieu aux services de localisation de Google
Play sur le téléphone (geofencing). Ta position reste sur le téléphone : elle n'est envoyée ni à ton serveur, ni à
l'agent, ni à Kerfy Studio, et ne sert qu'à te prévenir en arrivant ou en partant. **Base légale** : ton consentement
(article 6.1.a du RGPD), que tu donnes en autorisant la position. **Arrêter** : supprime le rappel (Réglages › Rappels
par lieu), ou retire la permission de position dans les réglages du téléphone.

**Ce que tu partages vers l'app.** Quand tu partages un message ou une capture d'écran vers BipAgents (depuis
WhatsApp, Signal, Messages…), il va à l'agent que tu choisis, sur ton serveur, comme une pièce jointe que tu envoies
toi-même ; il peut contenir les mots d'autres personnes, partage seulement ce dont tu as besoin. Le brouillon de
réponse que l'agent écrit n'est jamais envoyé par l'app : le bouton « Envoyer avec… » ouvre l'app de ton choix, et
c'est toi qui envoies.

## 4. Ce que Kerfy Studio peut recevoir

### 4.1 Les notifications, par le relais de Kerfy Studio

**Pourquoi un relais.** Pour t'envoyer une notification, ton serveur doit passer par Apple (iPhone) ou Google
(Android), avec des clés qui appartiennent à l'éditeur de l'app et qui ne peuvent pas être confiées à chaque serveur.
Le relais de Kerfy Studio les détient et transmet les notifications de ton serveur.

**Comment.** Quand tu autorises les notifications, Apple ou Google donne à l'app un **jeton d'appareil**. L'app
l'envoie au relais, qui le lui rend **scellé** (chiffré avec une clé que seul le relais détient). Ton serveur garde ce
jeton scellé et l'envoie au relais avec chaque notification ; le relais l'ouvre, transmet à Apple ou Google, puis
oublie tout.

**Ce que le relais traite :**

- le jeton d'appareil, la plateforme (Apple ou Google) et l'environnement (test ou production) ;
- l'**adresse IP** de ton téléphone (à l'inscription) ou de ton serveur (à chaque notification), uniquement pour
  limiter les abus : elle est transformée en empreinte, gardée en mémoire une heure au plus, jamais écrite ;
- le contenu de la notification, réduit au **nom de l'agent** (le titre), à un **texte générique** comme « Nouveau
  message » (tout autre texte est remplacé) et à des **identifiants techniques** (agent, conversation, tâche, type de
  notification). **Jamais le texte de tes messages** : l'app va le chercher elle-même sur ton serveur.

**Ce qui est conservé.** Le relais n'a pas de base de données. Le jeton scellé est conservé sur ton téléphone et sur
ton serveur ; avec l'offre tout compris, le guichet de Kerfy Studio le garde aussi, toujours scellé (section 5.6). Les
journaux techniques du relais contiennent la plateforme, une empreinte de 4 octets du jeton et la réponse d'Apple ou
de Google ; ils sont conservés **6 mois au plus**. L'hébergeur du relais peut journaliser les adresses IP des
requêtes : [ce que l'hébergeur du relais journalise, et pendant combien de temps].

**Finalités et bases légales.** Acheminer les notifications que tu as activées : exécution des conditions
d'utilisation (article 6.1.b du RGPD). Limiter les abus et protéger le service : intérêt légitime de Kerfy Studio
(article 6.1.f).

**Sous-traitants.** [hébergeur du relais] (Union européenne) et Google (Firebase Cloud Messaging), liés à Kerfy
Studio par un contrat conforme à l'article 28 du RGPD. Apple achemine les notifications vers les iPhone par son
service APNs.

**Sans notifications.** Tu peux refuser ou couper les notifications dans les réglages de ton téléphone : l'app
fonctionne sans elles. Si tu désinstalles l'app, Apple ou Google signalent le jeton comme invalide et ton serveur
l'oublie. Si tu compiles l'app toi-même avec ton propre compte Apple, le relais de Kerfy Studio n'est pas utilisé.

**Sur Android**, l'app ne demande un jeton à Google qu'au moment où tu actives les notifications.

### 4.2 Le support et les signalements

- **Réglages › Contacter le support** prépare un e-mail à support@kerfystudio.com avec un diagnostic que tu peux lire
  avant l'envoi : version de l'app, modèle et version du téléphone, langue, style d'affichage, mode démo ou non, état
  des notifications et, pour chaque agent, sa catégorie, s'il répond, s'il a un bridge et une porte sur le réseau
  local, et sa voix. **Aucune conversation, aucun nom d'agent, aucune adresse, aucune clé.** Rien ne part tant que tu
  n'as pas appuyé sur « Envoyer » dans ton app de mail. Nous recevons aussi ton adresse e-mail et tout ce que tu
  choisis d'écrire : évite d'y mettre des informations sensibles.
- **Signaler cette réponse** (appui long sur une réponse) envoie le signalement **depuis l'app**, par le relais de
  Kerfy Studio, à Kerfy Studio seulement : ni à ton serveur, ni au fournisseur du modèle. Il contient le motif, tes
  précisions éventuelles, **le texte de la réponse signalée**, la catégorie de l'agent, la version de l'app et le
  modèle du téléphone. **Ni ton adresse e-mail, ni ton nom, ni celui de l'agent.** Rien ne part tant que tu n'as pas
  appuyé sur « Envoyer ». Le relais voit au passage l'adresse IP de ton téléphone et ne s'en sert que pour limiter
  les abus (section 4.1). Comme le signalement ne contient aucun moyen de te joindre, nous ne pouvons pas y
  répondre : écris au support si tu attends une réponse.
- **Si le relais ne répond pas**, l'app te propose d'envoyer le même signalement par e-mail : nous recevons alors
  aussi ton adresse e-mail.

Le texte de la réponse signalée peut parler de ta santé ou d'autres personnes : relis-le avant d'envoyer, et n'écris
dans les précisions que ce qui est utile. Nous effaçons ce qui ne sert pas à traiter le signalement.

**Finalités.** Te répondre, traiter les signalements, corriger les bugs et améliorer l'app.
**Bases légales.** Répondre à tes messages et traiter tes signalements : intérêt légitime de Kerfy Studio à te
répondre et à rendre l'app plus sûre (article 6.1.f du RGPD). Répondre à une demande d'exercice de tes droits :
obligation légale (article 6.1.c ; articles 12 à 22 du RGPD).
**Durée.** Support : 2 ans après le dernier échange. Signalements : 1 an après leur traitement.
**Destinataires.** Kerfy Studio et ses sous-traitants : son prestataire de messagerie ([prestataire de messagerie],
[pays]) et l'hébergeur du relais, par lequel passent les signalements.

### 4.3 Les rapports de plantage

Si tu as activé, dans les réglages de ton téléphone, le partage des données d'analyse avec les développeurs d'apps
(iPhone : Réglages › Confidentialité et sécurité › Analyse et améliorations › Partager avec les développeurs d'apps ;
Android : partage des données d'utilisation et de diagnostic), Apple ou Google peuvent transmettre à Kerfy Studio des
rapports de plantage et des mesures techniques (modèle, version du système, endroit du code où l'app a planté).
L'app ne contient aucun outil de statistiques ou de rapport de plantage de tiers : ces rapports viennent uniquement
d'Apple ou de Google.

**Finalité.** Corriger les bugs.
**Base légale.** Intérêt légitime de Kerfy Studio à corriger les bugs (article 6.1.f du RGPD). Ces rapports ne nous
parviennent que si tu as activé ce partage dans les réglages de ton téléphone, où tu peux le couper à tout moment.
Apple et Google les collectent sous leur propre responsabilité.
**Durée.** Kerfy Studio supprime les rapports qu'il télécharge au plus tard 1 an après leur réception.

### 4.4 Les statistiques des stores

Apple et Google fournissent à Kerfy Studio des statistiques sur l'app (téléchargements, pays, versions). Apple et
Google les produisent sous leur propre responsabilité et nous les fournissent agrégées : elles ne permettent pas de
t'identifier.

## 5. L'offre tout compris

Cette section concerne les personnes qui utilisent l'offre tout compris ([conditions de l'offre](offer.fr.md)) : une
place sur un serveur que Kerfy Studio loue à un hébergeur partenaire, un crédit d'IA, des sauvegardes et un compte. Le
reste de cette politique s'applique aussi (l'app sur ton téléphone, les notifications, le support). Le service de
Kerfy Studio qui gère les comptes, les abonnements et les places s'appelle le guichet.

### 5.1 Qui est responsable de quoi

- **Kerfy Studio est responsable** des traitements que demande l'offre : ton compte, l'état de ton abonnement,
  l'hébergement de ta place, l'envoi de tes messages au modèle d'IA, les sauvegardes, la surveillance technique et le
  traitement des abus.
- **Le contenu de ta place** (tes agents, tes conversations, tes fichiers, leur mémoire, leurs routines, le coffre à
  mots de passe) : c'est toi qui décides de ce que tu y mets. Kerfy Studio l'héberge et le fait traiter par le modèle
  d'IA uniquement pour te fournir le service (article 6.1.b du RGPD), **sans y accéder** : son logiciel ne le lit pas (les seuls ordres qu'un serveur exécute sont réinstaller une place, la mettre à jour vers une version publiée du code source ouvert de BipAgents, ou la rouvrir, et aucun ne lit son contenu). L'accès aux comptes de Kerfy Studio chez les hébergeurs est réservé à la société. Un
  accès au contenu d'une place n'aurait lieu que pour répondre à une obligation légale (une réquisition judiciaire, par
  exemple), et tu en serais informé quand la loi le permet. Comme tout hébergeur, OVHcloud et Hetzner ont un accès
  physique à leurs serveurs. Si tu y mets des données sensibles (ta santé, par exemple), elles ne sont traitées que pour
  exécuter ce que tu demandes à tes agents.
- **Apple et Google** sont responsables de leurs propres traitements quand tu te connectes avec eux et quand tu paies
  dans leur store.
- **Tes agents sont des IA et le disent** : badge « IA », annonce au début de chaque session Live, mention dans les
  notifications et dans les messages qu'ils envoient à d'autres personnes (règlement (UE) 2024/1689, article 50).

### 5.2 Ton compte

**Données.** L'identifiant que te donne Apple ou Google et ton adresse e-mail (avec Apple, ce peut être une adresse
relais) ; ou, avec une adresse e-mail, les codes à usage unique qui te sont envoyés. La liste de tes appareils
approuvés, la clé publique de chacun (ce qu'un autre de tes appareils lui envoie est scellé pour lui) et leurs jetons
de notification, scellés par le relais
(section 4.1) : Kerfy Studio ne les voit jamais en clair. Les invitations que tu envoies (l'adresse e-mail de la
personne invitée et leur état). Une demande de récupération en cours. La copie de ta clé de sauvegarde, chiffrée par ton
code de récupération, que Kerfy Studio ne peut pas lire. Le crédit d'IA acheté et gardé pour un prochain abonnement (un
montant). La date de suppression du compte, si tu l'as demandée. Pas ton nom, pas ta photo, jamais les clés de ta
place.

**Finalités.** Te connecter, relier ton abonnement et tes places, approuver tes appareils, récupérer ton compte,
t'envoyer les codes de connexion, les avis de sécurité (nouvel appareil, récupération en cours), la confirmation de ton
achat, l'accusé de réception d'une rétractation et les avis de suppression, gérer les invitations. Aucun e-mail
publicitaire : un message commercial ne partirait qu'avec ton accord coché.
**Base légale.** Exécution du contrat (article 6.1.b du RGPD).
**Durée.** Tant que ton compte existe. Quand tu le supprimes, il est effacé **30 jours** après ta demande (tes places
s'arrêtent dès la demande, et tu peux changer d'avis jusque-là), avec ses places et leurs sauvegardes, sauf ce que la
loi oblige à garder : comme hébergeur, Kerfy Studio garde à part, et seulement pour répondre aux autorités judiciaires,
ton adresse e-mail 5 ans et ton identifiant Apple ou Google 1 an après la fin du contrat (loi pour la confiance dans
l'économie numérique, article 6, II ; décret n° 2021-1362, articles 2, 3 et 8). Codes à usage unique : le temps de leur
validité. Invitation non acceptée : 30 jours.
**Supprimer ton compte.** Dans l'app, Réglages › Compte › Supprimer mon compte, ou, si tu n'as plus l'app, sur la page
https://guichet.kerfystudio.com/account/delete avec un code envoyé à ton adresse e-mail. Les personnes avec qui tu
partages ton forfait sont prévenues par e-mail : leurs places s'arrêtent le jour où ton compte est effacé.

### 5.3 Ton abonnement, tes achats et la semaine gratuite

**Données reçues des stores.** Par les notifications d'Apple (App Store Server Notifications) et de Google (Google Play,
notifications en temps réel) : l'identifiant de ton abonnement chez le store, le forfait, la région, son état (semaine
gratuite, actif, suspendu, terminé, remboursé), la fin de la période payée, la monnaie et le prix mensuel du forfait, si le renouvellement est coupé, et pour le
crédit d'IA acheté, l'identifiant de chaque achat et son montant. **Jamais tes moyens de paiement**, que seul le store
connaît. Le pays du compte de ton store, que l'app envoie pour savoir si l'offre y est vendue (il n'est pas gardé).

**Ton contrat.** La date de début, celle de l'e-mail de confirmation et, si tu renonces au contrat, la date et l'heure
de ta rétractation.

**Crédit d'IA utilisé.** Le montant que la clé d'IA de chaque place a dépensé dans le mois, lu chez le routeur d'IA
une fois par minute au plus, et le crédit reporté : un montant, jamais tes messages.

**Une seule semaine gratuite par appareil.**

- **Sur iPhone**, l'app utilise DeviceCheck d'Apple : Apple garde deux bits par appareil pour Kerfy Studio (« a déjà eu
  une semaine gratuite »). Kerfy Studio ne reçoit aucun identifiant de l'appareil : l'app lui donne un jeton à usage
  unique qu'il transmet à Apple.
- **Sur Android**, l'app envoie un jeton Google Play Integrity (Google confirme que l'app est authentique et tourne sur
  un vrai appareil) et une empreinte de l'identifiant Android de l'appareil, calculée sur le téléphone (SHA-256), puis
  hachée à nouveau par le guichet avec une clé secrète avant d'être enregistrée. Kerfy Studio ne garde que cette
  seconde empreinte, et seulement pour cette limite.
- **L'adresse IP** de ton téléphone est gardée le temps de la réservation d'une place d'essai (15 minutes au plus),
  pour limiter le nombre de réservations par adresse.

**Finalités et bases légales.** Fournir et gérer ton abonnement, tes places et ton crédit : exécution du contrat
(article 6.1.b du RGPD). Empêcher qu'une même personne enchaîne les semaines gratuites : intérêt légitime de Kerfy
Studio à prévenir la fraude et à pouvoir continuer d'offrir une semaine gratuite (article 6.1.f et considérant 47).
Lire l'identifiant de l'appareil ne demande pas ton consentement, car cela sert uniquement à limiter l'accès gratuit
au service à une période limitée (article 82 de la loi Informatique et Libertés ; CNIL, délibération n° 2020-091,
article 5). Garder la preuve du contrat : intérêt légitime de Kerfy Studio à pouvoir défendre ses droits.
**Durée.** Données d'abonnement et d'achat : pendant l'abonnement, puis 5 ans en archive, pour la preuve du contrat
(article 2224 du Code civil). Empreinte de l'appareil Android : 24 mois. Adresse IP :
15 minutes au plus. Les bits de DeviceCheck sont gardés par Apple.

### 5.4 Ta place et le modèle d'IA

**Où.** Sur un serveur loué à OVHcloud (centres de données dans l'Union européenne) ou à Hetzner (Allemagne ou
Finlande), pour toutes les personnes de l'offre, y compris celles qui vivent hors de l'Union.

**Ce qui s'y trouve.** Ce que décrit la section 5.1, et les clés SSH publiques que tu ajoutes depuis l'app. Le guichet
de Kerfy Studio (le service qui gère les places) connaît l'adresse de ta place et l'empreinte de son certificat, pour
que ton téléphone s'y connecte et pour la déplacer en cas de panne.

**Le modèle d'IA.** Les messages que tes agents envoient au modèle (avec les fichiers et les photos qu'ils lui
montrent) passent par un routeur d'IA, **EUrouter** (EUrouter B.V., Amsterdam, Pays-Bas), pour toutes les personnes de
l'offre, puis par le fournisseur qui fait tourner le modèle choisi : [à compléter : fournisseurs des modèles proposés et
pays où ils tournent]. EUrouter indique ne garder par défaut ni les requêtes ni les réponses, et garder leurs
métadonnées (heure, modèle, nombre de jetons, coût, adresse IP) jusqu'à 12 mois. Ils agissent pour le compte de Kerfy
Studio. **Avant le premier message, l'app nomme le routeur et le fabricant du modèle et te demande ton accord** ; la
liste des modèles indique le fabricant de chacun.

**La mise en veille.** Ta place note elle-même la dernière demande de l'app ; le guichet sait seulement qu'elle dort.

**Base légale.** Exécution du contrat (article 6.1.b du RGPD).
**Durée.** Pendant l'abonnement. Quand il prend fin, ta place est gardée 30 jours (3 jours après une semaine gratuite
non poursuivie), puis son compte est effacé et réinstallé pour quelqu'un d'autre. Les places en trop d'un forfait
réduit sont effacées 30 jours après s'être arrêtées.

### 5.5 Les sauvegardes

Ta place se sauvegarde toutes les heures. La sauvegarde est **chiffrée sur la place**, avant de partir, avec une clé
que seuls tes appareils détiennent, plus une copie de cette clé chiffrée par ton code de récupération (section 5.2).
Kerfy Studio garde ces sauvegardes chez [à compléter : hébergeur des sauvegardes] sans pouvoir les lire : une par heure
pendant 48 heures, une par jour pendant 30 jours, une par semaine pendant 12 semaines. Elles sont effacées avec ta
place.

**L'image quotidienne d'OVHcloud.** Sur ses serveurs, OVHcloud fait aussi une image quotidienne du serveur entier,
qu'il garde chez lui : un point de restauration par jour, qui n'est pas chiffré par Kerfy Studio. Quand une place est
réinstallée pour quelqu'un d'autre, l'image qui la contenait est remplacée dans la journée environ.

**Base légale.** Exécution du contrat (article 6.1.b du RGPD).

### 5.6 La surveillance technique et les pannes

Toutes les deux minutes, chaque serveur envoie au guichet : sa mémoire, son disque et sa charge, et pour chaque place,
si son bridge répond, si son Hermes Agent tourne ou dort, sa version de BipAgents et l'heure de sa dernière
sauvegarde réussie. **Jamais de contenu.** Ces données servent à repérer une panne, à déplacer les places, à faire les
mises à jour par vagues et à prévenir Kerfy Studio.

Quand le serveur de ta place tombe en panne, est déplacé ou revient, tes téléphones sont prévenus par le relais de
notifications, avec un texte générique : pour cela, le guichet garde les jetons scellés de tes téléphones liés à ta
place.

**Base légale.** Exécution du contrat (article 6.1.b du RGPD).
**Durée.** Seules les dernières valeurs sont gardées, remplacées à chaque envoi. Journal des événements (pannes,
déplacements, mises à jour) : 12 mois. Jetons scellés : tant que
l'appareil est relié à ta place.

### 5.7 Les signalements d'abus et les demandes des autorités

**Données.** Le signalement (qui l'envoie et comment le joindre, les faits, l'adresse IP et l'heure), la place
concernée, notre décision et ses motifs, nos échanges avec toi.
**Finalités.** Traiter les signalements ([politique d'usage acceptable](acceptable-use.fr.md)), protéger les serveurs et
les autres personnes, répondre aux autorités.
**Bases légales.** Obligation légale (article 6.1.c du RGPD ; règlement (UE) 2022/2065, articles 16 à 18) et intérêt
légitime de Kerfy Studio à protéger son service et ses clients (article 6.1.f).
**Durée.** 1 an après leur traitement, plus longtemps si un litige ou une procédure l'exige.

### 5.8 Les prestataires de l'offre

| Prestataire | Rôle | Données | Où |
|---|---|---|---|
| OVHcloud (OVH SAS, Roubaix, France) | Héberge les serveurs des places ; image quotidienne de chaque serveur | Contenu des places | Union européenne |
| Hetzner Online GmbH (Gunzenhausen, Allemagne) | Héberge des serveurs de places | Contenu des places | Allemagne, Finlande |
| EUrouter B.V. (Amsterdam, Pays-Bas) | Routeur d'IA | Messages envoyés au modèle, métadonnées | Union européenne |
| Fournisseurs des modèles : [à compléter] | Font tourner le modèle choisi | Messages envoyés au modèle | [à compléter] |
| [à compléter : hébergeur des sauvegardes] | Garde les sauvegardes | Sauvegardes chiffrées, illisibles pour lui et pour Kerfy Studio | [à compléter] |
| [hébergeur du relais] | Héberge le guichet et le relais | Compte, abonnement, données techniques | Union européenne |
| [à compléter : prestataire d'envoi des e-mails] | Envoie les codes, les avis de sécurité, les invitations, la confirmation d'achat, l'accusé de rétractation et les avis de suppression | Adresse e-mail, contenu de ces e-mails | [à compléter] |

Ils sont liés à Kerfy Studio par un contrat conforme à l'article 28 du RGPD. Apple (DeviceCheck, App Store) et Google
(Play Integrity, Google Play) interviennent aussi, comme dit à la section 5.3 ; « Se connecter avec Apple » et la
connexion avec Google relèvent de leur propre responsabilité.

## 6. Ce que Kerfy Studio ne fait pas

- Pas de publicité, pas d'outil de statistiques, pas de pistage entre apps ou sites. Pas de compte, sauf pour l'offre
  tout compris.
- Pas de vente ni de location de données.
- Pas d'entraînement de modèles d'IA sur tes données par Kerfy Studio.
- Pas d'accès à ton serveur, au contenu de ta place, à tes conversations ni à tes fichiers.

## 7. Les autres services que l'app fait intervenir

En plus de ton serveur, l'app fait appel à ces services, qui appliquent leurs propres règles. Kerfy Studio ne reçoit
rien de ces échanges. Il a choisi de les confier à l'app et répond de ce qui part de ton téléphone ; chaque service
est ensuite responsable de ce qu'il en fait.

- **Icônes des sites.** Pour afficher la petite icône des sites cités dans les réponses, l'app demande ces icônes à
  ton serveur, qui les récupère auprès de DuckDuckGo (icons.duckduckgo.com) : DuckDuckGo reçoit le **nom du site**
  (par exemple « exemple.fr ») et voit l'adresse de ton serveur, pas celle de ton téléphone. Il ne reçoit pas le
  message.
- **Météo et itinéraires.** Quand un agent a « Météo » et que tu lui demandes le temps qu'il fera, ton serveur (ou
  ta place) demande le lieu à OpenStreetMap (Nominatim, pour trouver la ville) puis les prévisions à l'Institut
  météorologique norvégien (MET Norway) : ils reçoivent **seulement le nom du lieu et ses coordonnées arrondies**
  (environ 1 km), et voient l'adresse de ton serveur, pas celle de ton téléphone ni ton message. Un itinéraire est un
  lien qui ouvre Plans (iPhone) ou Google Maps (Android) sur ton téléphone ; ils s'appliquent alors leurs propres
  règles. Si ton serveur a une clé Google Maps (pas l'offre tout compris pour l'instant), les lieux et l'heure de
  l'itinéraire partent aussi chez Google (Google LLC, possiblement aux États-Unis, certifiée au « EU-U.S. Data Privacy
  Framework ») pour en calculer la durée ; rien n'en est gardé.
- **Reconnaissance vocale.** Les notes vocales et le Live utilisent la reconnaissance vocale de ton téléphone. L'app
  demande la reconnaissance sur l'appareil quand ton téléphone la propose pour ta langue. Sinon, Apple ou Google (sur
  Android, le service de reconnaissance du système, souvent celui de Google) traitent l'audio sur leurs serveurs, sous
  leur propre responsabilité. C'est le texte transcrit qui part à ton serveur. Base légale : te fournir la fonction
  que tu utilises (article 6.1.b du RGPD).
- **Synthèse vocale du téléphone.** Quand la voix du téléphone lit un texte (dans la démo, ou si ton serveur ne
  répond pas), elle utilise le moteur de synthèse de ton téléphone, selon ses réglages.
- **Sur Android, la lecture des QR codes** se fait sur le téléphone, avec une bibliothèque intégrée à l'app, sans
  service Google.
- **Sur Android, le scanner de documents** est une fonction des services Google Play (Google ML Kit). Google indique
  que ML Kit lui envoie des informations sur l'appareil et l'app, des identifiants d'installation et des mesures
  d'usage et de performance, pour le diagnostic et ses statistiques. L'app te le dit et te demande ton accord avant
  la première utilisation du scanner ; sans cet accord, le scanner ne s'ouvre pas, et tu peux toujours joindre une
  photo ou un fichier. Tu peux changer d'avis à tout moment dans Réglages.
- **Apple et Google** pour les notifications (section 4.1).
- **Tailscale**, si tu l'utilises : une app distincte, avec son propre compte et sa propre politique de
  confidentialité.
- **Les liens que tu ouvres** (sites cités par tes agents, page de Hermes Agent) s'ouvrent dans ton navigateur.

## 8. La démo

La démo intégrée tourne entièrement sur ton téléphone : des agents et des données inventés, sans compte, sans
serveur, et rien n'est envoyé à Kerfy Studio. Tu peux l'ouvrir dès la première ouverture, avec « Seulement la démo ».
Son podcast est un fichier audio enregistré à l'avance et inclus dans l'app (voix : Fish Audio Drama 3, « Built with
Fish Audio »). Si tu parles à la démo, la reconnaissance vocale du téléphone s'applique (section 7).

## 9. Les voix

Les voix des Bips sont synthétisées **sur ton téléphone** quand il le permet : l'app télécharge les voix de ta langue
depuis l'App Store ou Google Play, et ton serveur (ou ta place) lui envoie le texte à lire, préparé. Sinon, elles sont
synthétisées sur ton serveur ou sur ta place. Si tu branches ton propre modèle de voix, c'est un serveur que tu fais
tourner toi-même. Kerfy Studio ne reçoit rien de ce que lisent les voix.

## 10. Liens partenaires (hébergement, plus tard)

L'app ou le site kerfystudio.com pourront proposer plus tard des hébergeurs par des liens partenaires (affiliation).
Si c'est le cas :

- ces liens seront signalés par la mention « Lien partenaire » ;
- ni l'app ni le site ne déposeront de traceur pour l'affiliation : le code partenaire figurera seulement dans
  l'adresse du lien ;
- si tu suis un tel lien, l'hébergeur pourra savoir que ta visite vient de BipAgents, et Kerfy Studio pourra recevoir
  une commission si tu souscris ;
- l'app n'enverra aucune donnée personnelle à l'hébergeur ; ce que tu lui donnes ensuite relève de sa propre
  politique de confidentialité.

Avant le premier lien partenaire, cette section précisera ce que Kerfy Studio reçoit des partenaires, et ce changement
sera annoncé comme indiqué à la section 17.

## 11. Transferts hors de l'Union européenne

Le relais de notifications est hébergé dans l'Union européenne ([hébergeur du relais]). Pour acheminer les
notifications, Apple et Google peuvent traiter le jeton et la notification hors de l'Union européenne, notamment aux
États-Unis :

- Google LLC (Firebase Cloud Messaging) est certifiée au « EU-U.S. Data Privacy Framework », reconnu comme offrant une
  protection adéquate par la décision d'exécution (UE) 2023/1795 de la Commission du 10 juillet 2023 ;
- Apple indique encadrer les transferts depuis l'Espace économique européen par les clauses contractuelles types
  adoptées par la Commission européenne.

**L'offre tout compris.** Les serveurs des places sont dans l'Union européenne, pour toutes les personnes de l'offre.
EUrouter traite les messages dans l'Union européenne [à compléter : pays où tournent les modèles proposés, et garantie
si l'un est hors de l'Union]. Apple et Google (connexion, stores, DeviceCheck, Play Integrity) peuvent traiter des
données aux États-Unis, avec les garanties indiquées ci-dessus.

**Si tu vis hors de l'Union européenne** (États-Unis, Royaume-Uni, Israël, Singapour, Canada), tes données viennent
dans l'Union européenne, où Kerfy Studio les traite selon le RGPD, qui s'applique à tous ses traitements. Le
Royaume-Uni et Israël reconnaissent l'Union européenne comme offrant une protection adéquate ; pour Singapour, ce
niveau de protection et nos contrats avec nos prestataires donnent la protection comparable que demande la loi de
Singapour.

**Si tu vis au Canada**, tes données sont donc conservées et traitées hors du Canada, et hors du Québec si tu y vis :
dans l'Union européenne pour ta place, ses sauvegardes et ton compte, et chez les prestataires nommés dans cette
section. Elles y sont soumises aux lois de ces pays, et les tribunaux et autorités de ces pays peuvent y avoir accès
dans les cas que leurs lois prévoient. Kerfy Studio reste responsable de tes données chez ses prestataires et les
encadre par contrat. Pour les personnes au Québec, Kerfy Studio évalue avant ce transfert les facteurs relatifs à la
vie privée (sensibilité des données, finalités, mesures de protection, y compris contractuelles, et régime juridique
du pays de destination) et ne fait le transfert que si cette évaluation montre une protection adéquate (article 17 de
la Loi sur la protection des renseignements personnels dans le secteur privé). Tu peux nous demander un résumé de
cette évaluation.

Prestataire de messagerie : [pays ; garantie]. Prestataire d'envoi des e-mails et hébergeur des sauvegardes : [à
compléter : pays ; garantie]. Tu peux nous demander une copie des garanties applicables.

## 12. Durées de conservation

| Données | Durée |
|---|---|
| Données de l'app sur ton téléphone | Jusqu'à ce que tu supprimes l'agent ou l'app |
| Données de santé (« Santé ») sur ton serveur ou ta place | 400 jours au plus ; effacées quand plus aucun agent n'a « Santé », avec l'agent ou avec la place |
| Jeton d'appareil au relais | Aucune conservation : il n'existe que scellé, sur ton téléphone et ton serveur (et au guichet avec l'offre tout compris, tant que l'appareil est relié à ta place) |
| Adresse IP au relais | En mémoire, une heure au plus, sous forme d'empreinte |
| Journaux techniques du relais | 6 mois au plus |
| E-mails de support | 2 ans après le dernier échange |
| Signalements | 1 an après leur traitement |
| Rapports de plantage | 1 an après réception (côté Kerfy Studio) |
| Compte de l'offre (identifiants, e-mail, appareils, invitations, crédit gardé) | Tant que le compte existe ; effacé 30 jours après une demande de suppression |
| Invitation non acceptée | 30 jours |
| Adresse e-mail et identifiant de compte après la fin du contrat (obligation des hébergeurs) | 5 ans pour l'e-mail, 1 an pour l'identifiant, gardés à part |
| Abonnement et achats | Pendant l'abonnement, puis 5 ans en archive |
| Empreinte de l'appareil Android, hachée avec une clé secrète (semaine gratuite) | 24 mois |
| Adresse IP à la réservation d'une semaine gratuite | 15 minutes au plus |
| Contenu de ta place | Pendant l'abonnement, puis 30 jours (3 jours après une semaine gratuite non poursuivie) |
| Sauvegardes | 48 heures (horaires), 30 jours (quotidiennes), 12 semaines (hebdomadaires) ; effacées avec la place |
| Image quotidienne d'OVHcloud | Un point par jour, remplacé dans la journée environ après la réinstallation de la place |
| Données techniques des serveurs | Dernières valeurs seulement |
| Journal des événements du guichet | 12 mois |
| Métadonnées chez EUrouter | Jusqu'à 12 mois, selon EUrouter |
| Signalements d'abus | 1 an après leur traitement |

## 13. Sécurité

Le relais n'accepte que des connexions chiffrées (HTTPS), scelle les jetons (XChaCha20-Poly1305), n'a pas de base de
données et n'écrit jamais un jeton en clair. Dans l'app, les clés de tes agents sont protégées par le Trousseau ou le
Keystore, et la connexion à ton serveur passe par Tailscale ou par une porte HTTPS sur ton réseau local, qui
n'accepte que le certificat de ton serveur. Le code de l'app, du serveur et du relais est public : [URL du dépôt].
Pour signaler une faille de sécurité : **support@kerfystudio.com** (voir le fichier SECURITY.md du dépôt).

Avec l'offre tout compris, chaque place est un compte séparé du serveur, sans droits d'administrateur ; Kerfy Studio
n'a pas de clé SSH sur les serveurs, et un serveur n'exécute que des ordres signés par le guichet. L'app se connecte à
ta place en HTTPS et vérifie son certificat. Les sauvegardes sont chiffrées sur la place avant de partir.

**En cas de violation de données**, Kerfy Studio prévient la CNIL dans les 72 heures (article 33 du RGPD) et les
personnes concernées quand le risque pour elles est élevé (article 34) ; pour les personnes à Singapour, la Personal
Data Protection Commission dans les 3 jours quand la loi de Singapour le demande ; pour les personnes au Canada, le
Commissariat à la protection de la vie privée du Canada et les personnes concernées dès que possible quand il y a un
risque réel de préjudice grave, et, pour les personnes au Québec, la Commission d'accès à l'information et les
personnes concernées avec diligence quand l'incident présente un risque de préjudice sérieux. Kerfy Studio tient un
registre de chaque violation.

## 14. Tes droits

Tu peux demander l'accès à tes données, leur rectification, leur effacement, la limitation de leur traitement et leur
portabilité, et donner des directives sur le sort de tes données après ton décès.

**Droit d'opposition.** Tu peux t'opposer à tout moment, pour des raisons tenant à ta situation particulière, aux
traitements fondés sur notre intérêt légitime (limitation des abus, e-mails, signalements, rapports de plantage,
limite des semaines gratuites).

Écris à **support@kerfystudio.com**. Nous répondons dans un délai d'un mois, prolongeable de deux mois pour une
demande complexe ; nous pouvons te demander de quoi vérifier ton identité.

Avec ton propre serveur, comme nous ne conservons rien qui relie un jeton de notification ou un signalement à une
personne, nous ne pouvons généralement pas retrouver ces données te concernant : pour arrêter les notifications,
coupe-les ou désinstalle l'app. Pour les données de ton serveur, c'est toi qui les gères ; pour celles de ton
fournisseur de modèle ou de ton hébergeur, adresse-toi à eux.

**Avec l'offre tout compris**, ton compte relie ces données à toi : nous pouvons y donner accès, les corriger ou les
effacer. Tu accèdes toi-même au contenu de ta place, par l'app ou par SSH, et tu peux le copier à tout moment. Pour
supprimer ton compte : section 5.2.

**Si tu vis en Californie ou ailleurs aux États-Unis.** Kerfy Studio ne vend ni ne partage tes données personnelles au
sens de la loi californienne (California Consumer Privacy Act) et ne fait aucune publicité ciblée. L'app et nos sites
ne te suivent pas d'un site à l'autre : un signal « Do Not Track » de ton navigateur ne change donc rien à ce que nous
faisons (California Online Privacy Protection Act). Tu exerces les droits de cette section à support@kerfystudio.com.

**Si tu vis au Canada.** Les lois canadiennes reposent sur le consentement : en utilisant l'app et en souscrivant
après avoir vu cette politique, tu consens aux traitements qu'elle décrit, pour les fins qu'elle indique. Tu as les
droits d'accès et de rectification de la loi canadienne et, au Québec, ceux de la
loi du Québec : recevoir tes données informatisées dans un format structuré et couramment utilisé, retirer ton
consentement (ce qui met fin au service quand il en a besoin pour fonctionner), et être informé quand une décision
est prise uniquement par un traitement automatisé (Kerfy Studio n'en prend aucune à ton sujet). Pour toi, nous
répondons dans les 30 jours. Kerfy Studio ne vend pas tes données et ne fait aucune publicité ciblée.

## 15. Réclamation

Si tu estimes que tes droits ne sont pas respectés, tu peux saisir la **CNIL** (3 place de Fontenoy, TSA 80715,
75334 Paris Cedex 07 — https://www.cnil.fr/fr/plaintes) ou l'autorité de protection des données de ton pays : au
Royaume-Uni, l'Information Commissioner's Office (https://ico.org.uk) ; en Israël, la Privacy Protection Authority
(https://www.gov.il/en/departments/the_privacy_protection_authority) ; à Singapour, la Personal Data Protection
Commission (https://www.pdpc.gov.sg) ; au Canada, le Commissariat à la protection de la vie privée du Canada
(https://www.priv.gc.ca, 1-800-282-1376) ; au Québec, la Commission d'accès à l'information
(https://www.cai.gouv.qc.ca, 1 888 528-7741).

## 16. Enfants

BipAgents s'adresse aux personnes de **16 ans et plus**. Entre 16 et 18 ans, utilise-la avec l'accord d'un parent et
respecte l'âge minimum de ton fournisseur de modèle (souvent 18 ans). **Avant 16 ans, l'app ne doit être utilisée que
sur une installation préparée et supervisée par un parent ou un adulte qui en a la charge** (fonction famille) : cet
adulte choisit les agents, leurs accès et le modèle, accepte les conditions d'utilisation pour l'enfant et reste
responsable de son usage.

Kerfy Studio ne demande pas l'âge et aucun de ses traitements ne repose sur le consentement. Seul le scanner de
documents d'Android demande un accord, pour ce que Google en reçoit (section 7) : pour un enfant de moins de 15 ans,
l'adulte qui a préparé l'installation le donne avec lui. Si un enfant nous a écrit, un parent peut nous demander
d'effacer son message.

**Offre tout compris** : elle est souscrite par un adulte, ou avec l'accord d'un parent. Une personne invitée doit
avoir au moins 16 ans ; avant 16 ans, une place ne peut être utilisée que préparée et supervisée par un parent ou un
adulte qui en a la charge ([conditions de l'offre](offer.fr.md), section 7). **Aux États-Unis, aucune personne de moins
de 13 ans ne peut utiliser une place**, et Kerfy Studio ne collecte sciemment aucune donnée d'un enfant de moins de
13 ans (Children's Online Privacy Protection Act) ; si c'est arrivé, écris-nous et nous l'effaçons. **Au Québec**, les
données d'un enfant de moins de 14 ans ne sont recueillies qu'avec le consentement d'un parent ou du tuteur : l'adulte
qui prépare et supervise sa place le donne pour lui (articles 4.1 et 14 de la loi du Québec).

## 17. Modifications

Nous pouvons modifier cette politique, notamment avant de proposer des liens partenaires. La date en haut de page
change à chaque fois. Un changement important est annoncé dans l'app au moins 30 jours avant de s'appliquer.

## 18. Contact

Kerfy Studio — [adresse du siège] — **support@kerfystudio.com** — abus : **abuse@kerfystudio.com**

Cette politique, les conditions d'utilisation, les [conditions de l'offre tout compris](offer.fr.md), la
[politique d'usage acceptable](acceptable-use.fr.md) et les mentions légales sont aussi dans l'app : Réglages ›
À propos.
