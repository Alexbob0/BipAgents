[🇬🇧 English](privacy-compare.md) · 🇫🇷 Français

# Où vont tes conversations : BipAgents et les assistants grand public

**Vérifié le 22 octobre 2026.** Cette page compare ce que font **par défaut** les offres **grand public** (gratuites ou
payantes pour particuliers) de quelques assistants d'IA, d'après leurs propres pages, et ce que fait BipAgents, d'après
sa [politique de confidentialité](legal/privacy.fr.md) et [ce que les apps envoient](app-data.fr.md). Les offres
entreprise ou API de ces sociétés ont souvent d'autres règles. Ces règles changent : chaque ligne renvoie à sa source et
à sa date. Si tu vois une erreur, écris à support@bipagents.com.

## En un coup d'œil

| | Où sont gardées tes conversations | Entraînement de l'IA, par défaut | Combien de temps | Lecture par des personnes |
|---|---|---|---|---|
| **ChatGPT** (OpenAI) | Sur les serveurs d'OpenAI, dans ton historique jusqu'à ce que tu les supprimes | Oui, sauf si tu coupes « Améliorer le modèle pour tous » [1] | Supprimées : effacées sous 30 jours, sauf exceptions (sécurité, obligations légales) [2] | Non détaillé ici |
| **Gemini** (Google) | Avec ton compte Google (« Activité ») | Oui quand « Activité » est activée, ce qui est le réglage par défaut [3] | Effacée au bout de 18 mois par défaut ; 72 heures si l'activité est coupée [3] | Oui, une partie des conversations, gardées jusqu'à 3 ans [3] |
| **Claude** (Anthropic) | Sur les serveurs d'Anthropic | Seulement si « Aider à améliorer Claude » est activé ; l'option était pré-cochée quand le choix a été demandé en 2025 [4][5] | Jusqu'à 5 ans si l'option est activée, sinon 30 jours après suppression [6] | Non détaillé ici |
| **Meta AI** | Sur les serveurs de Meta | Meta utilise les échanges avec ses IA pour améliorer ses produits [7] | Non trouvé | Oui, par des personnes et des outils automatiques [7] |
| **Copilot** (Microsoft, nouvelle app depuis le 18 août 2026) | Sur les serveurs de Microsoft | Non pour ses « modèles de fondation », selon Microsoft [8] | Non précisé pour la nouvelle app | Non détaillé ici |
| **BipAgents** | Sur **ta place** (un compte à toi sur un serveur, séparé des autres) ou sur ton propre ordinateur | **Non** : Kerfy Studio n'entraîne aucune IA avec tes données [9] | Tant que tu gardes ta place ; effacées 30 jours après la suppression du compte [10] | Kerfy Studio ne lit pas ta place (voir les limites plus bas) [11] |

## Ce qui a changé récemment

- **ChatGPT et la justice américaine.** En mai 2025, dans le procès du New York Times contre OpenAI, un tribunal a
  ordonné à OpenAI de garder toutes les conversations, même supprimées [12]. Cette obligation a pris fin pour les
  nouvelles conversations le 26 septembre 2025 ; ce qui avait été gardé entre-temps reste conservé [13]. Les
  conversations de l'Espace économique européen, de la Suisse et du Royaume-Uni n'étaient pas concernées, selon
  OpenAI [14]. En janvier 2026, un juge a confirmé la remise de 20 millions de conversations anonymisées aux
  plaignants [15].
- **Meta AI et la publicité.** Depuis le 16 décembre 2025, Meta se sert des échanges avec Meta AI pour personnaliser
  les contenus et les publicités, sauf les sujets sensibles [16]. Selon la presse, il n'y a pas de bouton pour le
  refuser, et l'Union européenne, le Royaume-Uni et la Corée du Sud étaient exclus au départ [17].
- **Claude.** Depuis l'automne 2025, les conversations des offres Free, Pro et Max peuvent servir à l'entraînement si
  la personne l'accepte [4]. La mémoire est activée par défaut [18].
- **Copilot.** La nouvelle app de Microsoft (18 août 2026) dit ne pas entraîner ses modèles de fondation sur tes
  conversations ; l'ancienne le faisait par défaut, avec une option pour le couper, et gardait les conversations
  18 mois [8][19].

## Mémoire

Tous ces assistants ont désormais une mémoire qui retient des choses sur toi d'une conversation à l'autre : ChatGPT
[20], Gemini [3], Claude (activée par défaut) [18], Meta AI [21], Copilot [22]. Elle est gardée par l'entreprise. Dans
BipAgents, la mémoire de tes agents est un fichier sur ta place, que tu peux lire et corriger dans l'app.

## Ce que BipAgents ne promet pas

