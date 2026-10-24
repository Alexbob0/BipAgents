[🇬🇧 English](grok-bot.en.md) · 🇫🇷 Français · [🇩🇪 Deutsch](grok-bot.de.md) · [🇪🇸 Español](grok-bot.es.md)

# Venir de Grok Bot (ou de ChatGPT dots, Muse, d'un autre agent)

Grok Bot n'a pas de bouton d'export : la seule façon d'emporter ce qu'il sait, c'est de lui demander de tout écrire.
Le message ci-dessous lui fait écrire son rôle, sa mémoire, les personnes, les tâches, les routines et les
compétences dans des fichiers, puis les mettre dans un zip que tu télécharges.

1. Copie le message et colle-le dans la conversation de ton Bot (un Bot à la fois).
2. Télécharge le fichier `bipagents-export.zip` qu'il joint à sa réponse. Relis-le si tu veux : tu peux retirer ce que
   tu ne veux pas emporter.
3. Dans BipAgents : **Ajouter un agent › Venir d'un autre assistant › Importer le fichier**. L'app te montre tout ce
   qui sera créé avant d'enregistrer quoi que ce soit.

Ce que le Bot écrit est sa version de sa mémoire : elle peut être incomplète. Les compétences arrivent éteintes et
les routines ne sont pas reprogrammées sans ton accord. Rien ne ressemblant à un mot de passe, une clé ou un numéro de
carte n'est gardé.

## Le message

```text
Je change d'assistant et je veux tout emporter. Crée un dossier bipagents-export dans ton espace de travail et écris-y, aussi complètement que possible, ces fichiers Markdown :

1. SOUL.md : ton rôle, ta personnalité, ton ton et toutes les règles durables que je t'ai données, à la deuxième personne (« Tu es… », « Tu ne fais jamais… »).
2. USER.md : tout ce que tu sais de moi : qui je suis, ma situation, mes goûts, mes préférences, ma façon d'écrire.
3. MEMORY.md : tout le reste de ta mémoire, un fait par ligne commençant par « - », avec la date (AAAA-MM-JJ) quand tu la connais.
4. people.md : chaque personne que tu connais, une section « ## Nom » avec son lien avec moi et ce qu'il faut savoir d'elle.
5. tasks.md : les tâches et projets en cours, une ligne « - » chacun, avec l'échéance et où ça en est.
6. routines.md : chaque routine ou tâche programmée, une section « ## Nom » avec l'horaire exact et le fuseau, la consigne complète, ce qu'elle produit et les apps qu'elle utilise.
7. skills/<nom>/SKILL.md : chaque compétence ou procédure que tu as apprise, en entier. Commence par :
---
name: <nom-en-minuscules-avec-tirets>
description: <quand l'utiliser, en une phrase>
---
puis quand l'utiliser, ce qu'il faut au départ, les étapes numérotées, comment vérifier et ce que tu rends.
8. apps.md : les noms des apps et services auxquels tu es connecté, un par ligne, sans identifiant.
9. conversations.md : un résumé par sujet de nos conversations passées, une section « ## Sujet (période) » chacun, avec les décisions prises.

Règles :
- Sois exhaustif : mieux vaut trop que pas assez. Fais plusieurs passes s'il le faut. Un fichier sans contenu : écris « (rien) ».
- N'invente rien. Ajoute « (incertain) » à ce dont tu n'es pas sûr.
- N'écris JAMAIS de mot de passe, code, jeton, clé d'API, numéro de carte ou de compte bancaire : mets « [retiré] » à la place.
- Écris dans la langue où je te parle.

Ensuite, fais un seul zip de ce dossier nommé bipagents-export.zip, joins-le à ta réponse pour que je le télécharge (si tu ne peux pas faire de zip, joins les fichiers un par un) et réponds « C'est prêt » avec le nombre de fichiers.
```
