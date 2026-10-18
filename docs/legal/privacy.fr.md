[English](privacy.en.md) · Français

# Politique de confidentialité de BipAgents

> **Projet** — à faire relire par un avocat avant publication. Les éléments entre crochets sont à compléter.
> Dernière mise à jour : [date].

## En bref

- BipAgents relie ton téléphone à **tes propres agents IA**, qui tournent sur ton ordinateur ou ton serveur avec
  Hermes Agent. Tes conversations, tes fichiers, ta voix et la mémoire de tes agents restent sur ton serveur et chez le
  fournisseur de modèle d'IA que **tu** as choisi. **Kerfy Studio, l'éditeur de l'app, ne les voit pas.**
- Pas de compte, pas de publicité, pas de statistiques d'usage, pas de pistage.
- Kerfy Studio ne peut recevoir que trois choses :
  1. de quoi acheminer tes **notifications** par son relais : un jeton d'appareil scellé, jamais le texte de tes
     messages ;
  2. les **e-mails** que tu choisis d'envoyer (support, signalement d'une réponse) ;
  3. des **rapports de plantage**, seulement si tu as accepté de les partager dans les réglages de ton téléphone.

## 1. Qui est responsable

**Kerfy Studio**, SAS au capital de [capital] €, dont le siège est situé [adresse du siège], immatriculée au RCS de
[RCS] sous le numéro [SIREN] (« Kerfy Studio », « nous »). Contact : **support@kerfystudio.com**.
[Délégué à la protection des données : aucun désigné à ce jour — à confirmer.]

Kerfy Studio est responsable du traitement **uniquement** pour les données décrites à la section 4.

## 2. Comment marche BipAgents, et qui voit quoi

| Où | Ce qui s'y trouve | Qui en décide |
|---|---|---|
| **Ton serveur** (ton ordinateur, ou un serveur que tu loues) | Hermes Agent, tes agents, tes conversations, tes fichiers, leur mémoire, leurs tâches planifiées, le coffre à mots de passe | Toi |
| **Ton fournisseur de modèle** (une API dans le cloud, par exemple OpenRouter, ou un modèle qui tourne chez toi) | Chaque message que tes agents lui envoient pour y répondre | Toi, selon les conditions de ce fournisseur |
| **Ton téléphone** | Ce que l'app garde pour fonctionner (section 3) | Toi |
| **Kerfy Studio** | Uniquement ce qui est décrit à la section 4 | Kerfy Studio |

- **BipAgents est construit sur Hermes Agent**, un logiciel libre de Nous Research
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

Ces données restent sur ton téléphone et ne sont **pas** envoyées à Kerfy Studio :

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

**Sauvegardes.** Sur iPhone, ces données font partie de ta sauvegarde iCloud ou de celle de ton ordinateur si tu
l'as activée, sauf les clés, qui restent sur l'appareil. Sur Android, l'app exclut ses données des sauvegardes ; un
transfert direct vers un nouveau téléphone peut en copier une partie, mais pas les clés, qui y restent illisibles.

**Effacer.** Supprime un agent dans l'app, ou désinstalle l'app pour tout effacer du téléphone. Cela n'efface pas ce
qui se trouve sur ton serveur.

**Partages que tu déclenches.** Le rapport de diagnostic (Réglages › Diagnostics et latences › Exporter le rapport) contient le nom
et l'adresse de tes agents ; il ne part que vers l'app ou la personne que tu choisis dans la feuille de partage.

## 4. Ce que Kerfy Studio peut recevoir

### 4.1 Les notifications, par le relais de Kerfy Studio

> [À la date de rédaction, le relais n'est pas encore en service : à mettre à jour au lancement.]

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
jeton et la réponse d'Apple ou de Google, conservés [durée de conservation des journaux du relais]. [L'hébergeur du
relais peut journaliser les adresses IP des requêtes : nature et durée à préciser selon le prestataire retenu.]

**Finalités et bases légales.** Acheminer les notifications que tu as activées : exécution des conditions
d'utilisation (article 6.1.b du RGPD). Limiter les abus et protéger le service : intérêt légitime de Kerfy Studio
(article 6.1.f).

**Destinataires.** Apple (Apple Push Notification service), Google (Firebase Cloud Messaging), et l'hébergeur du
relais, situé dans l'Union européenne : [hébergeur du relais].

**Sans notifications.** Tu peux refuser ou couper les notifications dans les réglages de ton téléphone : l'app
fonctionne sans elles. Si tu désinstalles l'app, Apple ou Google signalent le jeton comme invalide et ton serveur
l'oublie. Si tu compiles l'app toi-même avec ton propre compte Apple, le relais de Kerfy Studio n'est pas utilisé.

**Sur Android**, la bibliothèque Firebase Cloud Messaging de Google démarre avec l'app et obtient un jeton auprès de
Google dès l'ouverture, y compris avant que tu ajoutes un agent. [À vérifier et, si possible, à retarder jusqu'à
l'ajout d'un agent.]

