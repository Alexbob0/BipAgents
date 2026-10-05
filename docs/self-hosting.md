🇬🇧 English · [🇫🇷 Français](self-hosting.fr.md)

# Self-hosting BipAgents: Mac mini, VPS or Linux server

BipAgents needs **no GPU**: the Bips' voices run on CPU (Kyutai Pocket TTS). Any machine that already runs
[Hermes Agent](https://github.com/NousResearch/hermes-agent) is enough, whether it is a Mac mini, a VPS
or a Linux server at home.

```
iPhone (BipAgents) ──Tailscale──▶ your machine
                                   ├─ Hermes Agent, api_server   :8642 (one port per agent: 8644, …)
                                   ├─ bridge (bridge/)           :8643  voice, files, push, Inbox
                                   └─ Pocket TTS (pocket/)       :8098  Bips' voices (local, called by the bridge)
```

Nothing is exposed to the Internet: everything goes through your tailnet, over HTTPS with a real certificate (`tailscale serve`).

## Which machine?

| Machine | Requirements | Notes |
|---|---|---|
| **Mac mini** Apple Silicon | 16 GB recommended | Measured on M4: first audio ≈ 50 ms, ≈ 6.5× real time, ≈ 0.6 GB for the 4 languages. Disable sleep and enable automatic login (the services are LaunchAgents). |
| **VPS** Linux | 4 vCPU (2 of them for Pocket), 8 GB | **Dedicated** vCPUs preferably (shared vCPUs slow the voice down). ARM (Ampere, Hetzner CAX) or x86 with `--quantize`. Hermes calls its language model through an API, it does not need a GPU. Not measured: check the first audio latency after installation. |
| **Linux server** with an Nvidia GPU | | Everything above, plus the Kyutai 1.6B bonus (human voice, podcast) below. |

## 1. Tailscale

Install Tailscale on the machine and on the iPhone, in the same tailnet. In the admin console:
*DNS* → enable **MagicDNS** and **HTTPS Certificates**. The machine then gets a name like `mamachine.tailnet.ts.net`.

On a Mac, the `tailscale` command comes with the app (*Tailscale Settings* → install the command-line
interface), or via `brew install tailscale`.

## 2. Hermes: one api_server per agent

Each agent (Hermes profile) exposes its api_server on a local port. In the profile's `.env`:

```bash
API_SERVER_ENABLED=true
API_SERVER_HOST=127.0.0.1        # 0.0.0.0 if Hermes runs in a container, published on 127.0.0.1
API_SERVER_PORT=8642             # 8644 for the second agent, etc.
API_SERVER_KEY=<random key of at least 32 characters, one per agent>
```

Check: `curl -H "Authorization: Bearer $KEY" http://127.0.0.1:8642/v1/capabilities` must advertise runs
(`run_submission`, `run_events_sse`, `run_approval`). The agent's questions (`clarify` tool) require a version
of Hermes that wires them into the api_server, see [`hermes-clarify-api.md`](hermes-clarify-api.md).

## 3. The repository and Pocket TTS (Bips' voices)

```bash
git clone https://github.com/Alexbob0/BipAgents ~/BipAgents
cd ~/BipAgents/pocket
python3 -m venv .venv && .venv/bin/pip install -r requirements.txt    # Python ≥ 3.10 (VPS: see below)
.venv/bin/python pocket_server.py --voices ../voices --languages fr,en,es,de
curl -s http://127.0.0.1:8098/health
```

The first launch downloads the models (≈ 440 MB per language). Loading only the languages you need speeds up
startup (`--languages fr`). Details: [`pocket/README.md`](../pocket/README.md).

### On a VPS

- **OS**: Ubuntu 24.04 or Debian 12 work fine (`sudo apt install python3-venv python3-pip git`).
- **PyTorch without CUDA**: on x86 Linux, `pip` installs PyTorch with CUDA by default (several useless GB without a
  GPU). Install the CPU build first, then the rest:

  ```bash
  .venv/bin/pip install torch --index-url https://download.pytorch.org/whl/cpu
  .venv/bin/pip install -r requirements.txt
  ```

  (On ARM, Hetzner CAX or Ampere, the default build already comes without CUDA.)
- **Sharing the CPU with Hermes**: `--threads 2` reserves 2 cores for Pocket (same speed measured as with all
  cores on Apple M4). On x86, **add `--quantize`** (int8 weights): measured on an x86 server, first audio drops from ≈ 140 ms to ≈ 55 ms;
  `pip install "pocket-tts[quantize]"` adds torchao to optimize it.
- **Memory and disk**: ≈ 0.6 to 1 GB of RAM for the 4 languages, ≈ 2 GB of disk for the models (in
  `~/.cache/huggingface`, `HF_HOME` to put them elsewhere) and ≈ 1 GB for CPU PyTorch. Add some swap
  on a 4 GB VPS.
- **Network**: keep Pocket on `127.0.0.1` (the default) and open nothing in the firewall: the bridge is on
  the same machine, the iPhone goes through Tailscale. With `ufw`: `sudo ufw allow in on tailscale0`, nothing else.
- **Measure** once installed: `python3 pocket/bench.py` (first audio and speed). Target: first audio under
  ≈ 300 ms, at least 1.5× real time. Otherwise: `--quantize` (x86), then dedicated vCPUs.

## 4. The bridge

```bash
cd ~/BipAgents/bridge
python3 -m venv .venv && .venv/bin/pip install -r requirements.txt    # Python ≥ 3.11 recommended
mkdir -p ~/.config/hermes-ios && chmod 700 ~/.config/hermes-ios
cp bridge.example.toml ~/.config/hermes-ios/bridge.toml && chmod 600 ~/.config/hermes-ios/bridge.toml
.venv/bin/python -m bipbridge genkey      # → bridge_key (also enter it in the app)
```

In `~/.config/hermes-ios/bridge.toml`, for a machine without a GPU:

```toml
[kyutai]
enabled = false                 # no Kyutai 1.6B: the Bips do everything

[pocket]
url = "http://127.0.0.1:8098"
default_voice = "pocket:loutre"

[agents.wellness]               # one section per agent
display_name = "Wellness"
hermes_url = "http://127.0.0.1:8642"
hermes_key = "<API_SERVER_KEY of this agent>"
upload_dir_host = "~/hermes/profiles/wellness/uploads"        # where the bridge drops uploaded files
upload_dir_container = "/home/hermes/.hermes/profiles/wellness/uploads"   # the same folder as seen by the agent
```

The `[ntfy]` sections and `ntfy_topic` / `ntfy_token` are optional (legacy proactive messages). Hermes' scheduled
tasks reach the Inbox without them (`[cron]`). Then run `.venv/bin/python -m bipbridge check`.

All options: [`bridge/README.md`](../bridge/README.md).

## 5. Start at boot

**macOS (Mac mini)**: two LaunchAgents.

```bash
cd ~/BipAgents
for f in pocket/launchd/io.github.bipagents.pocket.plist bridge/launchd/io.github.bipagents.bridge.plist; do
  sed "s#__HOME__#$HOME#g" "$f" > ~/Library/LaunchAgents/$(basename "$f")
  launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/$(basename "$f")
done
tail -f ~/Library/Logs/bipagents-*.log
```

To stop them: `launchctl bootout gui/$(id -u)/io.github.bipagents.pocket` (same for `.bridge`).

**Linux (VPS, server)**: two systemd user services.

```bash
mkdir -p ~/.config/systemd/user
cp ~/BipAgents/pocket/systemd/pocket-tts.service ~/.config/systemd/user/
cp ~/BipAgents/bridge/systemd/hermes-bridge.service ~/.config/systemd/user/   # adjust WorkingDirectory/ExecStart to ~/BipAgents/bridge
systemctl --user daemon-reload && systemctl --user enable --now pocket-tts hermes-bridge
sudo loginctl enable-linger $USER     # services keep running without an open session
```

Updating: `git pull`, `pip install -r requirements.txt` in each venv, restart both services.

## 6. Open on the tailnet

```bash
sudo tailscale serve --bg --https=8642 http://127.0.0.1:8642   # one per agent (8644, …)
sudo tailscale serve --bg --https=8643 http://127.0.0.1:8643   # the bridge
```

From the iPhone or another device on the tailnet: `https://mamachine.tailnet.ts.net:8643/health`. Pocket (8098)
stays local, only the bridge calls it.

## 7. The app

- Build with Xcode onto an iPhone: `echo 'DEVELOPMENT_TEAM = <your Team ID>' > Config/Local.xcconfig`, then run
  the BipAgents scheme.
- Under your own Apple account, pick your app identifier (it must be unique at Apple) and add it to
  `Config/Local.xcconfig`:

  ```
  DEVELOPMENT_TEAM = <your Team ID>
  BIP_BUNDLE_ID = com.tonnom.bipagents
  ```

  The extension (`….NotificationService`), the App Group (`group.…`) and the keychain group (`….shared`) derive from it;
  Xcode creates them on the first build (*Automatically manage signing*). Use the same identifier in the bridge's
  `[apns] bundle_id`.
- In the app: Settings › Add an agent → address `https://mamachine.tailnet.ts.net:8642`, api_server key, bridge
  address `https://mamachine.tailnet.ts.net:8643` and its key. Pick your language and your Bip.

**Notifications** (replies on the lock screen, approvals, Inbox): they go through Apple and require a paid
Apple Developer account. Create an APNs key (.p8) and fill in `[apns]`, see [`bridge/README.md`](../bridge/README.md#apns).
Without it, everything else works, just without notifications.

## Bonus: the human voice and the podcast (Kyutai 1.6B, GPU)

On a machine with an Nvidia GPU, the [Kyutai TTS 1.6B](https://github.com/kyutai-labs/delayed-streams-modeling) server
adds a calm human voice ("Classic voice" in the app, in French) and powers the morning podcast (a separate Hermes
skill, outside this repository). Run it on `:8097`, then in the bridge:

```toml
[kyutai]
enabled = true
url = "http://127.0.0.1:8097"
default_voice = "5476"
```

In French, if Pocket goes down, the bridge then falls back to this voice. Nothing else depends on it: the
Bips, the four languages, the Inbox and notifications work without it.
