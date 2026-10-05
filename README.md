<p align="center">
  <img src="docs/bips.svg" alt="Les huit Bips de BipAgents, qui sautillent et clignent des yeux" width="100%">
</p>

<h1 align="center">BipAgents</h1>

<p align="center">
  <b>Parle à tes agents IA auto-hébergés comme à des potes de poche.</b><br>
  App iOS voice-first pour <a href="https://github.com/NousResearch/hermes-agent">Hermes Agent</a>, sur ton propre serveur, via Tailscale.
</p>

<p align="center">
  <a href="https://alexbob0.github.io/BipAgents/play/"><img alt="Joue avec les Bips" src="https://img.shields.io/badge/%F0%9F%8E%AE_Joue_avec_les_Bips-FF7B5C?style=for-the-badge"></a>
  <a href="https://x.com/Bob_AI_Digger"><img alt="Suis @Bob_AI_Digger sur X" src="https://img.shields.io/badge/@Bob__AI__Digger-16181D?style=for-the-badge&logo=x&logoColor=white"></a>
  <img alt="iOS 18+" src="https://img.shields.io/badge/iOS-18%2B-4F7CFF?style=for-the-badge&logo=apple&logoColor=white">
  <img alt="Swift 6" src="https://img.shields.io/badge/Swift-6-F2649E?style=for-the-badge&logo=swift&logoColor=white">
  <a href="LICENSE"><img alt="Licence MIT" src="https://img.shields.io/badge/licence-MIT-3CC37A?style=for-the-badge"></a>
</p>

---

Chaque agent est un **Bip** : une petite mascotte avec sa couleur, sa forme, son humeur et **sa voix**. Wellness veille sur ton sommeil, Vie range tes fichiers, Budget surveille tes dépenses… Tu leur écris, tu leur parles, ou tu lances un **Live** pour discuter à voix haute, mains libres. Ils bossent sur **ton** serveur, et l'app te prévient quand ils ont fini, ou quand ils ont besoin de toi.

<p align="center">
  <img src="docs/screenshots/home.png" width="24%" alt="Accueil : une carte par agent">
  <img src="docs/screenshots/conversation.png" width="24%" alt="Conversation : outils, liens avec favicons, réponse à écouter">
  <img src="docs/screenshots/approval-question.png" width="24%" alt="Demande d'accord et question de l'agent">
  <img src="docs/screenshots/live.png" width="24%" alt="Live : conversation vocale mains libres">
</p>

## 🎮 Joue avec les Bips

Les Bips ne sont pas que décoratifs. Dans l'app (et [**dans ton navigateur**](https://alexbob0.github.io/BipAgents/play/)), ils réagissent :

| Geste | Réaction |
|---|---|
| 👆 Toucher | un petit saut, un clin d'œil, une pirouette… au hasard |
| 👆👆👆 Trois touchers rapides, ou frotter vite | ça chatouille ! |
| 🫳 Caresser lentement | ronron, cœurs, joues roses |
| ✊ Appui long puis relâcher | **Boing !** |
| 😵 Trop les embêter | le tournis… puis ils boudent |

Et ils **babillent** : chaque réaction est dite dans leur « Animalese », un charabia mignon synthétisé à la volée, sans modèle ni réseau, avec une hauteur de voix propre à chaque Bip.

<p align="center">
  <a href="https://alexbob0.github.io/BipAgents/play/"><b>→ Ouvrir le terrain de jeu des Bips</b></a>
</p>

## ✨ Ce que sait faire l'app

- 🎙️ **Voix d'abord** : notes vocales (maintenir « Audio » sur la carte d'un agent, relâcher, c'est parti), **Live** mains libres avec interruption à la voix, « Écouter » sous chaque réponse en streaming.
- 🗣️ **Une voix par Bip** : voix mignonnes Kyutai Pocket TTS sur CPU, ou voix « sérieuse » Kyutai 1.6B sur GPU pour le podcast du matin. Les nombres, symboles et mots anglais sont rendus prononçables en français.
- 🔒 **Ça continue écran verrouillé** : les messages partent en *runs* Hermes. Tu fermes l'app, l'agent finit son travail, et la réponse arrive en notification, avec l'avatar du Bip et le vrai texte.
- ✅ **Accords depuis l'écran verrouillé** : « Wellness veut lancer `pip install openpyxl` » → Une fois / Cette session / Toujours / Refuser, sans ouvrir l'app.
- ❓ **L'agent peut te poser une question** (outil `clarify`) : une carte avec un bouton par choix, comme sur Hermes Desktop.
- 🤝 **Les agents se parlent** : Vie peut consulter Wellness ; l'échange se replie en une ligne dans la conversation.
- 📬 **La Boîte** : rapports des tâches planifiées (cron), réponses arrivées pendant ton absence, à écouter ou à relancer en Live.
- 📎 **Tout type de fichier** : photos, PDF, Excel, scans… et les fichiers produits par l'agent (audio, documents) s'ouvrent dans l'app.
- 🌍 **En français, anglais, espagnol et allemand** : l'app suit la langue de l'iPhone, et chaque agent a sa propre langue (reconnaissance vocale et voix).
- 🔗 Liens cliquables avec favicons, outils repliés, brouillons gardés par conversation, séparateurs de jours.

