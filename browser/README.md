🇬🇧 English · [🇫🇷 Français](README.fr.md)

# The agents' visible browser

Each agent can have a browser of its own: a real Google Chrome (Chromium on ARM, where Google ships none for Linux)
on a desktop of its own. You watch it live in the app (« Bureau », behind Face ID) and can take over the mouse and
keyboard: to pass a captcha, check something, or finish a step yourself. It is a browser someone can see and drive,
not a way around websites' rules: an agent that meets a captcha asks you, it never solves one.

## How it fits together

```
 app ──(Face ID)── bridge ──VNC, 127.0.0.1──┐
                                            │   container bipagents-browser-<agent>
 Hermes (the agent) ──MCP over podman exec──┼── stealth-browser-mcp ── Chrome on its own screen
          └── vault fill, DevTools 127.0.0.1┘        (Xvfb, openbox, x11vnc)
```

- **One container per agent**: its Chrome profile (cookies, logins) is a folder of its own on the host, mounted in
  that container only. A page it opens, `file://` included, sees the container, never the host and its keys.
- **Driven by [stealth-browser-mcp](https://github.com/vibheksoni/stealth-browser-mcp)** (MIT), an MCP server built on
  nodriver: the agent gets its tools (`spawn_browser`, `navigate`, `click_element`, `type_text`…) under the name
  `stealth`. The server runs inside the container, next to Chrome; Hermes starts it with `podman exec`.
- **The vault** fills passwords in this same Chrome: Hermes reaches its DevTools port (`browser.cdp_url`) and fills
  the field on the exact site the item was saved for, without the agent ever seeing the password. Chrome's own
  password manager and autofill are turned off (policies in the image): the vault keeps them.
- **The desktop**: the container's VNC server has a password of its own; the bridge logs in and relays the screen to
  the app. The DevTools and VNC ports listen on 127.0.0.1 only, never on the tailnet.

## Set up

The installer offers it on Linux when podman (or docker) is there: it builds the image, then the bridge gives every
agent its container (`[browser] enabled = true` in `bridge.toml`; agents created later from the app get one too). By
hand:

```bash
podman build -t bipagents-browser ~/BipAgents/browser
~/BipAgents/bridge/.venv/bin/python -m bipbridge browser add --agent <agent>   # or --all; `remove` undoes it
systemctl --user restart bipagents-bridge
```

Each agent's container runs as a systemd user unit, `bipagents-browser-<agent>`, only while it is used: it starts at
the agent's first browser call (a few seconds) or when you open its « Bureau », and stops after 10 minutes unused
(nobody watching the desktop, no call, nothing connected to its DevTools). Its profile stays: logins and cookies too.
On a machine with several agents, only the browsers in use take memory. Hermes keeps a small MCP server of ours
connected (`bridge/bipbridge/browser_mcp.py`), which answers with the tools the browser's server listed last time and
starts it when a tool is called. `idle_minutes = 0` in `[browser]` keeps every browser running, as before.

With a GPU (`/dev/dri`), Chrome renders with it; otherwise in software. `[browser]` in `bridge.toml` also takes
`idle_minutes` (10), `language` (e.g. `fr-FR`), `timezone`, `gpu = "none"`, `first_port` (9600) and `runtime` (`podman`
or `docker`).

## What is ours, what is fetched

Nothing of stealth-browser-mcp is kept in this repository. The image build fetches it from its own repository at the
commit BipAgents is tested with (`a7b9eea`, set in the `Containerfile`) and applies our patch:

- `stealth.patch`: Chrome's DevTools on a fixed port (`STEALTH_CDP_PORT`), so Hermes and the vault drive the same
  browser; the container's defaults (profile, flags, time zone) when the agent leaves them out; flags the server adds
  in containers that this one does without (`STEALTH_DROP_ARGS`); a browser always shown on the container's screen,
  even when the agent asks for headless (`STEALTH_HEADFUL`).
- `requirements.txt`: its Python packages at the exact versions tested, installed from PyPI. Among them, **nodriver is
  under the GNU AGPL v3** and certifi under the MPL 2.0: both are installed unmodified, as separate packages.
- `entrypoint.sh` (the screen, the VNC server), `devtools_relay.py` (Chrome's DevTools from the host, at an address
  that outlives Chrome's restarts), `mcp.sh` (the server with only what browsing needs:
  no element cloning, file extraction, network interception, raw CDP or debugging hooks) and the skill telling the
  agent how to use its browser (`bridge/bipbridge/skills/visible-browser/SKILL.md`, copied into its Hermes profile).

## Taking over

« Prendre la main » in the app sets a flag in the container (`/tmp/bipagents-hold`, by the bridge): until « Rendre la
main », the agent's browser tools answer « the person has the browser, wait » (`hold_gate.py`, between Hermes and the
MCP server) and its DevTools connections are closed (`devtools_relay.py`), so it neither clicks nor reads the page while
the person logs in. A flag older than 15 minutes (the app closed without handing it back) no longer counts.

## Limits

- Linux only (the screen is an X server in the container). On a Mac, this will come with a separate macOS account.
- With its page tools (`execute_script`), an agent can read what a page shows, a field the vault filled included.
  The vault keeps passwords out of the agent's conversation and memory, but an agent that wants a password can look
  at the field. Give vault items only to agents you trust with them.
- The DevTools port has no password: on a machine shared by several accounts, another account's programs could
  connect to it on 127.0.0.1 and drive that browser. On a family server, those accounts are the family's.
