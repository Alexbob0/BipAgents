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
   | Large | Linux server ≥ 64 GB | elsewhere | Pocket + Kyutai (Nvidia GPU) | permanent, one per person |

   On a 16 GB Mac mini with the model elsewhere: about 1 GB per person plus the voices — a family fits.
4. **Installs Hermes** with its official installer, as **one gateway per person** (Hermes' multiplex mode): every
   agent is a profile reached at `/p/<agent>/` with its own key. New agents are picked up without a restart.
5. **Creates the first agents** from templates (Everyday, Wellness, Budget, Home, Work, Learn, Creative, Tech),
   each with a starting personality (`SOUL.md`) and joined to Bot Mode so they can talk to each other.
6. **Sets up the voices** (Pocket TTS, one server per machine, shared by everyone) and **the bridge** (its config in
   `~/.config/bipagents/bridge.toml`, keys generated, its local-network door), as services that start on their own:
   launchd on macOS, systemd user units on Linux.
7. **Publishes the install on the tailnet** (`tailscale serve`, never on the Internet). Without the right to do it
   (another person's account on Linux), it prints the commands for the administrator.
8. **Shows one QR code**: scanned in the app, it adds every agent of the install.

## Several people on one machine

Each person runs the installer in **their own account**: their own agents, memory, files and browser sessions,
separated by the system. The voices and the model are shared. Each install takes a block of ten ports (8640 for the
first, then 9200, 9210…).

Notifications go through Apple with the administrator's key, which stays in their install: their bridge relays the
others' (`[relay]` in their `bridge.toml`, one key per install), and the others install with
`--relay-url https://<server>:8643 --relay-key <their key>`.

On Linux, for the services of an account to run while that person is logged out, the administrator enables it once:
`sudo loginctl enable-linger <user>`. On macOS, services run while the account is logged in (automatic login on a
Mac mini used as a server).

## Files

- `install.sh` (repository root): fetches the repository and Python (uv), then runs the installer.
- `bipinstall/`: `detect.py` (the machine), `models.py` (finding and checking the model), `plan.py` (the profile and
  its memory), `ports.py`, `hermes.py` (Hermes through its own commands), `stack.py` (bridge, voices, services),
  `templates.yaml` (agent templates).
- Tests: `python -m pytest installer/tests` (no network, nothing installed).
