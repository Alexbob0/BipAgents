# Security

BipAgents gives AI agents a terminal, a browser and a vault of passwords and cards, on your own server or in a
private space of Kerfy Studio's all-included offer. Security reports are very welcome.

## Reporting a vulnerability

Please **don't open a public issue**. Use GitHub's private reporting instead:
[**Report a vulnerability**](https://github.com/Alexbob0/BipAgents/security/advisories/new). Or write to **security@bipagents.com** (also in
[security.txt](https://kerfystudio.com/.well-known/security.txt)). We aim to answer within a week. Once a fix is out, the advisory is published, with your name if you want it.

A problem in Hermes Agent itself goes to [its own security policy](https://github.com/NousResearch/hermes-agent/security).

## What is in scope

- **The bridge** (`bridge-rs/`): its keys, the web console's sessions, the relay to each agent's Hermes, links between
  agents and households, the agents' sandboxes, shared folders and allowed sites, the vault, files and pages served to
  the apps, the space's encryption at rest.
- **The installer** (`installer/`, `install.sh`): file permissions, services, what `tailscale serve` publishes.
- **The agents' visible browsers** (`browser/`): their isolation, the take-over hold, the vault filling passwords.
- **The push relay** (`relay/`): what it forwards, what it keeps out.
- **The apps** (iPhone, iPad, Android; their source is private, reports welcome all the same): keys in the Keychain or
  Keystore, the account's end-to-end encryption (`AccountCrypto`), the local-network door (certificate pinning),
  notifications, links.
- **The all-included offer**: one person's space reached from another's, or by Kerfy Studio's services beyond what the
  [privacy policy](https://kerfystudio.com/en/privacy/) says.
- **Prompt injection that gets past a safeguard BipAgents adds**: an approval skipped, a message between agents that
  were not linked, a password the agent could read.

## How it's meant to work

- Nothing is published on the Internet at home: the bridge and Hermes listen on `127.0.0.1`, and Tailscale makes them
  reachable from your devices only. The local-network door (and, on the offer, the space's door) is HTTPS with a
  pinned self-signed certificate.
- Each agent's Hermes has its own key; the bridge has another. The web console never receives either: its session
  cookie (HttpOnly, Secure, SameSite Strict) plus a header stand for the bridge key, and the bridge adds the agent's
  key itself.
- Risky commands always ask you (`approvals.mode: manual`); « Always allow » isn't offered in the app.
- Agents only write to the agents you linked, through the bridge, and only into their Bot Chat. What they read (pages,
  emails, files, other agents' messages) is information, never instructions, and their personality says so.
- On Linux with podman, each agent's sandbox holds no folder of the machine and reaches no network but the package
  registries and the sites you allowed for that agent.
- The vault fills passwords in the agent's browser without the agent ever seeing them; while you hold the browser,
  the agent can't use it. A payment with a card from the vault always waits for your approval in the app.

## Known limits

- An agent with a terminal, in the same account as others, can read their secrets: agents that hold secrets get a
  sandbox (podman) or an account of their own.
- On a rented server, the provider can read the disk, the vault's key included, unless the space is encrypted at rest
  (off by default for now: its key is split in two, half of it kept by Kerfy Studio's key service at another provider,
  so a copy of the disk doesn't open elsewhere; Kerfy Studio, which keeps that half, could technically access it).
- The model you choose receives every message it answers.
