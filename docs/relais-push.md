# Relais de notifications (pour une publication App Store)

Décision du 2026-10-04 : **pas nécessaire tant que l'app n'est utilisée que par son auteur** (le bridge d'aibox envoie lui-même à APNs avec la clé `.p8` de l'équipe). Le jour d'une publication sur l'App Store, le relais tournera sur **une petite VM Hetzner**.

## Pourquoi un relais
La clé APNs `.p8` appartient à l'équipe qui publie l'app et ne peut pas être distribuée aux bridges des utilisateurs. Un service central, seul détenteur de la clé, reçoit les demandes des bridges et les transmet à APNs (modèle ntfy.sh pour les serveurs ntfy auto-hébergés).

## Fonctionnement prévu
1. L'app s'enregistre auprès du relais avec son device token APNs et reçoit un jeton opaque, qu'elle transmet à son bridge.
2. Le bridge envoie au relais `{jeton, type: MESSAGE|APPROVAL|SILENT, ids}` : jamais de texte (les payloads sont déjà génériques par défaut, cf. `bridge/README.md`).
3. Le relais signe le JWT ES256, envoie à APNs (HTTP/2, connexions persistantes) et supprime les tokens que APNs déclare invalides (410, `BadDeviceToken`).
4. La Notification Service Extension récupère le contenu sur le bridge de l'utilisateur, via son tailnet.

Garde-fous : authentification du bridge par jeton, quotas par jeton, journal sans contenu.

## Ordres de grandeur (à revérifier au lancement)
- Jusqu'à ~10 000 utilisateurs : une VM ~4–5 €/mois suffit.
- ~1 M d'utilisateurs : quelques centaines d'euros par mois, surtout du coût d'exploitation (multi-région, supervision, RGPD, abus).

## Côté code
Point de départ : `bridge/bipbridge/apns.py` (client APNs déjà écrit). Côté bridge, ajouter un mode « via relais » à côté du mode « APNs direct » actuel ; l'app n'a qu'à envoyer le jeton du relais au lieu du device token.
