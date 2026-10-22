🇬🇧 English · [🇫🇷 Français](README.fr.md)

# The BipAgents installer

One command, run in the account of the person the agents are for — no admin rights:

```bash
curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash
```

Add `--dry-run` to see every step and command without changing anything.

## What it does

1. **Looks at the machine**: system, memory, processor, GPU, Tailscale.
2. **Asks where the model runs** — usually *not* on this machine:
   - another machine of your network (a PC with a GPU, a Spark, a Mac): it finds the OpenAI-compatible servers by
     itself (Ollama, LM Studio, vLLM, llama.cpp…) and lists their models;
   - a cloud API (OpenRouter, OpenAI, Anthropic, Mistral) with your key;
   - this machine, when it has the room.

   It checks that the model answers, and whether it reads images (photos are then sent to it as they are).
3. **Picks a setup that fits**:

   | Profile | Typical machine | Model | Voices | Agents' browser |
   |---|---|---|---|---|
   | Light (default) | Mac mini 16 GB | elsewhere (network or API) | Pocket (CPU) | on demand, one at a time |
   | Local | Mac 24–32 GB, PC with GPU | on this machine | Pocket | on demand |
   | Large | Linux server ≥ 64 GB | elsewhere | Pocket + your voice model (if one already runs) | permanent, one per person |

   On a 16 GB Mac mini with the model elsewhere: about 1 GB per person plus the voices — a family fits.
4. **Installs Hermes** with its official installer, at the version tested with BipAgents (`517b5e10f`), applies the
   patches of `hermes/patches/` (the agent's questions in the app, photos kept for follow-up questions), as **one gateway per person** (Hermes' multiplex mode): every
   agent is a profile reached at `/p/<agent>/` with its own key. New agents are picked up without a restart.
5. **Creates the first agents**, named by the person, from kinds (Everyday, Wellness, Finances, Home, Work, Learn, Creative, Tech) or a custom « Other » (its role, tone, rules, language and colour, described by the person),
   each with a starting personality (`SOUL.md`) and joined to Bot Mode so they can talk to each other.
6. **Sets up the voices** (Pocket TTS, one server per machine, shared by everyone) and **the bridge** (one program in
   Rust: this version's binaries downloaded from the GitHub release `bridge-<key>` and checked against its
   `SHA256SUMS`, into `~/.local/share/bipagents/bridge/<key>/`; its config in `~/.config/bipagents/bridge.toml`, keys
   generated, its local-network door), as services that start on their own: launchd on macOS, systemd user units on
   Linux. Builds exist for Linux x86_64 and ARM64 (Windows runs BipAgents in WSL2) and macOS; on any other machine
   the installer stops before changing anything, and runs a bridge built there with
   `BIPAGENTS_BRIDGE_BUILD=<folder of bipbridge and bip-browser-mcp>` (`cargo build --release` in `bridge-rs/`).
   `bipagents bridge` says which version runs; `bipagents bridge rust` fetches it again and restarts it.
7. **Gives each agent its visible browser** (Linux with podman, if you want it): a real Chrome on a desktop of its
   own, which you watch live and take over from the app (Bureau, behind Face ID), and in which the vault fills
   passwords without the agent seeing them. See [`browser/`](../browser/README.md).
