[🇬🇧 English](relais-push.md) · 🇫🇷 Français

# Relais de notifications (pour une sortie sur l'App Store)

La clé APNs `.p8` et le compte Firebase appartiennent à l'équipe qui publie l'app et ne peuvent pas être distribués
aux bridges des utilisateurs. Un service central, seul détenteur des deux, reçoit les notifications des bridges et les
transmet à Apple et à Google (le modèle de ntfy.sh pour les serveurs ntfy auto-hébergés).

**Réalisé dans [`relay/`](../relay/README.fr.md)** (18/10/2026), en Rust : un petit binaire, sans base de données.
L'app inscrit son jeton d'appareil et reçoit un *jeton de relais* (le jeton d'appareil chiffré avec une clé que seul
le relais détient) ; son bridge envoie ce jeton de relais avec chaque notification. Aucun texte de message ne passe en
clair : le nom de l'agent et le début de son message sont scellés pour la clé du téléphone (lui seul les ouvre), et le
relais ne garde qu'une ligne neutre (« Nouveau message », « Approbation requise »…,
`contracts/notifications/loc-keys.json`) et des identifiants courts (dont celui de l'agent).

Toutes les installations passent par le relais : `[push] service_url = "https://push.bipagents.com"`, réglé par
l'installateur et, sur les installations plus anciennes, par la mise à jour du 12/11/2026. Avec lui, le bridge
n'envoie plus lui-même à Apple, même avec une section `[apns]` (qu'on peut retirer) ; les téléphones se réinscrivent
auprès du relais à leur prochain lancement.

## Ordres de grandeur (à revérifier au lancement)
- Un exemplaire (≈ 4 Mo de mémoire) couvre un grand nombre d'installations ; il peut tourner en conteneur
  « serverless » qui s'arrête au repos, ou sur une petite VM.
- Le coût grandit avec le nombre de notifications, pas avec le stockage : il n'y en a pas.
