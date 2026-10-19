[English](privacy.en.md) · Français · [Deutsch](privacy.de.md) · [Español](privacy.es.md)

# Politique de confidentialité de BipAgents

> Dernière mise à jour : [date].

## En bref

- BipAgents relie ton téléphone à **tes propres agents IA**, qui tournent sur ton ordinateur ou ton serveur avec
  Hermes Agent. Tes conversations, tes fichiers, ta voix et la mémoire de tes agents restent sur ton serveur et chez le
  fournisseur de modèle d'IA que **tu** as choisi. **Kerfy Studio, l'éditeur de l'app, ne les voit pas.**
- Pas de compte, pas de publicité, pas de statistiques d'usage, pas de pistage.
- Kerfy Studio ne peut recevoir que trois choses :
  1. de quoi acheminer tes **notifications** par son relais : un jeton d'appareil scellé, jamais le texte de tes
     messages ;
  2. ce que tu choisis de nous envoyer : un **e-mail** au support, ou le **signalement** d'une réponse depuis l'app ;
  3. des **rapports de plantage**, seulement si tu as activé leur partage dans les réglages de ton téléphone.

## 1. Qui est responsable

**Kerfy Studio**, SAS au capital de [capital] €, dont le siège est situé [adresse du siège], immatriculée au RCS de
[ville du greffe] sous le numéro [SIREN] (« Kerfy Studio », « nous »). Contact : **support@kerfystudio.com**.

Kerfy Studio n'a pas désigné de délégué à la protection des données : ce n'est pas obligatoire pour son activité
(article 37 du RGPD). Pour toute question sur tes données : support@kerfystudio.com.

Kerfy Studio est responsable des traitements que l'app réalise pour fonctionner, tels qu'il les a conçus
(sections 3, 4 et 6), même lorsqu'ils restent sur ton téléphone et qu'il n'y a pas accès. Il n'est ni responsable ni
sous-traitant des données que traitent ton serveur, tes agents et ton fournisseur de modèle (section 2).

