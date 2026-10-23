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
| **Linux server** with a GPU | | Everything above, plus the bonus below: your own voice model. |

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

The bridge is a single program, written in Rust (`bridge-rs/`). Each version of the repository has its binaries in
the GitHub release `bridge-<key>`, where the key is the first 12 characters of the SHA-256 of the `bridge-rs/` tree:

```bash
cd ~/BipAgents
key=$(printf '%s' "$(git rev-parse HEAD:bridge-rs)" | sha256sum | cut -c1-12)    # macOS: shasum -a 256
asset=bipbridge-linux-x86_64.tar.gz    # bipbridge-linux-aarch64.tar.gz (ARM), bipbridge-macos-universal.tar.gz (Mac)
mkdir -p ~/.local/bin
curl -fsSL "https://github.com/Alexbob0/BipAgents/releases/download/bridge-$key/$asset" | tar -xz -C ~/.local/bin bipbridge bip-browser-mcp
# Or build it (stable Rust): cd bridge-rs && cargo build --release && cp target/release/{bipbridge,bip-browser-mcp} ~/.local/bin/
mkdir -p ~/.config/hermes-ios && chmod 700 ~/.config/hermes-ios
cp bridge/bridge.example.toml ~/.config/hermes-ios/bridge.toml && chmod 600 ~/.config/hermes-ios/bridge.toml
~/.local/bin/bipbridge genkey      # → bridge_key (also enter it in the app)
```

The release's `SHA256SUMS` lists each archive's checksum.

In `~/.config/hermes-ios/bridge.toml`, for a machine without a GPU:

```toml
[kyutai]
enabled = false                 # Kyutai 1.6B, if you run it, is a [voice_server] like any other (see the end)

[pocket]
url = "http://127.0.0.1:8098"
default_voice = "pocket:colibri"

[agents.wellness]               # one section per agent
display_name = "Wellness"
hermes_url = "http://127.0.0.1:8642"
hermes_key = "<API_SERVER_KEY of this agent>"
upload_dir_host = "~/hermes/profiles/wellness/uploads"        # where the bridge drops uploaded files
upload_dir_container = "/home/hermes/.hermes/profiles/wellness/uploads"   # the same folder as seen by the agent
```

The `[ntfy]` sections and `ntfy_topic` / `ntfy_token` are optional (legacy proactive messages). Hermes' scheduled
tasks reach the Inbox without them (`[cron]`). Then run `~/.local/bin/bipbridge check --config ~/.config/hermes-ios/bridge.toml`.

All options: [`bridge/README.md`](../bridge/README.md).

### The agents' sandboxes (Linux with podman)

When the agents' tools run in Hermes' sandbox (`terminal.backend: docker`, with rootless podman), the bridge keeps
that sandbox isolated, so Hermes runs harmless work there (`python -c`, `execute_code`, `pip install`) without asking
you each time:

- **no folder of this machine inside**: the files you send an agent are copied into a podman volume of its own
  (`bipagents-files-<agent>`, read-only at `/fichiers`); a folder an agent shares is a podman volume too
  (`bipagents-share-<agent>`), at `/partage` in its sandbox and read-only at `/lecture/<agent>` in the sandboxes of
  the agents you let read it. The bridge keeps `<data>/shares/<agent>/` in step with it (backups and exports take
  that folder); the first time, that folder's content is copied into the volume and the folder kept as
  `<agent>.before-volume`. The `shared_files_*` tools reach the same folders (you approve deletes from the app);
- **no internet but the package registries**: the sandbox sits on `bipagents-sandbox`, a podman network without a
  route out, and reaches PyPI, npm, Rust crates, Go and Debian only, through a small proxy container
  (`bipagents-egress`, `podman logs bipagents-egress` shows what it let through or blocked). Web search and the
  agent's own visible browser are separate tools and keep working. Each agent can also have **sites of its own**
  (HTTPS only): each sandbox gets its own proxy credentials, so the proxy knows which agent asks. When the proxy
  refuses a site to an agent's programs, or when the agent asks first (`sandbox_sites_request`), you get one card in
  the app, « Autoriser toujours / Refuser »; the list is in the agent's options, « Sites autorisés », where you can
  remove a site or add one by hand (`*.example.org` for its subdomains). A name of your own network (`localhost`,
  `.local`, a private or Tailscale address) is never let through.

