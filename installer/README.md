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
   | Large | Linux server ≥ 64 GB | elsewhere | Pocket + Kyutai (GPU, if already running) | permanent, one per person |

   On a 16 GB Mac mini with the model elsewhere: about 1 GB per person plus the voices — a family fits.
4. **Installs Hermes** with its official installer, at the version tested with BipAgents (`517b5e10f`), applies the
   patches of `hermes/patches/` (the agent's questions in the app, photos kept for follow-up questions), as **one gateway per person** (Hermes' multiplex mode): every
   agent is a profile reached at `/p/<agent>/` with its own key. New agents are picked up without a restart.
5. **Creates the first agents**, named by the person, from kinds (Everyday, Wellness, Finances, Home, Work, Learn, Creative, Tech) or a custom « Other » (its role, tone, rules, language and colour, described by the person),
   each with a starting personality (`SOUL.md`) and joined to Bot Mode so they can talk to each other.
6. **Sets up the voices** (Pocket TTS, one server per machine, shared by everyone) and **the bridge** (its config in
   `~/.config/bipagents/bridge.toml`, keys generated, its local-network door), as services that start on their own:
   launchd on macOS, systemd user units on Linux.
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

## Several people on one machine

Each person runs the installer in **their own account**: their own agents, memory, files and browser sessions,
separated by the system. The voices and the model are shared. Each install takes a block of ten ports (8640 for the
first, then 9200, 9210…). The voices too: Pocket, and Kyutai when it runs on the machine (the installer does not
install it; `--kyutai-url` for a Kyutai on another machine).

Notifications go through Apple with the administrator's key, which stays in their install: their bridge relays the
others' (one key per install).

The administrator prepares each person's place, from **their own** account:

```bash
bash ~/BipAgents/install.sh family add Lea
```

On Linux it first gets her account ready (`lea`, or the one given with `--account`), showing each `sudo` command
before running it: the account if it does not exist, its ID ranges for rootless containers (the agents' browser),
and « linger », so her services run while she is logged out. `--dry-run` shows those commands and changes nothing.

Then it creates her relay key, reserves her block of ports, and offers to run her install right away in **her**
account, from your terminal: she types nothing, the questions show up on yours. Otherwise it prints the command for
later. Last, the `tailscale serve` commands to run once her install is done. Restart your bridge so it knows the new
key.

The installer speaks English. Agents answer in the language they are written to; the one they write first in
(routines, reports) is set per agent in the app, the iPhone's by default.

On macOS, create her account in System Settings; she pastes the printed line in **her** session. Services run while
the account is logged in (automatic login on a Mac mini used as a server).

## Files

- `install.sh` (repository root): fetches the repository and Python (uv), then runs the installer.
- `bipinstall/`: `detect.py` (the machine), `models.py` (finding and checking the model), `plan.py` (the profile and
  its memory), `ports.py`, `hermes.py` (Hermes through its own commands), `stack.py` (bridge, voices, services),
  agent kinds in `bridge/bipbridge/agent_templates.yaml` (shared with the bridge).
- Tests: `python -m pytest installer/tests` (no network, nothing installed).