## 2. Comment marche BipAgents, et qui voit quoi

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
ton serveur, pas chez nous. Les journaux techniques du relais contiennent la plateforme, une empreinte de 4 octets du
jeton et la réponse d'Apple ou de Google ; ils sont conservés **6 mois au plus**. L'hébergeur du relais peut
journaliser les adresses IP des requêtes : [ce que l'hébergeur du relais journalise, et pendant combien de temps].

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

## 5. Ce que Kerfy Studio ne fait pas

- Pas de compte, pas de publicité, pas d'outil de statistiques, pas de pistage entre apps ou sites.
- Pas de vente ni de location de données.
- Pas d'entraînement de modèles d'IA sur tes données : nous ne les avons pas.
- Pas d'accès à ton serveur, à tes conversations ni à tes fichiers.

## 6. Les autres services que l'app fait intervenir

En plus de ton serveur, l'app fait appel à ces services, qui appliquent leurs propres règles. Kerfy Studio ne reçoit
rien de ces échanges. Il a choisi de les confier à l'app et répond de ce qui part de ton téléphone ; chaque service
est ensuite responsable de ce qu'il en fait.

- **Icônes des sites.** Pour afficher la petite icône des sites cités dans les réponses, l'app demande ces icônes à
  ton serveur, qui les récupère auprès de DuckDuckGo (icons.duckduckgo.com) : DuckDuckGo reçoit le **nom du site**
  (par exemple « exemple.fr ») et voit l'adresse de ton serveur, pas celle de ton téléphone. Il ne reçoit pas le
  message.
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

## 7. La démo

La démo intégrée tourne entièrement sur ton téléphone : des agents et des données inventés, sans compte, sans
serveur, et rien n'est envoyé à Kerfy Studio. Tu peux l'ouvrir dès la première ouverture, avec « Seulement la démo ».
Son podcast est un fichier audio enregistré à l'avance et inclus dans l'app (voix : Fish Audio Drama 3, « Built with
Fish Audio »). Si tu parles à la démo, la reconnaissance vocale du téléphone s'applique (section 6).

## 8. Les voix

Les voix des Bips sont synthétisées **sur ton serveur** : le texte à lire part à ton serveur, pas chez nous. Si tu
branches ton propre modèle de voix, c'est un serveur que tu fais tourner toi-même. Kerfy Studio ne reçoit rien.

## 9. Liens partenaires (hébergement, plus tard)

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
sera annoncé comme indiqué à la section 16.

## 10. Transferts hors de l'Union européenne

Le relais de notifications est hébergé dans l'Union européenne ([hébergeur du relais]). Pour acheminer les
notifications, Apple et Google peuvent traiter le jeton et la notification hors de l'Union européenne, notamment aux
États-Unis :

- Google LLC (Firebase Cloud Messaging) est certifiée au « EU-U.S. Data Privacy Framework », reconnu comme offrant une
  protection adéquate par la décision d'exécution (UE) 2023/1795 de la Commission du 10 juillet 2023 ;
- Apple indique encadrer les transferts depuis l'Espace économique européen par les clauses contractuelles types
  adoptées par la Commission européenne.

Prestataire de messagerie : [pays ; garantie]. Tu peux nous demander une copie des garanties applicables.

## 11. Durées de conservation

| Données | Durée |
|---|---|
| Données de l'app sur ton téléphone | Jusqu'à ce que tu supprimes l'agent ou l'app |
| Jeton d'appareil au relais | Aucune conservation : il n'existe que scellé, sur ton téléphone et ton serveur |
| Adresse IP au relais | En mémoire, une heure au plus, sous forme d'empreinte |
| Journaux techniques du relais | 6 mois au plus |
| E-mails de support | 2 ans après le dernier échange |
| Signalements | 1 an après leur traitement |
| Rapports de plantage | 1 an après réception (côté Kerfy Studio) |

## 12. Sécurité

Le relais n'accepte que des connexions chiffrées (HTTPS), scelle les jetons (XChaCha20-Poly1305), n'a pas de base de
données et n'écrit jamais un jeton en clair. Dans l'app, les clés de tes agents sont protégées par le Trousseau ou le
Keystore, et la connexion à ton serveur passe par Tailscale ou par une porte HTTPS sur ton réseau local, qui
n'accepte que le certificat de ton serveur. Le code de l'app, du serveur et du relais est public : [URL du dépôt].
Pour signaler une faille de sécurité : **support@kerfystudio.com** (voir le fichier SECURITY.md du dépôt).

## 13. Tes droits

Tu peux demander l'accès à tes données, leur rectification, leur effacement, la limitation de leur traitement et leur
portabilité, et donner des directives sur le sort de tes données après ton décès.

**Droit d'opposition.** Tu peux t'opposer à tout moment, pour des raisons tenant à ta situation particulière, aux
traitements fondés sur notre intérêt légitime (limitation des abus, e-mails, signalements, rapports de plantage).

Écris à **support@kerfystudio.com**. Nous répondons dans un délai d'un mois, prolongeable de deux mois pour une
demande complexe ; nous pouvons te demander de quoi vérifier ton identité.

Comme nous ne conservons rien qui relie un jeton de notification ou un signalement à une personne, nous ne pouvons
généralement pas retrouver ces données te concernant : pour arrêter les notifications, coupe-les ou désinstalle
l'app. Pour les données de ton serveur, c'est toi qui les gères ; pour celles de ton fournisseur de modèle ou de ton
hébergeur, adresse-toi à eux.

## 14. Réclamation

Si tu estimes que tes droits ne sont pas respectés, tu peux saisir la **CNIL** (3 place de Fontenoy, TSA 80715,
75334 Paris Cedex 07 — https://www.cnil.fr/fr/plaintes) ou l'autorité de protection des données de ton pays.

## 15. Enfants

BipAgents s'adresse aux personnes de **16 ans et plus**. Entre 16 et 18 ans, utilise-la avec l'accord d'un parent et
respecte l'âge minimum de ton fournisseur de modèle (souvent 18 ans). **Avant 16 ans, l'app ne doit être utilisée que
sur une installation préparée et supervisée par un parent ou un adulte qui en a la charge** (fonction famille) : cet
adulte choisit les agents, leurs accès et le modèle, accepte les conditions d'utilisation pour l'enfant et reste
responsable de son usage.

Kerfy Studio ne demande pas l'âge et aucun de ses traitements ne repose sur le consentement. Seul le scanner de
documents d'Android demande un accord, pour ce que Google en reçoit (section 6) : pour un enfant de moins de 15 ans,
l'adulte qui a préparé l'installation le donne avec lui. Si un enfant nous a écrit, un parent peut nous demander
d'effacer son message.

## 16. Modifications

Nous pouvons modifier cette politique, notamment avant de proposer des liens partenaires. La date en haut de page
change à chaque fois. Un changement important est annoncé dans l'app au moins 30 jours avant de s'appliquer.

## 17. Contact

Kerfy Studio — [adresse du siège] — **support@kerfystudio.com**

Cette politique, les conditions d'utilisation et les mentions légales sont aussi dans l'app : Réglages › À propos.
