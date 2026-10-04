# Addendum SPEC — envoi de fichiers (PDF, Excel, texte…)

Décision du 2026-10-04 : l'app doit envoyer **tous types de fichiers**, comme Telegram aujourd'hui.

## Constat
- L'`api_server` Hermes v0.21.5 refuse les fichiers non-image dans `content` (`400 unsupported_content_type`, cf. `api-server.md` § Limitations).
- Les adaptateurs de messagerie (Telegram, SimpleX) téléchargent le document dans un cache local du conteneur et passent **son chemin** à l'agent, qui le lit avec ses outils (`read_file`, terminal, extraction PDF…).
- `POST /v1/artifacts/upload` (vérifié sur aibox le 2026-10-04) : réservée au broker de l'extension navigateur (404 sans `browser.extension_control.enabled`), stockage éphémère 5 min, lecture unique, aucun chemin disque, aucun consommateur côté agent. **Inutilisable pour donner un fichier à l'agent.**

## Mécanisme retenu (même principe que Telegram)
1. L'app envoie le fichier au **bridge** : `POST https://aibox.example.ts.net:8643/v1/files` (multipart : `agent`, `file`; Bearer clé bridge).
2. Le bridge l'écrit dans un dossier **déjà visible par l'agent** : un volume monté dans le conteneur (comme `/workspace` aujourd'hui) ou le home Hermes monté (`~/hermes-agent/hermes-home` → `/home/hermes/.hermes`). Choix final à faire côté aibox.
3. Réponse : `{"path": "<chemin vu par le conteneur>", "filename": "...", "size": n}`.
4. L'app ajoute au message une ligne de référence, puis l'envoie normalement par `chat/stream` :
   `[Pièce jointe : releve.pdf (application/pdf, 220 Ko) → /home/hermes/.hermes/profiles/vie/uploads/2026-10/…-releve.pdf]`
   Implémenté dans `HermesKit` (`MessageInput.prepared(uploader:)`, `BridgeDocumentUploader`).

Le Claude d'aibox ajoutera `POST /v1/files` au contrat C2 du SPEC.

## À faire côté aibox (bridge, §B3)
- Route `POST /v1/files` : limite de taille (ex. 50 Mo), noms de fichiers assainis, permissions 640, purge > 30 jours.
- Les images continuent d'être envoyées en ligne (`image_url` data URI ≤ 1600 px), sans passer par le bridge.
