# Security

BipAgents gives AI agents a terminal, a browser and a vault of passwords, on your own server. Security reports are
very welcome.

## Reporting a vulnerability

Please **don't open a public issue**. Use GitHub's private reporting instead:
[**Report a vulnerability**](https://github.com/Alexbob0/BipAgents/security/advisories/new). We aim to answer
within a week. Once a fix is out, the advisory is published, with your name if you want it.

A problem in Hermes Agent itself goes to [its own security policy](https://github.com/NousResearch/hermes-agent/security).

## What is in scope

- **The bridge** (`bridge/`): its keys, the web console's sessions, the relay to each agent's Hermes, links between
  agents and households, shared folders, the vault, files served to the app.
- **The installer** (`installer/`, `install.sh`): file permissions, services, what `tailscale serve` publishes.
- **The agents' visible browsers** (`browser/`): their isolation, the take-over hold, the vault filling passwords.
- **The iPhone app**: keys in the Keychain, the local-network door (certificate pinning), notifications, links.
- **Prompt injection that gets past a safeguard BipAgents adds**: an approval skipped, a message between agents that
  were not linked, a password the agent could read.

## How it's meant to work

- Nothing is published on the Internet: the bridge and Hermes listen on `127.0.0.1`, and Tailscale makes them
  reachable from your devices only. The local-network door is HTTPS with a pinned self-signed certificate.
- Each agent's Hermes has its own key; the bridge has another. The web console never receives either: its session
  cookie (HttpOnly, Secure, SameSite Strict) plus a header stand for the bridge key, and the bridge adds the agent's
  key itself.
- Risky commands always ask you (`approvals.mode: manual`); « Always allow » isn't offered in the app.
- Agents only write to the agents you linked, through the bridge, and only into their Bot Chat. What they read (pages,
  emails, files, other agents' messages) is information, never instructions, and their personality says so.
- The vault fills passwords in the agent's browser without the agent ever seeing them; while you hold the browser,
  the agent can't use it.

## Known limits

- An agent with a terminal, in the same account as others, can read their secrets: agents that hold secrets get a
  sandbox (podman) or an account of their own.
- On a rented server, the provider can read the disk, the vault's key included.
- The model you choose receives every message it answers.
