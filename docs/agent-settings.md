# Agent settings and the Hermes profile writer

How the bridge (`bridge-rs/`) keeps each agent's settings and writes them into the agent's Hermes profile. This
replaces the old way, where about twenty modules each edited `config.yaml` with no lock. Each did it one of three ways:
`hermes config set` subprocesses, a PyYAML load/dump, or a plain `fs::write`. Startup « heal » functions then repaired
the drift.

## The rules

1. **One writer.** Only `profile_files.rs` writes a profile's files: `config.yaml`, `SOUL.md`, the settings record.
   It also runs Hermes' own commands that write the profile (`hermes config set` with side effects, `hermes tools
   enable/disable`). Every write:
   - holds the profile's lock: `flock` on `<profile>/.bipagents-profile.lock`, from the read to the rename. Other
     processes can take it too, and threads of this bridge take it on their own descriptor. The bridge runs on one
     thread, so the lock is never held across an `.await`: a Hermes command that writes runs to its end on a
     blocking thread, with the lock taken there;
   - is a read-modify-write of the whole document. When the change leaves it equal, nothing is written: the bytes
     on disk stay the same, including Hermes' own formatting and comments;
   - writes to a temporary file in the same folder and renames it over the old one, keeping its mode. A gateway
     never reads a half-written file.
2. **One record per agent.** `<profile>/bipagents-settings.json` (`AgentSettings`) holds the choices that today
   live only in the profile: version 1 has the thinking level the person chose. The bridge owns it and Hermes never
   reads it. It stays in the profile folder so it moves with the agent (moves, exports, restic backups). Version 1
   replaces the `.bipagents-thinking-chosen` and `.bipagents-thinking-off` mark files. There is no compatibility
   window (only beta installs, owner 2026-11-02): `migrate` runs once in the start-time reconcile of the bridge
   that brings the record, moves the marks in and removes them, and nothing reads the marks afterwards. A record
   with a higher version than this bridge knows is left alone (a downgrade keeps it).
3. **One reconcile pass.** `profile_files::reconcile(config, agent)` computes the profile's desired `config.yaml`
   from the record and the place's policy. That covers the thinking level and its GLM floor, the router's provider
   order, the tool diet (`tools.tool_search.defer`, compact skill categories, `max_turns`, routine reports,
   `clarify` in the API toolset, the browser server's filters), and the browser proxy's program path. It writes
   them under the lock in one edit. It runs:
   - at start for every agent (replacing `heal_thinking`, `heal_browser_proxy`, and the start-time
     `tool_diet::apply_all`);
   - whenever the bridge gives an agent its tools (`agent_tools::register`);
   - after a vault change.
   It is idempotent: a second run writes nothing (tested).
4. **Features ask the writer.** A module that changes something either changes the record and calls `reconcile`, or
   calls `profile_files::edit(path, |doc| …)` for its own slice: an MCP server entry (`set_server`, the way `hermes
   config set mcp_servers.<…>` would write it, with no Python process per key), `skills.disabled`, the time zone.
   Keys where Hermes' command has side effects still go through Hermes. `agents_admin::default_run` runs those
   commands under the same lock (`profile_files::locked`). They are `terminal.*` (mirrored into `.env` as
   `TERMINAL_*`), credentials (`API_SERVER_KEY` goes to `.env`), and `tools enable/disable`.

## Where each setting lives (after this change)

| Setting | Owner (truth) | Rendered into |
|---|---|---|
| name, appearance, category, voice, language follow, group, isolation, shares, answer style… | `bridge.toml` `[agents.<name>]` (`AgentConfig`, its own atomic locked writer, config.rs) | SOUL blocks, MCP env |
| thinking level chosen | `AgentSettings.thinking` | `agent.reasoning_effort`, `providers.bipagents-thinking` |
| model | `config.yaml` `model.*` (Hermes' own section; written through `edit`) | — |
| tool diet, clarify, max_turns, routine reports | computed (kind, history, vault) | `tools.*`, `skills.compact_categories`, `platform_toolsets.api_server`, `agent.max_turns`, `cron.session_reports` |
| router order | place policy (`model_provider_order`) | `providers.*.provider_order` |
| MCP servers (bridge, services, connectors, stealth browser) | bridge stores (connections, services, browsers) | `mcp_servers.*` through `edit` |
| skills switched off | `config.yaml` `skills.disabled` (written through `edit`) | — |
| sandbox and shares (`terminal.*`, `command_allowlist`) | bridge stores | through `hermes` (keeps Hermes' `.env` mirror) |
| time zones | person's zone (dates) | `timezone`, `cron.timezone` through `edit` |
| SOUL blocks (language, payments, shared memory, server, health…) | `bridge.toml` + feature stores | `SOUL.md` through `edit_text` |

The phones keep some shadow copies in UserDefaults and DataStore (`agentGroups`, `agentBridgeNames`,
`toldAgentAppearance`…). They are not touched here. The apps' API is unchanged: every endpoint reads and writes the
same JSON.

## Next steps (not done here)

- Move more choices into `AgentSettings`, one version bump each, with a migration from the profile: skills switched
  off, the model the person picked, then the SOUL personality text (blocks rendered around it).
- Make SOUL.md a rendered file: personality from the record plus the bridge blocks. `heal_souls` then joins
  `reconcile`.
- Turn installer migrations that edit profiles into record migrations.
- Once the apps read the record through `/v1/agents/<id>/settings`, drop their UserDefaults copies.
