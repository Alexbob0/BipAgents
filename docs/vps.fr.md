[🇬🇧 English](vps.md) · 🇫🇷 Français

# BipAgents sur un serveur loué (VPS)

Pas de machine allumée à la maison ? Un petit serveur loué fait l'affaire : tes agents tournent dans un datacenter,
et l'app les joint par Tailscale, comme à la maison. Rien n'est ouvert sur Internet à part SSH, et même SSH peut se
fermer une fois Tailscale en place.

## Ce qu'il faut savoir avant

- **Ton hébergeur a accès au disque du serveur.** Conversations, mémoire des agents et coffre y sont (le coffre est
  chiffré, mais sa clé est sur le même disque). Choisis un hébergeur de confiance, en Europe si tu préfères le RGPD.
- **Le modèle** n'y tourne pas (pas de GPU) : il est **dans le cloud** (OpenRouter, Mistral, OpenAI, Anthropic), ou
  sur **un PC de la maison** joint par Tailscale (l'installateur demande alors son adresse sur ton tailnet). Dans le
  cloud, chaque message part au fournisseur du modèle pour qu'il y réponde : avec OpenRouter, choisis dans ses réglages
  de confidentialité des fournisseurs qui ne gardent rien (zero data retention).
- **Les notifications** passent par Apple avec une clé de développeur, ou par le relais d'une autre installation
  BipAgents (celle d'un proche : `family add`).
- **Le coût** : le serveur, ≈ 7 € par mois (4 vCPU, 8 Go chez Hetzner) ; le modèle, payé à l'usage chez OpenRouter.
  Les voix des Bips tournent sur le processeur du serveur, sans frais en plus.

## 1. Choisir le serveur

| Usage | Taille | Exemple |
|---|---|---|
| Agents, voix des Bips | 4 vCPU, 8 Go | Hetzner CAX21 (ARM), CPX31 (x86) |
| La même chose + navigateur visible pour chaque agent | 4 à 8 vCPU, 16 Go | Hetzner CAX31 |

Système : **Ubuntu 24.04**. Un serveur ARM convient très bien. Son navigateur visible est alors Chromium, Google ne
publiant pas Chrome pour Linux ARM.

## 2. Le créer, préparé d'avance

Au moment de créer le serveur, colle le contenu de [`vps/cloud-init.yaml`](vps/cloud-init.yaml) dans le champ
**cloud-init** (« Cloud config », « User data » selon l'hébergeur), après y avoir mis **ta** clé SSH publique
(le contenu de `~/.ssh/id_ed25519.pub`). Il prépare tout seul, en quelques minutes :

- deux comptes : `admin` (le tien, avec sudo) et `bip` (BipAgents et ses agents, **sans** sudo) ;
- podman (les bacs à sable des agents) et Tailscale ;
- un pare-feu qui ne laisse entrer que SSH et le tailnet ;
- les mises à jour de sécurité automatiques.

## 3. Le brancher sur ton tailnet

```bash
ssh admin@<adresse-du-serveur>
sudo tailscale up --operator=bip --ssh
```

Ouvre le lien affiché pour ajouter le serveur à ton tailnet. Dans la console Tailscale, vérifie que **MagicDNS** et
**HTTPS Certificates** sont activés.

Avec `--ssh`, tu peux ensuite te connecter par le tailnet (`ssh admin@<nom-du-serveur>`) et fermer SSH côté Internet :
`sudo ufw delete allow OpenSSH`.

## 4. Installer BipAgents

Dans le compte `bip` :

```bash
sudo -iu bip
curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash
```

L'installateur voit qu'il est sur un serveur loué : pas de porte sur un réseau local, et le modèle dans le cloud est
proposé en premier. Il propose aussi le navigateur visible de chaque agent, puisque podman est là. À la fin, scanne le
QR code dans l'app.

## Ensuite

- **Sauvegardes** : active les sauvegardes ou instantanés de ton hébergeur. Tout est dans `/home/bip`.
- **Mises à jour** : le système se met à jour seul. Pour BipAgents : `bipagents update` dans le compte `bip` (il relance
  seulement ce qui a changé) ; à la première fois, il propose de le faire chaque nuit.
- **Désinstaller** : `bash ~/BipAgents/install.sh uninstall` dans le compte `bip`, puis supprime le serveur chez
  l'hébergeur.