### 4.2 Les e-mails que tu nous envoies : support et signalements

- **Réglages › Contacter le support** prépare un e-mail à support@kerfystudio.com avec un diagnostic que tu peux lire
  avant l'envoi : version de l'app, modèle et version du téléphone, langue, style d'affichage, mode démo ou non, état
  des notifications et, pour chaque agent, sa catégorie, s'il répond, s'il a un bridge et une porte sur le réseau
  local, et sa voix. **Aucune conversation, aucun nom d'agent, aucune adresse, aucune clé.**
- **Signaler cette réponse** (appui long sur une réponse) prépare un e-mail avec le motif, tes précisions
  éventuelles, **le texte de la réponse signalée**, la catégorie de l'agent, la version de l'app et le modèle du
  téléphone. Il part à Kerfy Studio seulement : ni à ton serveur, ni au fournisseur du modèle.
- Rien ne part tant que tu n'as pas appuyé sur « Envoyer » dans ton app de mail. Nous recevons aussi ton adresse
  e-mail et tout ce que tu choisis d'écrire : évite d'y mettre des informations sensibles.

**Finalités.** Te répondre, traiter les signalements, corriger les bugs et améliorer l'app.
**Base légale.** Intérêt légitime de Kerfy Studio à répondre aux demandes et à rendre l'app plus sûre
(article 6.1.f du RGPD).
**Durée.** [Proposition à valider : le temps de traiter la demande, puis 3 ans après le dernier échange ; les
signalements, 1 an après leur traitement.]
**Destinataires.** Kerfy Studio et son prestataire de messagerie : [prestataire de messagerie et pays
d'hébergement].

### 4.3 Les rapports de plantage

Si tu as accepté, dans les réglages de ton téléphone, de partager les données d'analyse avec les développeurs
d'apps (iPhone : Réglages › Confidentialité et sécurité › Analyse et améliorations › Partager avec les développeurs d'apps ; Android : partage des données
d'utilisation et de diagnostic), Apple ou Google peuvent transmettre à Kerfy Studio des rapports de plantage et des
mesures techniques (modèle, version du système, endroit du code où l'app a planté). L'app ne contient aucun outil
de statistiques ou de rapport de plantage de tiers : ces rapports viennent uniquement d'Apple ou de Google.

**Finalité.** Corriger les bugs. **Base légale.** Ton consentement, donné à Apple ou Google et retirable dans les
mêmes réglages (article 6.1.a du RGPD). **Durée.** [Durée de conservation fixée par Apple et Google, à préciser.]

### 4.4 Les statistiques des stores

Apple et Google fournissent à Kerfy Studio des statistiques agrégées sur l'app (téléchargements, pays, versions),
selon leurs propres règles. Elles ne nous permettent pas de t'identifier.

## 5. Ce que Kerfy Studio ne fait pas

- Pas de compte, pas de publicité, pas d'outil de statistiques, pas de pistage entre apps ou sites.
- Pas de vente ni de location de données.
- Pas d'entraînement de modèles d'IA sur tes données : nous ne les avons pas.
- Pas d'accès à ton serveur, à tes conversations ni à tes fichiers.

## 6. Les services que l'app contacte directement

En plus de ton serveur, l'app peut contacter directement ces services, qui appliquent leurs propres règles. Kerfy
Studio ne reçoit rien de ces échanges.

- **Icônes des sites (DuckDuckGo).** Pour afficher la petite icône des sites cités dans les réponses, l'app la demande
  à DuckDuckGo (icons.duckduckgo.com). DuckDuckGo reçoit le **nom du site** (par exemple « exemple.fr ») et ton
  adresse IP, pas le message.
- **Reconnaissance vocale.** Les notes vocales et le Live utilisent la reconnaissance vocale de ton téléphone. Sur
  iPhone, elle se fait sur l'appareil quand il le permet pour la langue choisie ; sinon, Apple peut traiter l'audio sur
  ses serveurs. Sur Android, le service de reconnaissance du système (souvent celui de Google) peut traiter l'audio
  sur ses serveurs, en particulier pendant le Live. C'est le texte transcrit qui part à ton serveur.
- **Synthèse vocale du téléphone.** Quand la voix du téléphone lit un texte (dans la démo, ou si ton serveur ne
  répond pas), elle utilise le moteur de synthèse de ton téléphone, selon ses réglages.
- **Sur Android, Google ML Kit** sert à lire le QR code (sur l'appareil) et à scanner des documents (par les services
  Google Play). Google indique que ML Kit peut lui envoyer des mesures d'usage et de performance. [À vérifier.]
- **Apple et Google** pour les notifications (section 4.1).
- **Tailscale**, si tu l'utilises : une app distincte, avec son propre compte et sa propre politique de
  confidentialité.
- **Les liens que tu ouvres** (sites cités par tes agents, page de Hermes Agent) s'ouvrent dans ton navigateur.

## 7. La démo

La démo intégrée tourne entièrement sur ton téléphone : des agents et des données inventés, sans compte, sans
serveur, et rien n'est envoyé à Kerfy Studio. Son podcast est un fichier audio enregistré à l'avance et inclus dans
l'app (voix : Fish Audio Drama 3, « Built with Fish Audio »). Si tu parles à la démo, la reconnaissance vocale du
téléphone s'applique (section 6).

## 8. Les voix

Les voix des Bips sont synthétisées **sur ton serveur** : le texte à lire part à ton serveur, pas chez nous. Si tu
branches ton propre modèle de voix, c'est un serveur que tu fais tourner toi-même. Kerfy Studio ne reçoit rien.

## 9. Liens partenaires (hébergement)

L'app ou le site kerfystudio.com pourront proposer des hébergeurs par des liens partenaires (affiliation). Si tu
suis un tel lien, l'hébergeur peut savoir que ta visite vient de BipAgents, et Kerfy Studio peut recevoir une
commission si tu souscris. L'app n'envoie aucune donnée personnelle à l'hébergeur ; ce que tu lui donnes ensuite
relève de sa propre politique de confidentialité. [À compléter selon les contrats : données éventuellement reçues
du partenaire, par exemple un nombre d'inscriptions.]

## 10. Transferts hors de l'Union européenne

Le relais de notifications est hébergé dans l'Union européenne ([hébergeur du relais]). Apple et Google peuvent
traiter les jetons, les notifications et les rapports de plantage hors de l'Union européenne, notamment aux
États-Unis ; ils s'appuient pour cela sur [la décision d'adéquation « EU-US Data Privacy Framework » et/ou des
clauses contractuelles types — à vérifier]. Prestataire de messagerie : [pays d'hébergement].

## 11. Durées de conservation

| Données | Durée |
|---|---|
| Jeton d'appareil au relais | Aucune conservation : il n'existe que scellé, sur ton téléphone et ton serveur |
| Adresse IP au relais | En mémoire, une heure au plus, sous forme d'empreinte |
| Journaux techniques du relais | [durée de conservation des journaux du relais] |
| E-mails de support | [proposition : 3 ans après le dernier échange] |
| Signalements | [proposition : 1 an après leur traitement] |
| Rapports de plantage | [selon Apple et Google] |

## 12. Sécurité

Le relais n'accepte que des connexions chiffrées (HTTPS), scelle les jetons (XChaCha20-Poly1305), n'a pas de base de
données et n'écrit jamais un jeton en clair. Dans l'app, les clés de tes agents sont protégées par le Trousseau ou le
Keystore, et la connexion à ton serveur passe par Tailscale ou par une porte HTTPS sur ton réseau local, qui
n'accepte que le certificat de ton serveur. Le code de l'app, du serveur et du relais est public : [URL du dépôt].

## 13. Tes droits

Tu peux demander l'accès à tes données, leur rectification, leur effacement, la limitation de leur traitement, leur
portabilité, t'opposer à un traitement fondé sur notre intérêt légitime, retirer ton consentement à tout moment, et
donner des directives sur le sort de tes données après ton décès.

Écris à **support@kerfystudio.com**. Nous répondons dans un délai d'un mois, prolongeable de deux mois pour une
demande complexe ; nous pouvons te demander de quoi vérifier ton identité.

Comme nous ne conservons rien qui relie un jeton de notification à une personne, nous ne pouvons généralement pas
retrouver de données de relais te concernant : pour arrêter, coupe les notifications ou désinstalle l'app. Pour les
données de ton serveur, c'est toi qui les gères ; pour celles de ton fournisseur de modèle ou de ton hébergeur,
adresse-toi à eux.

## 14. Réclamation

Si tu estimes que tes droits ne sont pas respectés, tu peux saisir la **CNIL** (3 place de Fontenoy, TSA 80715,
75334 Paris Cedex 07 — https://www.cnil.fr/fr/plaintes) ou l'autorité de protection des données de ton pays.

## 15. Enfants

> [À valider par l'avocat : âge de 15 ans (article 45 de la loi Informatique et Libertés), classification d'âge des
> stores, cohérence avec la fonction famille.]

BipAgents ne s'adresse pas aux enfants. **Avant 15 ans, l'app ne doit être utilisée qu'avec la supervision d'un
parent.** La fonction famille permet à un adulte de préparer une installation pour un proche : cet adulte est
responsable de superviser l'usage d'un mineur, de choisir des agents et un modèle adaptés, et de vérifier l'âge
minimum exigé par le fournisseur de modèle. L'app n'a pas de compte et ne connaît donc pas l'âge de ses utilisateurs.
Si un enfant de moins de 15 ans nous a écrit sans l'accord d'un parent, contacte-nous : nous effacerons son message.

## 16. Modifications

Nous pouvons modifier cette politique, notamment au lancement du relais ou de liens partenaires. La date en haut de
page change à chaque fois ; un changement important est annoncé dans l'app ou sur kerfystudio.com
[mode d'information à préciser].

## 17. Contact

Kerfy Studio — [adresse du siège] — **support@kerfystudio.com**