Existing agents switch at the bridge's next start (`bipbridge sandbox` does it now and prints what each one got). The
`[sandbox]` section of `bridge.example.toml` lists the settings. On macOS, or with docker instead of podman, the
sandboxes stay as they were (folders mounted, Hermes asks before risky commands). To step back on Linux, set
`[sandbox] isolate = "off"` and restart the bridge: each sandbox gets its folders mounted again (what the agents wrote
in the volumes copied back first), and the network and proxy settings are taken out; the volumes stay.

### The Carnet (the agents' notebook)

The bridge keeps the person's Carnet under its data dir (`notebook/carnet/`, plain Markdown that opens in Obsidian;
`notebook/private/` for health; `notebook/history/` for every change). Its nightly sorting calls a small model once a
night at most, only when the day's conversations hold something the agents did not file, under a monthly cap:
`[notebook] model` (default: the agents' own model, at their provider), `base_url` and `key` for another
OpenAI-compatible API, `price_in`/`price_out` (€ per million tokens), `monthly_cap` (€, default 0.10) and `hour`
(default 3). Each run logs `notebook pass` with its tokens and cost; `GET /v1/notebook/runs` shows the month.

Long work goes to background tasks, never inline in a conversation. An agent starts one by itself only when the person
asked for it, within `[jobs] direct_budget_eur` (€, default 0.20, counted at the `[notebook]` prices); above that it
proposes the task and the person starts it with one tap. A conversation's run that goes past `handoff_tool_calls`
(default 15) tool calls or `handoff_seconds` (default 180) is told once to hand the rest over (`handoff = false` turns
this off); the bridge logs `long run: handing over to a background task (steer)`.

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
cp ~/BipAgents/bridge/systemd/hermes-bridge.service ~/.config/systemd/user/   # runs ~/.local/bin/bipbridge
systemctl --user daemon-reload && systemctl --user enable --now pocket-tts hermes-bridge
sudo loginctl enable-linger $USER     # services keep running without an open session
```

Updating: `git pull`, `pip install -r requirements.txt` in Pocket's venv, the bridge binary of the new key (same
commands as above, when `bridge-rs/` changed), restart both services. If you added the agents' browser by hand
(`browser/README.md`), `bipagents browser-refresh` keeps its Chrome up to date: run it from a nightly timer
(`installer/bipinstall/browser_refresh.py` writes one: « Staying up to date » there).

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
- In the app: Settings › Add an agent → **Scan the QR code**. On the server, fill in `public_url` (the bridge's
  and each agent's tailnet address) in `bridge.toml`, then show the code with `bipagents qr` (one per
  agent, `--agent vie` for one only, `--png vie.png` to save it). It holds the agent's keys: show it only to your
  own phone. Without it: address `https://mamachine.tailnet.ts.net:8642`, api_server key, bridge address
  `https://mamachine.tailnet.ts.net:8643` and its key. Pick your language and your Bip.

**Without Tailscale (optional)**: with `[lan] enabled = true`, the bridge also opens an HTTPS door on your local
network (port 8650, open it on the LAN only) and the QR code carries its address and certificate fingerprint. When
the tailnet does not answer, the app goes through that door, accepting only that certificate; it shows « Online ·
local network ». It learns a new local address on its own while the tailnet works; if the address changed while the
tailnet was down, it asks you to scan a new QR code. Notifications still need the Internet (Apple).

**Notifications** (replies on the lock screen, approvals, Inbox): they go through Apple and require a paid
Apple Developer account. Create an APNs key (.p8) and fill in `[apns]`, see [`bridge/README.md`](../bridge/README.md#apns).
Without it, everything else works, just without notifications.

## Bonus: your own voice model (GPU)

With a GPU, a more expressive voice server can speak instead of the Bips for some uses: the Live, the spoken replies,
the reading of messages. Any server with OpenAI's `POST /v1/audio/speech` will do: Fish Audio Drama 3 on audio.cpp
(recipe in `tts/drama3/`), Kokoro, Kyutai TTS 1.6B… Run it, then in the bridge (or `install.sh voice`, which finds it
on the network):

```toml
[voice_server]
url = "http://127.0.0.1:8880"
live = true
replies = true
reading = true
```

Its voices show up in the app as "Voice 1, Voice 2…" (agent › Voice). When it doesn't answer, the Bips take over.
Nothing else depends on it: the Bips, the four languages, the Inbox and notifications work without it.
