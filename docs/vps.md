🇬🇧 English · [🇫🇷 Français](vps.fr.md)

# BipAgents on a rented server (VPS)

No machine running at home? A small rented server does it: your agents run in a datacenter, and the app reaches them
through Tailscale, as at home. Nothing is open on the Internet but SSH, and even SSH can be closed once Tailscale is
in place.

## Before you start

- **Your provider has access to the server's disk.** Conversations, the agents' memory and the vault are there (the
  vault is encrypted, but its key is on the same disk). Pick a provider you trust, in Europe if you prefer the GDPR.
- **The model** does not run there (no GPU): it is **in the cloud** (OpenRouter, Mistral, OpenAI, Anthropic), or on
  **a PC at home** reached through Tailscale (the installer then asks for its address on your tailnet).
- **Notifications** go through Apple with a developer key, or through the relay of another BipAgents install (a
  relative's: `family add`).
- **Cost**: a few euros a month.

## 1. Pick the server

| Use | Size | Example |
|---|---|---|
| Agents, the Bips' voices | 4 vCPU, 8 GB | Hetzner CAX21 (ARM), CPX31 (x86) |
| The same + a visible browser for each agent | 4 to 8 vCPU, 16 GB | Hetzner CAX31 |

System: **Ubuntu 24.04**. An ARM server works well; its visible browser is then Chromium (Google ships no Chrome for
Linux on ARM).

## 2. Create it, prepared in advance

When you create the server, paste [`vps/cloud-init.yaml`](vps/cloud-init.yaml) in the **cloud-init** field (« Cloud
config », « User data » depending on the provider), after putting **your** public SSH key in it (the content of
`~/.ssh/id_ed25519.pub`). In a few minutes it sets up:

- two accounts: `admin` (yours, with sudo) and `bip` (BipAgents and its agents, **without** sudo);
- podman (the agents' sandboxes) and Tailscale;
- a firewall that lets in only SSH and the tailnet;
- automatic security updates.

## 3. Join your tailnet

```bash
ssh admin@<server-address>
sudo tailscale up --operator=bip --ssh
```

Open the link it shows to add the server to your tailnet. In the Tailscale console, check that **MagicDNS** and
**HTTPS Certificates** are on.

With `--ssh` you can then log in through the tailnet (`ssh admin@<server-name>`) and close SSH on the Internet side:
`sudo ufw delete allow OpenSSH`.

## 4. Install BipAgents

In the `bip` account:

```bash
sudo -iu bip
curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash
```

The installer sees it runs on a rented server: no door on a local network, and the model in the cloud is offered
first. It also offers each agent's visible browser, since podman is there. At the end, scan the QR code in the app.

## Afterwards

- **Backups**: turn on your provider's backups or snapshots; everything is in `/home/bip`.
- **Updates**: the system updates itself. For BipAgents: `cd ~/BipAgents && git pull` in the `bip` account, then
  restart its services (`systemctl --user restart bipagents-bridge`).
- **Uninstall**: `bash ~/BipAgents/install.sh uninstall` in the `bip` account, then delete the server at the
  provider.