<details>
<summary><b>📸 Plus de captures</b></summary>
<br>
<p align="center">
  <img src="docs/screenshots/home-5-agents.png" width="24%" alt="Accueil avec cinq agents">
  <img src="docs/screenshots/inbox.png" width="24%" alt="La Boîte">
  <img src="docs/screenshots/settings.png" width="24%" alt="Réglages">
</p>
</details>

## 🧩 Comment ça marche

```mermaid
flowchart LR
  subgraph iPhone
    A[BipAgents<br/>SwiftUI]
    N[Extension de<br/>notification]
  end
  subgraph Serveur["Ton serveur (Tailscale)"]
    H[Hermes Agent<br/>api_server · runs · Bot Mode]
    B[bipbridge<br/>FastAPI]
    K[Kyutai TTS<br/>GPU + Pocket CPU]
  end
  A -- "texte, fichiers, runs (SSE)" --> H
  A -- "voix, fichiers, suivi des runs" --> B
  B --> H
  B --> K
  B -- "push APNs" --> N
```

- **L'app** parle directement à l'api_server Hermes (sessions, runs, accords, questions) et au **bridge** pour tout le reste.
- **Le bridge** (`bridge/`, Python) synthétise les voix, sert les fichiers, relaie les runs pour qu'aucun événement ne se perde, surveille les tâches planifiées et envoie les notifications push.
- Tout reste chez toi : aucun cloud tiers à part Apple pour les push. Aucun secret dans ce dépôt ; les clés sont saisies dans l'app (Trousseau) ou dans la config du bridge sur le serveur.

## 🚀 Démarrer

**Juste pour voir**, sans serveur : ouvre `BipAgents.xcodeproj`, schéma BipAgents → Arguments → `-demo`. Ajoute `-screen conversation`, `-screen call`, `-tab inbox` ou `-demoAgents 5` pour arriver directement sur un écran.

**Pour de vrai** :

1. Un serveur avec [Hermes Agent](https://github.com/NousResearch/hermes-agent), son api_server activé et [Tailscale](https://tailscale.com).
2. Le bridge : voir [`bridge/README.md`](bridge/README.md) (TTS Kyutai, push APNs, fichiers, tâches planifiées).
3. Dans Xcode, choisis ton équipe de signature (cibles BipAgents et NotificationService). Push, App Group `group.io.github.bipagents` et Keychain Sharing sont déclarés dans `Config/*.entitlements`.
4. Dans l'app : Réglages › Ajouter un agent (adresse Tailscale, clé api_server, profil Hermes, catégorie = son Bip).

<details>
<summary><b>🗂️ Le dépôt</b></summary>

- `App/` — l'app SwiftUI (iOS 18+, Swift 6).
  - `Design/` — thème, catégories, mascottes vectorielles (`MascotView`), interactions (`InteractiveMascot`), babillage (`BipBabble`), voix des agents.
  - `Model/` — `AgentStore` (agents dans l'App Group, clés dans le Trousseau), `BridgeClient`, `Router`.
  - `Features/` — Agents, Sessions, Conversation, Live, Boîte, Réglages, Diagnostics, notifications.
- `NotificationService/` — extension qui met le texte complet, l'avatar du Bip et l'audio dans les notifications.
- `Packages/HermesKit` — client de l'api_server Hermes (SSE, sessions, runs, accords, `clarify`, pièces jointes).
- `Packages/VoiceKit` — capture, transcription sur l'iPhone, TTS en flux PCM, interruption à la voix.
- `bridge/` — le service Python côté serveur ([README](bridge/README.md)).
- `voices/` — les voix des Bips ([README](voices/README.md)).
- `docs/` — notes d'API ([`clarify`](docs/hermes-clarify-api.md), [fichiers](docs/aibox-fichiers.md)), captures, [terrain de jeu](docs/play/index.html) et bannière (`node docs/tools/banner.mjs` la régénère).
- `SPEC.md` — la spécification complète.

</details>

## 💬 Qui est derrière

Un projet perso de **Bob**, qui creuse l'IA au quotidien : agents, voix, auto-hébergement.
Les coulisses, les galères et les prochains Bips sont sur X : **[@Bob_AI_Digger](https://x.com/Bob_AI_Digger)** 👋

Construit avec [Hermes Agent](https://github.com/NousResearch/hermes-agent) de Nous Research, les voix [Kyutai](https://kyutai.org), et beaucoup de sessions avec Claude.

<p align="center"><sub>MIT · Fait avec ♥ et des Bips</sub></p>
