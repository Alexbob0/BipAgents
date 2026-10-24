[🇬🇧 English](relais-push.md) · 🇫🇷 Français

# Relais de notifications (pour une sortie sur l'App Store)

La clé APNs `.p8` et le compte Firebase appartiennent à l'équipe qui publie l'app et ne peuvent pas être distribués
aux bridges des utilisateurs. Un service central, seul détenteur des deux, reçoit les notifications des bridges et les
transmet à Apple et à Google (le modèle de ntfy.sh pour les serveurs ntfy auto-hébergés).

**Réalisé dans [`relay/`](../relay/README.fr.md)** (18/10/2026), en Rust : un petit binaire, sans base de données.
L'app inscrit son jeton d'appareil et reçoit un *jeton de relais* (le jeton d'appareil chiffré avec une clé que seul
le relais détient) ; son bridge envoie ce jeton de relais avec chaque notification. Aucun texte de message ne passe :
les aperçus sont remplacés par le texte générique traduit, et l'extension de notification de l'app va chercher le
contenu sur le bridge de la personne.

Le bridge envoie encore lui-même à Apple quand sa config contient la clé de l'équipe (`[apns]`, les installations de
l'éditeur) ; toutes les autres installations passent par le relais (`[push] service_url`), que l'installateur règle dès
que le relais est en ligne, pour la sortie des apps sur les stores.

## Ordres de grandeur (à revérifier au lancement)
- Un exemplaire (≈ 4 Mo de mémoire) couvre un grand nombre d'installations ; il peut tourner en conteneur
  « serverless » qui s'arrête au repos, ou sur une petite VM.
- Le coût grandit avec le nombre de notifications, pas avec le stockage : il n'y en a pas.
