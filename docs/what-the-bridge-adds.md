🇬🇧 English · [🇫🇷 Français](what-the-bridge-adds.fr.md) · [🇩🇪 Deutsch](what-the-bridge-adds.de.md) · [🇪🇸 Español](what-the-bridge-adds.es.md) · [🇳🇱 Nederlands](what-the-bridge-adds.nl.md)

# What the bridge adds to Hermes Agent

BipAgents is **built on [Hermes Agent](https://github.com/NousResearch/hermes-agent)** by Nous Research. Hermes does
the thinking: the model loop, tools, memory, skills, routines, approvals, its gateways and its Runs API. Hermes is
made for a terminal and chat platforms, and every Hermes client (the CLI, the official desktop app, the Telegram
gateway, other apps) shows what Hermes does. BipAgents wants something else: **several agents per person, used from a
phone by people who never open a terminal, safe enough to hold their mail, calendar and cards, and cheap enough to
sell as a monthly plan.** That takes a layer between the apps and Hermes: the **bridge** ([`bridge-rs/`](../bridge-rs/),
one Rust program, `bipbridge`). This page lists what it adds, area by area, with the modules (`bridge-rs/src/<name>.rs`)
that do it, and what it deliberately leaves to Hermes.

## At a glance

| Area | Hermes alone | Hermes + other clients | BipAgents bridge |
|---|---|---|---|
| Talking to it | terminal, chat gateways, Runs API | a chat window (desktop app, Telegram…) | iPhone, iPad and Android apps, voice notes, hands-free Live calls, a web console |
| Voices | none | none, or the platform's own | a voice per agent, sentence by sentence while the reply streams, text prepared for speech in 5 languages |
| Away from the app | the gateway's own messages | the platform's notifications | push with the text fetched from your server, approvals and questions answered from the lock screen, Live Activities |
| Several agents | profiles, configured by hand | usually one agent at a time | agents created in 2 s from the app, each with its own sandbox, browser, tools, voice and memory language |
| Browsing | a browser tool | same | each agent's own Chrome, watched live and taken over from the phone |
| Safety | approvals, `approvals.deny`, docker sandbox | same | approvals always asked and worded plainly, sandboxes without your folders, sending to unknown sites capped, peers limited, write-only vault, loop brake |
| Memory | memory files, skills | same | a readable Notebook, memory editor, common memory, group discussions, family agents |
| Your data | what tools reach | same | mailboxes, calendars, contacts, reminders, health, notes and WhatsApp imports, connectors with OAuth |
| Results | text and files | text and files | locked offline pages, maps, podcasts, files handed over wherever the agent wrote them |
| Work on its own | cron jobs | same | background tasks with a budget, watchers, every routine's reply in the app, one list of what runs by itself |
| Cost | whatever the model costs | same | token diet, provider pinning for the cache, per-call usage, credit, budgets, sleep |
| Install and keep | install, then config by hand | same | one command, updates, backups, restores, moves between servers |
| Hosted offer | — | — | a place per person on Kerfy Studio's servers: its own certificate, a door on port 443, credit and backups |

## Phone first

- **Voice.** `voice_ws.rs` follows a Hermes run and streams one synthesized sentence at a time; `textproc.rs` splits
  the streamed markdown into sentences; `tts.rs` schedules the engines (Pocket TTS on CPU, or the person's own voice
  server); `voices.rs` gives each agent its voice. Text is prepared for speech: `numbers_fr.rs`, `speech_intl.rs`
  (English, Spanish, German, Dutch), `durations.rs`, `pronounce.rs`, `pronunciations.rs`.
- **Live calls.** `live_brief.rs` starts a call from a short brief of the conversation instead of 100k tokens of
  history; `live_calls.rs` writes what the call did back into the conversation.
- **Notifications.** `push.rs` (APNs, `apns.rs`; Android, `fcm.rs`) sends a generic alert and the phone fetches the
  text from the bridge, so Apple and Google never see it; `relay.rs` and the separate [`relay/`](../relay/) forward
  pushes without content. `runs.rs` turns an approval nobody is watching into a notification; `agent_notify.rs` sets
  this per agent; `live_activities.rs` shows long work on the lock screen and Dynamic Island.
- **Messages during the work.** `msg_queue.rs`: a message sent while the agent works goes into the turn going on
  (Hermes' steer, after its current tool calls); « After » keeps it in the agent's queue instead, on the bridge, the same on
  every device, started as the next turn once the work ends (reorder, cancel, send now, pause), one notification at the end.
- **Nothing missed.** `ntfy.rs` and `cronwatch.rs` put agent messages and every routine's reply into an outbox
  (`store.rs`) with its audio ready; `restore_reconcile.rs` puts back the messages phones kept after a restore.
- **Reaching home.** Over Tailscale, or `lan.rs`: a pinned HTTPS door on the local network when the Internet is down.
  `device_keys.rs` gives each device its own key; `guard.rs` counts wrong keys; `console.rs` is the web console.
- **Plain words.** `approval_text.rs`, `approval_command.rs`, `no_reply.rs`, `names.rs`, `answer_style.rs` and
  `welcome.rs` turn Hermes' technical events into short sentences in the person's language.

## Several agents per person

- `agents_admin.rs` creates, edits and removes agents from the app; `agent_pool.rs` keeps spare profiles so a new
  agent is ready in about 2 seconds; `souls.rs` gives each kind of agent its starting personality; `guide.rs` and
  `guide_team.rs` propose the first agents or a whole team in one card; `agent_groups.rs` sorts them (Perso, Pro…).
- `spaces.rs` moves an agent into its own Hermes when it must hold secrets; `own_space.rs` does it by itself at the
  first secret. `sandbox.rs` gives each agent an isolated sandbox (no host folder, only package registries).
- `browsers.rs` runs each agent's real Chrome in its own container; `browser_mcp.rs` starts its tools when used;
  `desktop.rs`, `desktop_page.rs` and `desktop_read.rs` show it in the app and let the person take over.
- Between agents: `peers.rs` (only along links the person turned on), `agent_messages.rs`, `agent_pairs.rs` (a
  separate « A ⇄ B » conversation), `agent_files.rs` (files, which Hermes' `message_agent` cannot carry),
  `shares.rs` (read-only data).

## Safety

- **Approvals.** The bridge keeps Hermes' approvals in manual mode (`sandbox.rs`), words them plainly, keeps the
  answered ones in the conversation (`approvals_log.rs`) and follows Hermes 0.22.5's prompt protocol
  (`hermes_prompts.rs`).
- **Prompt injection.** Every agent's personality says that what it reads (pages, mail, files, other agents) is
  information, never orders (`souls.rs`). Peers only reach a « Bot Chat » (`peers.rs`). Guest turns get closed tools
  (patch 0018).
- **What leaves.** `sandbox_sites.rs` and `send_guard.rs`: reading the web is open, but sending data to sites the
  person did not validate is capped (64 KB per site per hour) and then asked; `heavy_work.rs` makes agents ask before
  big installs or builds on shared servers; `body_limit.rs` caps requests; `beneath.rs` keeps sandbox file access
  under its folder.
- **Vault.** `vault.rs` keeps credentials encrypted where no agent can read them; Hermes fills logins without the
  model seeing the password. `vault_logins.rs` creates accounts with passwords the agent never sees;
  `vault_contacts.rs` stores addresses; `secrets.rs` holds a connector's secrets.
- **Payments.** `cards.rs`, `card_pay.rs`, `card_fill.rs`: capped virtual cards, every payment approved;
  `handover.rs`: « the agent prepares, you pay »; `link_wallet.rs`: Stripe's Link wallet in the US and Canada.
- **Brakes.** `loop_brake.rs` stops a turn going round in circles (a model once made 267 calls in one turn);
  `agent_restart.rs` restarts an agent's Hermes only when there is a reason.

## Memory, Notebook, rooms and family

- `notebook/` is the « Carnet »: people, subjects, tasks, records and trips the person told their agents, organised
  and readable; `archives/` indexes big piles of documents; `message_search.rs` searches every conversation.
- `memory_edit.rs` shows and edits Hermes' memory in the app; `shared_memory.rs` adds one common « what my agents
  know about me »; `memory_language.rs` keeps memory in the agent's language; `own_skills.rs` lists skills the agent
  wrote itself.
- `rooms.rs`, `rooms_handoff.rs`, `rooms_memory.rs`, `rooms_remote.rs`: group discussions between agents, a
  relative's agent included.
- `households.rs` (with `door_tls.rs`) links two installs on purpose; `family_agent.rs`, `family_news.rs`,
  `agent_shares.rs`, `meet.rs`: an agent common to the family, news passed on only after approval, shared agents,
  two agents finding a date. `read_shares.rs`, `project_agent.rs`, `friends_cards.rs` and `templates.rs` (with
  `templates_scrub.rs`) share a conversation read-only, an occasion's agent or an agent as a template.

## Connectors and your data

- Mail: `mailbox.rs` (any IMAP box), `mail_scan.rs`, `mail_tidy.rs`; an agent's own address: `agent_email.rs`,
  `agent_email_smtp.rs`.
- Phone data, shared only with chosen agents: `calendar.rs`, `caldav.rs`, `contacts.rs`, `reminders.rs`, `health.rs`,
  `phone_asks.rs`, `phone_rights.rs`.
- Imports: `notes_import.rs`, `whatsapp_import/`, `assistant_import.rs` (an agent brought from another assistant).
- Services: `services.rs` (MCP servers from the app's catalog), `connections.rs` (OAuth 2.1 sign-in, no key to
  paste), `connectors.rs` and `connectors_run.rs` (connectors an agent writes, run in its sandbox), `watchers.rs`.
- Tools served over MCP to every agent (`agent_tools.rs`, `mcp_sessions.rs`): audio and podcasts, `weather.rs`,
  `routes.rs`, maps.

## Files, pages and maps

- `files.rs` stores what the person sends (share sheet included) where the agent's sandbox can read it;
  `files_kept.rs` sets how long. `media.rs`, `sent_files.rs`, `give_file.rs`, `made_files.rs` and `cited_files.rs`
  make sure a file the agent made or mentions actually reaches the person.
- `pages.rs`: dashboards, trackers and small tools the agent writes as one HTML file, locked by a
  Content-Security-Policy (no network, no forms) before they are shown.
- `maps.rs`, `map_tiles.rs`, `map_route.rs`: OpenStreetMap maps through the bridge, interactive maps, real street
  routes; `favicons.rs` fetches site icons so the phone doesn't.

## Work on its own

- `jobs.rs`: background tasks with a budget chosen first, the tokens shown after, pause and resume; `handback.rs`
  brings the report back to the conversation it came from.
- `cronwatch.rs` delivers every routine's reply to the app; `autonomous.rs` lists everything agents do without
  being asked; `storage_watch.rs` makes agents ask what to keep; `feature_tips.rs` shows features over time.

## Cost control

- `tool_diet.rs` puts rarely used tools behind Hermes' tool search per kind of agent (measured: about 19k input
  tokens per call, 11.8k of them tool schemas).
- `model_route.rs` pins one provider at EUrouter so its prompt cache works (cached input at about 17.5 % of the
  price); `model_choice.rs` lets self-hosters pick a model per agent.
- Patch 0020 reports each model call's usage, which `jobs.rs` and `runs.rs` use to stop a task before it passes its
  budget. `credit.rs` pauses what runs by itself when the month's credit is used, never the conversations.
- `sleep.rs` stops Hermes on a place nobody used for days and wakes it on the next request.

## Install, backups and moves

- The installer ([`installer/`](../installer/)) sets up Hermes, the bridge, voices and sandboxes in one command and
  updates them. `place_backup.rs` exposes backups and their key; `place_import.rs` brings your own Hermes in;
  `place_move.rs` moves a place to another server; `ssh_keys.rs` lets the person log in to their own account.
- `space.rs`: a place's space encrypted at rest (off by default).

## The all-included offer

On Kerfy Studio's servers each person gets a **place**: their own account, Hermes and bridge. `cloud_setup.rs` sets a
new place up from a ready server; `place_cert.rs` gets it a real Let's Encrypt certificate under
`<label>.p.bipagents.com`; `proxy_header.rs` lets every place answer on port 443 behind an SNI router (many networks
block other ports); `credit.rs` and `beta_measures.rs` talk to the guichet, Kerfy Studio's account and billing
service (a private repository, not part of this one).

## The Hermes patches we carry

A few changes the apps need are not in Hermes yet. They live in [`hermes/patches/`](../hermes/patches/README.md) on
top of the pinned stable release (`BASE`: v0.22.5), and the installer applies them.

| Patch | Why |
|---|---|
| 0005 vault fills after the browser restarted | logins keep working after the agent's browser was closed |
| 0006 synchronous peer reply | an agent asked by another one answers in the asker's conversation |
| 0007 `browser.vault_only` | agents with their own visible browser keep the vault without Hermes' browser |
| 0008 time zone, routines, wall clock | routines run at the person's home time, once per wall-clock time |
| 0009 tool search `!name`, compact skill categories | the token diet: some tools kept direct, rare skills by name only |
| 0010 empty replies | tool calls a model writes as text become real calls; retries change the request |
| 0011 titles with GLM | GLM answers thinking-off calls with nothing, so titles ask for « low » thinking |
| 0013 MCP results: vault passwords scrubbed | a password never comes back to the model through a tool result |
| 0014 routine results in plain UTF-8 | accents stay readable in routine tool results |
| 0015 time anchor | the current time travels with each message |
| 0016 one `message_agent`, the bridge's | every agent-to-agent message goes through the bridge's checks |
| 0017 address items | vault addresses carry a name, a phone and an e-mail |
| 0018 guest turns | a relative's turn gets only the guest's tools and no USER.md |
| 0019 routines report to their conversation | a routine made in a conversation answers there |
| 0020 per-call usage in the Runs API | real cost per model call, for budgets |

**How we upstream them.** The series is kept as a git branch on upstream Hermes and exported with
`scripts/hermes-patches.sh` (`branch`, `export`, `check`): one commit per patch, stable names, the same bytes every
time. A move to a newer Hermes is a rebase of that branch, then a new `BASE`. Patches are dropped as soon as Hermes has
the same thing: 0001 (clarify in the Runs API) and 0004 (images kept for follow-up questions) went when v0.22.5
shipped its own. 0005 is proposed upstream as NousResearch/hermes-agent#136660; small, general ones (0008's fix,
0009, 0011, 0013, 0014, 0017, 0019's labels) are the next candidates. Details per patch:
[`hermes/patches/README.md`](../hermes/patches/README.md).

## What we deliberately leave to Hermes

The model loop and its tools, memory files and skills, the cron scheduler, the approval engine, the vault store and
its form filling, Bot Mode, profiles, the docker terminal backend, the Bot Desktop, sessions and their database, and
the gateways. The bridge configures them (through Hermes' own CLI and config files) and talks to Hermes through its
Runs API; it never forks Hermes. Anything general enough is offered upstream instead of being kept here.
