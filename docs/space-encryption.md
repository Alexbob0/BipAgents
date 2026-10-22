# A place's space encrypted at rest (level 1)

Design note, 2026-10-22. Code: `bridge-rs/src/space.rs` (the bridge), `keys-rs/` (Kerfy's key service),
`installer/bipinstall/space.py` (`bipagents encrypt`), `installer/cloud/space-hardening.sh`. Off by default in this
first version; turned on per place. Résumé en français à la fin.

## What it is for

A server of the all-included offer is a VPS. Its provider (Hetzner, OVHcloud…) can copy its disk, keep snapshots,
take a disk out, or boot the server in rescue mode and read the disk. Today everything of a person's space (Hermes'
home: profiles, sessions, memories, SOUL.md, skills; the bridge's data: Carnet, vault, uploads, sent files, outbox,
pages, history, audit logs; the backup key; the browsers' profiles) is in clear on that disk.

Level 1 makes that disk unreadable on its own: the space lives in an encrypted folder whose key is never on the
server's disk.

## The honest properties (the owner's choice: option A, unattended unlock with a split key)

The folder's key is two random 32-byte shares put together (XOR): one kept on the server's disk, the other kept by
Kerfy Studio's key service (`keys-rs/`), which must run at **another provider** than the places' servers. Each share
alone says nothing about the key. After a reboot the place's bridge asks the key service for the other share, with the
place's own token, opens the folder and starts the agents. **No phone, no person**: agents and routines keep working
after any reboot.

| Against | Protected? |
|---|---|
| A copy of the disk read away from the server: snapshots, images, backups of the volume, a disk taken out or resold | **Yes**: the server's share alone opens nothing; the key service gives the other only to that server's address |
| The provider booting the server in rescue mode and reading the disk | **Detected, not prevented**: with the disk it has the place's share and token, and from the server's own address it can ask the key service. That request is logged (place, time, address) at a moment the place did not reboot; the guichet can compare it with the heartbeat (uptime) and alert. Without that request the disk is unreadable |
| Kerfy Studio | **No**: Kerfy runs the key service and could put a share together with a disk's. The privacy policy's sentence about Kerfy stays as it is |
| Someone reading the running server's memory (the hypervisor, a malicious code update) | **No**. Updates come only from public commits |
| The AI model's provider | **No**: it sees what is sent to the model (already in the privacy policy) |

The key service must be at another provider than the servers it serves; if both were at the same provider, it could
hold both shares. Today's servers: Hetzner then OVHcloud (server-providers): the key service would go to a third one
(e.g. Scaleway, or a small EU VPS elsewhere), with its own database backups at yet another place or encrypted.

What the key service does, and only that (`keys-rs/src/lib.rs`):

- `POST /v1/share` with the place's token (the guichet's place token, the one its backups already use). The first
  time, the request carries the guichet's **Ed25519-signed grant** for that token (the same key that signs the
  servers' orders): the service makes the place's share and remembers the address it enrolled from.
- It gives the share only to that address (`bind_address`), at most `max_per_day` times a day (24), and logs every
  request: place, time, address, outcome. It never sees the server's share nor the key.