8. **Publishes the install on the tailnet** (`tailscale serve`, never on the Internet). Without the right to do it
   (another person's account on Linux), it prints the commands for the administrator.
9. **Shows one QR code**: scanned in the app, it adds every agent of the install.

## On a rented server (VPS)

The installer recognises a rented server (Hetzner, OVH, Scaleway, AWS…): no door on a local network, and the model
in the cloud offered first. The server is prepared in advance with a cloud-init file: see
[BipAgents on a rented server](../docs/vps.md).

On a place of the all-included offer (`--cloud` with the guichet's setup), the place backs itself up every hour
(`bipagents backup`, restic): encrypted with a key made on the place and kept by the person's phones, sent to the
guichet, which can't read it. After an outage the place moves to another server and the phone gives it its key back.

## On Windows

Windows 11 (22H2 or later), in PowerShell opened as administrator:

```powershell
irm https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.ps1 | iex
```

The agents live in **WSL2** (Ubuntu), installed exactly as on a Linux server — sandboxes, voices and browsers
included. `install.ps1` prepares what Windows needs, each step only when needed, then runs this installer there:

- WSL2 and Ubuntu (Ubuntu asks for a Linux user name and password the first time; Windows may ask for a restart:
  run the command again afterwards);
- WSL's network **mirrors** Windows' (`networkingMode=mirrored` in `%USERPROFILE%\.wslconfig`): the local network
  reaches the bridge's door, and a model server running on Windows (LM Studio, Ollama…) is found at `127.0.0.1`;
- in Ubuntu: systemd, git, curl, podman, and « linger »;
- **Windows' Tailscale**, signed in (installed with winget if you agree): it publishes the agents on your tailnet —
  the installer drives it from WSL;
- a **« BipAgents » task at logon** that keeps WSL awake (Windows stops it once no window uses it).

Keep the PC on and awake (Settings › System › Power: never sleep when plugged in); the agents start when you log in
to Windows. Update: `wsl -d Ubuntu-24.04 -- bash -lc 'bipagents update'`. Another distribution:
`$env:BIPAGENTS_DISTRO = '…'` before the command. Remove: uninstall BipAgents in Ubuntu (below), then
`Unregister-ScheduledTask BipAgents` (and `wsl --unregister Ubuntu-24.04` to delete Ubuntu with everything in it).

A Hermes installed on Windows itself (the RTX Spark's agent setup, `install.ps1` of Hermes) is not used: its agents
run without a sandbox, and the bridge does not run on Windows yet. It stays as it is, next to BipAgents.

## If you already have Hermes

The installer sees it and plugs into it **without touching your settings**: no question about the model, and your
model, tools, platforms (Telegram…) and agents stay as they are. It asks which agents (profiles) to show in the app,
your main profile included, and shows everything it will change before it starts:

- Hermes' local API, when it is off: on, on 127.0.0.1, on a port of your block, with a new key;
- a key for each shown agent that has none;
- BipAgents' patches in Hermes' code, if you want them and your version is the tested one;
- a service for your gateway when it is not running, or a restart when it already runs as a service (one you started
  by hand, you restart yourself).

All of it is written down in `~/.config/bipagents/existing-hermes.json`: uninstalling puts every setting back and
takes the patches off. `--all` never removes a Hermes that was there before BipAgents. When your agents' tools do not
run in a sandbox, the bridge keeps no passwords (no vault): it tells you so, without changing their tools. It needs a
Hermes that serves several agents at one address (`hermes update` otherwise).

## Updating BipAgents

`bipagents update` (or `curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash -s --
update`, the same as `bash ~/BipAgents/install.sh update`) brings this account's install up to date. `bipagents` alone
lists its commands (update, voice, hermes-update, family add, export, resume, uninstall); the installer puts it in `~/.local/bin`. It shows what is new, restarts only
what changed (the bridge, the Bips' voices when this account runs them, the agents' browsers when their image changed:
rebuilt, their profiles kept), and applies the settings a fresh install now makes (`installer/bipinstall/migrations.py`:
for instance risky commands always asking you). It then offers to update the machine's other installs
too (the family's), each in its own account, after your sudo password, asked once (`--all`: without asking); run
from an administrator's account with no install of its own, it updates that account's Bips' voices and the others.
What no account can do this way comes as commands to run (Hermes' own update).
Local changes in `~/BipAgents` that block it are shown with the commands that put them aside.

The first time, it offers to do this each night at 4:30 (a timer of this account, `install.sh update --auto`: no
questions; `systemctl --user disable --now bipagents-update.timer` stops it). `--check` only says what is new. A Hermes
you had before BipAgents keeps its settings unless you add `--apply-to-my-hermes`.

The agents' browser does not wait for these updates: each night a timer of its own (`bipagents-browser-refresh`) checks
whether Chrome or the image's packages have security fixes and, only then, builds the image again and swaps it in
without interrupting a browser in use (`browser/README.md`, « Staying up to date »). The offer's places get it too.

## Moving to another machine

From a rented server to a computer at home, from one host to a cheaper one, or the other way round: everything comes
along. Your agents' personality, memory, skills, conversations, routines and files, Hermes' settings and your model's
key, the phones' keys and notifications, the vault, shared data, the family's links and the browsers' logins.

1. On the old machine: `bipagents export --moving`. It writes one encrypted file (`~/bipagents-<machine>-<date>.bipagents`),
   shows its passphrase once, then stops the install there, so no routine runs twice. `bipagents resume` starts it again
   if you change your mind.
2. For your phones to need nothing: in Tailscale's admin, rename the old machine (`aibox-old`) and give the new one its
   name. Same name and same ports: the app finds its agents by itself. Otherwise you scan a new QR code at the end.
3. Copy the file to the new machine (`scp`), then run there, in the account that will hold the agents:
   `curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash -s -- import <file>`. It
   asks for the passphrase and installs BipAgents and Hermes in the same versions, for this machine's processor. It
   puts everything back with paths, ports, the tailnet name and the local network adjusted, starts it all and checks
   the agents answer.
4. Once everything works on the new machine, `bipagents uninstall --all` on the old one removes the old copy.

The file goes from one machine to the other as you copy it: nothing passes through anyone else. Whoever has the file
**and** its passphrase can run your agents, so keep them apart. Without `--moving`, `bipagents export` is a backup:
the install keeps running. Left out are Hermes' code and runtimes (installed again), caches, logs and browser caches.

What may need you afterwards:
- **A model on your local network.** One the new machine cannot reach is reported at the end; choose another in the
  app.
- **Sites signing in again.** Some sites ask again from a new address.
- **Losing the sandbox.** Moving to a Mac, or to a Linux without podman, leaves the agents without terminal, file
  access, vault or browser until one is there.

## Updating Hermes

Whenever you want: `bash ~/BipAgents/install.sh hermes-update`. Before changing anything, it says which BipAgents
patches still apply on the new version (checked on a copy). Then it takes our patches off, runs `hermes update`
(its backup, its dependencies), puts back the patches that apply, restarts the agents (the hub and their spaces) and
checks that each one answers. When an agent no longer answers, it offers to go back to the previous version.
`--check` checks without changing anything.

## Uninstall

`python -m bipinstall uninstall` (from `~/BipAgents`, with `PYTHONPATH=installer`) removes the services, the tailnet
publishing and the bridge's config, keeping the agents' data in `~/.hermes`. `--all` also removes Hermes and that data,
after a confirmation. `--dry-run` shows what would be removed.

## Voices: the Bips, and your own voice model

The Bips' voices (Kyutai Pocket TTS, on the CPU, in French, English, Spanish and German) are always installed: they
are light and answer almost instantly. **Your own voice model** can speak for some uses instead: any server already
running on this machine or the local network with OpenAI's `POST /v1/audio/speech` — Fish Audio Drama 3 on audio.cpp
(`tts/drama3/`), Kokoro, Kyutai… Nothing is installed for it: the installer looks for one (audio.cpp's port 8099,
Kokoro's 8880, Kyutai's 8097) and asks what it speaks for — the **Live**, the **spoken replies** to your voice notes,
the **reading** of messages and reports. The Bips speak for the rest, and stand in when it is down.

- `--voice-server http://192.168.1.20:8880` gives its address; `--voice-uses live,replies` what it speaks for.
- `bash ~/BipAgents/install.sh voice` connects one later (or changes it, `--off` to go back to the Bips), in each
  account whose bridge should use it. In the app: each agent › Voice › « Your voice model » (« Voice 1, Voice 2… »),
  and Settings › Voice › « Your voice model » for what it speaks for.

## Several people on one machine

**One install per person, each in their own account.** Each person gets their own bridge, Hermes, agents, memory,
files and browser sessions, separated by the system: one person's agents cannot read the other's. All installs run
at the same time on the same machine, each on its own block of ten ports (8640 for the first, then 9200, 9210…).

What they share: the model, the voices (Pocket, and a voice model already running on the machine or the network, see
above) and the notifications: they go through Apple with
the administrator's key, which never leaves the administrator's install; their bridge relays the others' (one relay
key per install).

### Who runs what, from which account

| | Who | Which account | What |
|---|---|---|---|
| Once | The administrator | Their own (with `sudo`) | `bash ~/BipAgents/install.sh` — their own install, as usual |
| For each person | The administrator | **Their own** (with `sudo`), never the person's | `bash ~/BipAgents/install.sh family add Lea` |
| The person | — | — | Nothing on the server: Tailscale on your tailnet, the app, their QR code |

### `family add`, step by step (Linux)

Run it **in the administrator's terminal**. It shows each step as `[n/5]`:

1. **Their Linux account** (`lea`, or the one given with `--account`): created if it does not exist, given ID ranges
   for rootless containers (the agents' browser) and « linger » (their services keep running while they are logged
   out). Each `sudo` command is shown and confirmed first; nothing is asked when the account is already ready.
   `--dry-run` only shows these commands.
2. **Restart your bridge**, so it knows their relay key: it offers to do it (`y`), or prints the command to run in
   another terminal of **your** account, before step 3 — usually `systemctl --user restart bipagents-bridge`.
3. **Their install, in their account**: no need to log into their account (no `ssh lea@…`). Answer `y` and it runs
   there through `sudo`, its questions showing up **in your terminal** (the model, their first agents, the visible
   browser). They type nothing. It ends with their **QR code**. Answer `N` to get the command and run it later.
4. **Publish their ports on the tailnet**: it offers to run the `sudo tailscale serve` commands (`y`), or prints them.
5. **Their iPhone, on YOUR tailnet**: the one this server is in. A Tailscale account of their own makes a tailnet of
   its own, where the app can't reach the server. So:
   1. invite them in the [Tailscale console](https://login.tailscale.com/admin/users) › Invite users (their email);
   2. on their iPhone, they accept the invite, then in the Tailscale app: profile icon › Add account › sign in with
      the invited email › pick your tailnet;
   3. once the Tailscale app lists the server: the BipAgents app (TestFlight or App Store) › « + » › Scan QR code.

   Simpler, but their iPhone is then one of your devices: sign Tailscale in with your own account. The QR code again:
   `bipagents qr lea`.

The installer speaks English. Agents answer in the language they are written to; the one they write first in
(routines, reports) is set per agent in the app, the iPhone's by default.

On macOS, create their account in System Settings; they paste the printed line in **their own** session. Services
run while the account is logged in (automatic login on a Mac mini used as a server).

## Files

- `install.sh` (repository root): fetches the repository and Python (uv), then runs the installer. The installer's
  own environment (`installer/.venv`, `installer/requirements.txt`) is what its timers and the bridge run it with.
- `bipinstall/`: `detect.py` (the machine), `models.py` (finding and checking the model), `plan.py` (the profile and
  its memory), `ports.py`, `hermes.py` (Hermes through its own commands), `stack.py` (bridge, voices, services),
  `rustbridge.py` (the bridge's binaries), agent kinds in `bridge-rs/assets/agent_templates.yaml` (read by
  `souls.py`, embedded in the bridge).
- Tests: `python -m pytest installer/tests` (no network, nothing installed).