- **Avec l'offre tout compris, Kerfy Studio loue le serveur** (OVHcloud ou Hetzner, en Europe). Techniquement,
  l'hébergeur et Kerfy Studio pourraient accéder à ta place. Le logiciel de Kerfy Studio ne la lit pas, l'accès aux
  comptes chez les hébergeurs est réservé à la société, et un accès n'aurait lieu que sur obligation légale [11].
  OVHcloud fait aussi une image quotidienne de ses serveurs, que Kerfy Studio ne chiffre pas [23].
- **L'IA lit ce que tes agents lui envoient**, pour répondre. Avec l'offre, ça passe par EUrouter (Amsterdam), qui
  indique ne garder ni les messages ni les réponses, mais garde leurs métadonnées jusqu'à 12 mois, puis par le
  fournisseur qui fait tourner le modèle choisi [24].
- **« Signaler cette réponse »** envoie le texte de la réponse à Kerfy Studio, quand tu l'envoies [25].
- Les garanties des apps sont décrites dans [ce que les apps envoient](app-data.fr.md), avec ses limites.

Avec ton propre ordinateur ou serveur, rien de tout cela ne passe par Kerfy Studio : seules les notifications passent
par son relais, scellées [26].

## Sources

Consultées le 22 octobre 2026 sauf autre date indiquée.

1. OpenAI, *Data Controls FAQ* — https://help.openai.com/en/articles/7730893-data-controls-faq
2. OpenAI, *How to delete your account* — https://help.openai.com/en/articles/6378407-how-to-delete-your-account
3. Google, *Gemini Apps Privacy Hub* (mis à jour le 19 octobre 2026) — https://support.google.com/gemini/answer/13594961
4. Anthropic, *Updates to our consumer terms* (28 août 2025) — https://www.anthropic.com/news/updates-to-our-consumer-terms
5. TechCrunch, 28 août 2025 — https://techcrunch.com/2025/08/28/anthropic-users-face-a-new-choice-opt-out-or-share-your-data-for-ai-training/
6. Anthropic, *How long do you store my data?* (1er juillet 2026) — https://privacy.claude.com/en/articles/10023548-how-long-do-you-store-my-data
7. Meta, *Privacy matters: Meta's generative AI features* (septembre 2023) — https://about.fb.com/news/2023/09/privacy-matters-metas-generative-ai-features/
8. Microsoft, *Copilot activity history* — https://support.microsoft.com/en-us/privacy/microsoft-copilot/activity-history ; *overview* — https://support.microsoft.com/en-us/privacy/microsoft-copilot/overview
9. [Politique de confidentialité de BipAgents](legal/privacy.fr.md), section 6
10. Même texte, section 5.2
11. Même texte, section 5.1
12. CyberInsider, mai 2025 — https://cyberinsider.com/us-court-orders-openai-to-preserve-all-chatgpt-logs-indefinitely/
13. Engadget, 11 octobre 2025 — https://www.engadget.com/ai/openai-no-longer-has-to-preserve-all-of-its-chatgpt-data-with-some-exceptions-192422093.html
14. OpenAI, *How we're responding to The New York Times' data demands* (mis à jour le 22 octobre 2025) — https://openai.com/index/response-to-nyt-data-demands/
15. ABA Journal, 8 janvier 2026 — https://www.abajournal.com/news/article/chatgpt-creator-must-turn-over-20m-chat-logs-in-copyright-litigation-federal-judge-says
16. Meta, *Improving your recommendations with AI at Meta* (1er octobre 2025) — https://about.fb.com/news/2025/10/improving-your-recommendations-apps-ai-meta/
17. Business Today, 3 octobre 2025 — https://www.businesstoday.in/technology/news/story/meta-to-use-ai-chat-data-for-ads-and-posts-from-december-16-and-you-cant-say-no-496635-2025-10-03
18. Anthropic, *Use Claude's chat search and memory* — https://support.claude.com/en/articles/11817273-use-claude-s-chat-search-and-memory-to-build-on-previous-context
19. Microsoft, *Privacy FAQ for Microsoft Copilot* (ancienne app) — https://support.microsoft.com/en-us/microsoft-copilot/privacy-faq-for-microsoft-copilot
20. OpenAI, *Memory in ChatGPT* — https://help.openai.com/en/articles/8590148-memory-in-chatgpt
21. Meta, *Building toward a smarter, more personalized assistant* (27 janvier 2025) — https://about.fb.com/news/2025/01/building-toward-a-smarter-more-personalized-assistant/
22. Microsoft, *Copilot privacy controls* — https://support.microsoft.com/en-us/privacy/microsoft-copilot/privacy-controls
23. [Politique de confidentialité de BipAgents](legal/privacy.fr.md), section 5.5
24. Même texte, section 5.4
25. [Ce que les apps envoient](app-data.fr.md), section 3
26. [Politique de confidentialité de BipAgents](legal/privacy.fr.md), section 4.1