- The guichet's signed orders: `stop` (abuse, legal: the place can't reopen after a reboot), `resume`, `forget` (the
  place deleted: its share erased, the space unreadable for good), `rebind` (the server's address changed).
- A copy of the service's database gives shares but no way to enrol or to read anything without the places' disks.

## Mechanism: gocryptfs

| | fscrypt v2 | gocryptfs (FUSE) | LUKS file container |
|---|---|---|---|
| Filesystems | ext4, f2fs (and UBIFS, CephFS); **not btrfs** in mainline | any (ext4, btrfs, xfs) | any |
| Privileges | root once to turn the ext4 `encrypt` feature on; then a user can add a v2 key (`FS_IOC_ADD_ENCRYPTION_KEY`, kernel quota) | none at runtime (`fusermount3` is setuid); `user_allow_other` in /etc/fuse.conf only if containers of other ids must read | root at every open (`cryptsetup open`) |
| Existing data | a new empty folder, then copy | a new folder, then copy | a new container, then copy |
| Leaks | sizes, tree shape, times | sizes (rounded), tree shape | the container's size only |
| Key held in | the kernel (filesystem keyring) | the gocryptfs process | the kernel (dm-crypt) |

The offer's servers are Ubuntu 24.04 (ext4) and aibox is Fedora (btrfs by default): fscrypt is out for btrfs, LUKS
needs root at each unlock. **gocryptfs** works on both, without root at runtime: chosen. It encrypts contents
(AES-256-GCM) and names (EME); it shows sizes and the tree's shape. Its master key is in `gocryptfs.conf`, sealed with
the password (scrypt); the password is our 256-bit key in base64url, so scrypt is kept cheap (`-scryptn 10`).

It runs under systemd as a transient unit of its own (`bipagents-space-open`): restarting the bridge (an update) leaves
the folder open. The password goes through a FIFO in `$XDG_RUNTIME_DIR` (memory), never a file or a command line. The
`-allow_other` option (for the browsers' containers, if they run as other user ids) makes gocryptfs turn on
`default_permissions`, so the files' own modes still apply; the home folders are 0700 besides.

Not covered by the folder, and handled by the server's settings (`installer/cloud/space-hardening.sh`, run as root
once): the swap file (now encrypted with a fresh random key at each boot), core dumps (none kept), the journal (in
memory only). Also not covered: the containers' image layers and anything a container writes outside its volumes
(their volumes are inside: they are under the bridge's data folder), and `/tmp`.

## Layout

    ~/.local/share/bipagents/space/        outside
        space.json        on/off, the moved folders, the locked door, the services to start
        share             this server's share (0600)
        keyservice.json   the key service's address, the place's token and grant (0600)
        locked.json       while locked: since, why, tries (the seat agent reads it)
        cipher/           gocryptfs
        open/             the open folder; open/.bipagents-space marks it open
    ~/.hermes                 -> open/hermes
    ~/.config/bipagents       -> open/config      (bridge.toml, backup.json with the backup key, keys.json…)
    ~/.local/state/bipagents  -> open/state       (backup staging copies, import files)
    the bridge's data folder: its entries move to open/data and bridge.toml's data_dir points there; its folders keep
    a link at their old place; what stays outside: bridge/ (binaries), voices/, lan/ (the door's certificate), space/,
    migrations.json

Links replace whole folders only (a file link would be replaced by an atomic write with a plain file, outside).

## Boot order

1. The bridge's service starts as usual. `bipbridge serve` finds the space on but not open: it serves the **locked
   door** (its usual port, and the LAN/Internet door with its certificate): `/health` says
   `{"ok": false, "space": "locked", "reason": …}`, everything else answers 423.
2. It asks the key service (5, 10, 20, 40, 80, 160 s, then every 5 minutes), writing why it is still locked to
   `locked.json`: the seat agent's heartbeat says `"locked": "key service unreachable"` (the guichet must treat it as
   locked, not as an outage, and alert Kerfy when it lasts — guichet side still to do).
3. Opened: the agents' services start (`systemctl --user start hermes-gateway…`); each of them, and the backup,
   browser refresh and update services, carries a drop-in `ConditionPathExists=…/open/.bipagents-space`, so at boot
   they were skipped, not failed. The timers keep running and their services run once the folder is open.
4. The bridge runs itself again (exec, same PID) and starts normally.

## Reboots

With option A, a reboot needs no device. What it needs is the key service: the nightly kernel reboot
(unattended-upgrades, 00:00-00:44) should only be taken when the key service answers. Proposed, not built: the seat
agent replaces unattended-upgrades' own reboot: in the night window, if `/var/run/reboot-required` and the key
service's `/health` answers, reboot; otherwise wait (retry each night window), never more than 24 h for a security
fix. Missed work while locked: Hermes' cron catches up as its scheduler does at a gateway start (to check on Hermes'
side); systemd timers with `Persistent=true` whose run was skipped by the condition should be started once after
the unlock (not built). **Canonical Livepatch** (Ubuntu Pro) avoids most kernel reboots; its free tier covers up to
5 machines for personal and small-scale commercial use: a paid Ubuntu Pro subscription would be needed for the fleet
(to price; check the current terms at ubuntu.com/pro).

## Turning it on

- A server of the offer: its setup gets `"keys": {"url", "grant", "signature"}` (with `"backup"`, whose token is the
  place's token). The installer encrypts the space at the end of its install, before anything of anyone is there.
  Making it the default is one setting at the guichet (put `keys` in every place's setup); the guichet must also
  issue the grant (`{"v": 1, "kind": "keys-grant", "place", "server", "token_sha256", "issued"}` signed as orders are)
  and send `forget`/`stop` orders (`{"kind": "keys-order", "action", "place", "reason", "expires"}`) when a place is
  deleted, moved or stopped — **guichet side still to do**. Run `space-hardening.sh` from cloud-init then.
- An existing space: `bipagents encrypt` (with the key service's address and the place's token). It explains, writes
  the drop-ins, stops the bridge and the agents' services, and the bridge does the rest: checks the free disk (the
  data's size plus 10 % and 256 MB), makes the shares and the folder, copies each folder in and compares files and
  bytes, sets the originals aside and puts links in their place, points `data_dir` inside, checks no configured path
  stays outside, then **closes the folder and opens it again from scratch** (the key service asked again) and compares
  again. Only then are the originals deleted (`--keep-rollback` keeps them until `bipagents encrypt finish`;
  `bipagents encrypt rollback` puts them back while kept). Any failure before the end puts everything back. Honest
  limit: deleted unencrypted files may stay readable in the disk's free blocks until overwritten (SSD, a provider's
  volume); a new place encrypted from its first boot has no such past.
- Self-hosted installs: not offered in this version. At home the threat is a stolen machine, for which full-disk
  encryption with a TPM (systemd-cryptenroll) is the right tool, and a key service at Kerfy would make a home server
  depend on Kerfy at each boot. `bipagents encrypt --key-service URL` works on any Linux account for tests.

## Backups and moves

The backup key (`backup.json`) is now inside the folder; the hourly backup runs only while it is open (its service
waits for it). Restic's repository at the guichet stays encrypted with that key, which the phones also hold (GET
/v1/backup): restoring on a new server works as before (`installer/bipinstall/backup.py`, the phone gives the key
back), and the new place gets a new encrypted space with new shares; the guichet sends `forget` for the old one.

## Families

Each person's place (each Linux account) has its own folder, its own shares, its own token at the key service.
Nothing is shared. One person's place locked does not affect the others.

## What is built, what is left (2026-10-22)

Built: the bridge (locked door, unlock loop, enable with checks and rollback, lock/unlock/finish/rollback commands,
`/health` `space`), the key service (enrol by grant, release by token and address, rate limit, log, signed orders, CLI),
the installer (`bipagents encrypt`, drop-ins, the offer's setup `keys`, the heartbeat's `locked`), the hardening
script. Tests: bridge black-box with a fake backend and a scripted key service; key service black-box; installer.

Left: the guichet (grants, orders, `locked` in the heartbeat and its alerts, the key service's address in the setups,
hosting the key service at another provider); the reboot coordination; the catch-up of skipped timers; a real run with
gocryptfs on Linux (not possible on the Mac where this was built); the apps (nothing needed: a locked place answers
423 and `/health` says so — a plain message in the apps is a small next step).

## Option B (study): confidential VMs, the key released only to our measured code

With AMD SEV-SNP or Intel TDX the VM's memory is encrypted by the processor with a key the host never sees, and the
processor signs an **attestation report** with the measurement (hash) of what booted: firmware, kernel, initrd,
kernel command line. A key service that releases the share only after checking that report against the measurement
of our public image closes both gaps of option A: the host can't read memory, and a rescue boot (other code) gets
nothing. Kerfy could still change the expected measurement, but that is a public, auditable act if the image is built
reproducibly from public commits.

Providers in the EU (checked 2026-10-22):

- **IONOS Cloud**: Confidential VMs with SEV-SNP, EU data centres only (Frankfurt `de/fra/2` named in the docs), price
  on quote. IONOS gives no attestation service: the customer runs one (their reference: SNPGuard) which checks the
  SHA-384 launch measurement (firmware, kernel, initrd, command line) and releases the volume key. Limits: a custom
  Linux image with a `LAUNCH_ARTIFACTS` partition, no cloud-init v1, no live migration (host maintenance = restart),
  an in-guest reboot terminates the VM (power-cycle through the API), no boot-volume snapshots, fixed size.
  https://docs.ionos.com/cloud/compute-services/compute-engine/confidential-computing-vm/confidential-vm-faq
- **Azure, Google Cloud, AWS**: SEV-SNP (and TDX) confidential VMs in EU regions (e.g. Azure West/North Europe), with
  their own attestation services; more expensive than our VPS, and US companies (GDPR-wise acceptable with EU
  regions, per the model-origin rule, but a positioning question).
- **OVHcloud**: confidential computing pages are about bare-metal servers (Intel SGX, AMD features of the processor);
  no SEV-SNP VM product found. **Hetzner**: no confidential VMs; a dedicated AMD EPYC server could host our own
  SEV-SNP VMs (we would run the hypervisor; the physical host is then the remaining party). **Scaleway**: none found.
  **VPSBG** (Bulgaria) announced SEV-SNP cloud servers in 2025.

Work for B: a reproducible, measured image (kernel + initrd with the bridge, no cloud-init v1); the key service checks
SNP reports (AMD's certificate chain VCEK/ASK/ARK) against the published measurement; the place's disk encrypted as a
whole (LUKS) with the released key; updates become image updates with a new measurement published first. Weeks of
work and a provider change for the offer's places; prices to ask IONOS.

## Privacy wording (draft, to switch on only once encryption is on by default on the offer)

Keep as it is: « techniquement l'hébergeur et Kerfy Studio pourraient… » until then. Then, for the host only:

- FR : « L'hébergeur ne peut pas lire ton espace sur son disque : il y est chiffré, et la moitié de sa clé est gardée
  ailleurs, chez un autre prestataire. Kerfy Studio, qui garde cette moitié, pourrait techniquement y accéder. »
- EN: "Your host can't read your space on its disk: it is encrypted there, and half of its key is kept elsewhere, at
  another provider. Kerfy Studio, which keeps that half, could technically access it."

## Résumé en français

L'espace de chaque place est chiffré sur le disque du serveur (gocryptfs : fonctionne sur l'ext4 d'Ubuntu et le btrfs
de Fedora, sans droits root au déverrouillage). Sa clé est en deux moitiés : une sur le serveur, l'autre au service de
clés de Kerfy Studio, chez un autre prestataire. Après un redémarrage, le bridge démarre « verrouillé » (sa porte et
/health répondent, le reste dit 423), va chercher l'autre moitié, ouvre le dossier, lance les agents et repart
normalement : aucun téléphone, personne n'a rien à faire. Ce que ça protège : une copie du disque (instantanés,
images, disque retiré) ne s'ouvre pas ailleurs. Ce que ça ne protège pas : Kerfy Studio (qui garde l'autre moitié), la
mémoire du serveur en marche, et un hébergeur qui démarrerait le serveur en mode secours pour demander la moitié depuis
l'adresse du serveur (demande journalisée et détectable, pas empêchée). `bipagents encrypt` chiffre un espace existant
en place, vérifie chaque copie, referme et rouvre de zéro avant d'effacer les originaux, et remet tout comme avant en
cas d'échec. Sur l'offre, l'activer par défaut = un réglage au guichet (« keys » dans la mise en place de chaque place),
une fois le côté guichet fait. L'option B (VM confidentielles SEV-SNP avec attestation, IONOS en UE) fermerait les deux
trous restants ; étude ci-dessus.
