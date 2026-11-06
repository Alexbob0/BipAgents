# The phone exit: protocol v1

An agent's HTTPS connections to a few sites leave through its person's phone instead of the place's datacenter
address, for sites that refuse servers (ameli: Cloudflare; leboncoin: DataDome). No VPN and no Tailscale: the phone
opens **one outbound WebSocket** to its bridge, and the site's TCP streams are multiplexed over it. TLS stays end to
end between the agent's Chrome and the site; the bridge and the phone only copy bytes and read the ClientHello's
server name.

This document is the contract between the bridge (`bridge-rs/src/phone_exit.rs`, owner of the protocol) and the two
clients (Android `io.github.bipagents.phoneexit`, iOS on branch `phone-exit-ios`). **Version 1.** A change to
anything below is a protocol change: it is said in the branch's report and mirrored by both apps.

```text
agent's Chrome ─CONNECT─▶ bipagents-egress ─(unix socket « EXIT host 443 agent »)─▶ hub (bridge)
                                                     ─(WebSocket frames)─▶ phone ─TCP─▶ site
```

## 1. Product rules (what the protocol enforces)

1. **Direct by default.** A site goes through the phone only after a refusal: the agent's browser lands on a
   challenge/403/block page (the bridge detects it and asks), or the agent calls `ask_phone_exit`. The person then
   gets a card « Passer par mon téléphone pour <site> ? » with **once** / **always for this site** / **no**.
2. **Per site, in Settings.** A site the person set to « Toujours » goes through the phone whenever an agent of theirs
   opens it; « Jamais » never asks again. Anything else asks (a card) after a refusal.
3. **Never a silent fallback.** A site routed to the phone never leaves from the datacenter instead. No phone: the
   stream is refused with its reason (`paused`, `not_allowed`, `cap`…), and the agent is told that reason (§7), so
   it never says « the site blocks servers » wrongly.
4. **Only the agent's owner.** Never for a shared agent, a family/household agent, a project agent, or an agent linked
   with another household (`vault::others_facing`): the tool is not offered, the card is never made, the hub refuses
   (`not_allowed`).
5. **Visible and stoppable.** A foreground notification (Android) or the « Relais » screen (iOS) shows the site and
   the MB while it relays, with « Arrêter ». Stopping on the phone, in the app's Settings, or from the bridge
   (`DELETE /v1/exit/session`) ends it at once.
6. **Guardrails kept.** No CAPTCHA solving (a CAPTCHA goes to the person: « Prendre la main »), caps per session,
   human pace (new streams per minute), no mass scraping, no IP rotation, never for anyone else.

## 2. Sessions

A **session** is what a person accepted: one agent, one or more sites, its caps. It is *not* a WebSocket: the phone
may reconnect (network change, app restart) and the session goes on with the same counters.

| State | Meaning | Streams |
|---|---|---|
| `accepted` | The person said yes; no phone carries it yet | refused `paused` |
| `active` | A tunnel carries it (listed in the phone's `HELLO`/`SESSIONS`, not paused) | opened through the phone |
| `paused` | It was active; the tunnel is gone, or the phone said `PAUSE` (Wi-Fi only on 4G, person's pause) | refused `paused` (`wifi_only` when that is why) |
| `ended` | Over, with a reason (below); it is in the journal | refused with the end reason for 10 min, then `not_allowed` |

A card refused (« Non ») makes no session.

**End reasons** (`ended`, also `BYE`/`END` payloads): `user_stop` (Arrêter on the phone, Settings, or the bridge's
kill switch), `cap` (data cap), `expired` (time cap), `idle` (no stream for `idle_minutes`), `off` (the person
switched « Autoriser » off, or the place switched the feature off), `task_done` (the agent said it is done:
`ask_phone_exit` with `done`), `replaced` (a newer session for the same agent and sites).

**Caps, per session** (counted by the bridge on the bytes it carries both ways for that session, across
reconnects):

| Cap | Default | Where set |
|---|---|---|
| Data | **50 MB**; the person picks 50 MB / 200 MB / 1 GB / Sans limite (`cap_mb`: 50, 200, 1000, 0 = no data cap) | app Settings (`PUT /v1/exit/settings`), copied into each session when accepted |
| Time | 30 min from acceptance | bridge.toml `[phone_exit] max_minutes` |
| Idle | 10 min without any stream open | bridge.toml `idle_minutes` |
| New streams | 120 per minute per tunnel (refused `rate` beyond) | bridge.toml `connections_per_minute` |

« Sans limite » turns the **data** cap off only; time, idle and the per-minute limit stay. The MB used are shown in
the notification and the journal either way. « Wi-Fi seulement » is independent of the cap: on mobile data the phone
sends `PAUSE {"why":"wifi_only"}` and refuses opens with `wifi_only`.

## 3. Routing

- A **site** is a registrable domain (`ameli.fr`, `impots.gouv.fr`): the site itself and all its subdomains.
- The bridge writes, per agent, the sites routed to the phone into the proxy's policy file (`agents.<a>.phone`):
  the sites of the agent's live sessions (`accepted`, `active`, `paused`) and the sites set to « Toujours ».
- **Minimal route** (on by default, a switch per site and a global one): static-asset hosts of a routed site go
  direct, the page and its API go through the phone. Static = the first label under the site is one of `assets`,
  `static`, `statics`, `cdn`, `img`, `images`, `image`, `media`, `medias`, `fonts`, `font`, `css`, `js`, `pics`,
  `photos`, `thumbs`, `video`, `videos` (e.g. `assets.doctolib.fr`, `img.leboncoin.fr`). The apex and `www` never.
  Turn it off for a site that compares addresses between its page and its assets.
- Hosts of other domains (a CDN of another name, `challenges.cloudflare.com`) go direct.
- The phone's DNS decides (the bridge sends the **name**, never an address). A home ad-blocking DNS answering
  `0.0.0.0` makes the phone refuse that tracker (`private`): wanted, the agent sees the web « as at home ».

## 4. HTTP API (bridge key, under `/v1`)

All JSON. `site` values are registrable domains; the bridge normalizes what it is given (`https://www.ameli.fr/x` →
`ameli.fr`).

| Route | Body | Answer |
|---|---|---|
| `GET /v1/exit` | — | `Overview` |
| `PUT /v1/exit/settings` | any of `{"allowed": bool, "cap_mb": 0\|50\|200\|1000, "wifi_only": bool, "minimal_route": bool}` | `Overview` |
| `PUT /v1/exit/sites/{site}` | `{"mode": "always"\|"never"\|"ask", "minimal": bool?}` | `Overview` |
| `DELETE /v1/exit/sites/{site}` | — | `Overview` (the site asks again) |
| `GET /v1/exit/sessions` | — | `{"sessions": [Session]}`: the live ones (`accepted`, `active`, `paused`) |
| `POST /v1/exit/sessions` | `{"agent": id, "sites": [site]}` | `{"session": Session}`: the person starts one from the app |
| `DELETE /v1/exit/sessions/{id}` | — | `{"stopped": bool}` (`user_stop`) |
| `DELETE /v1/exit/session` | — | `{"stopped": bool}`: the kill switch, every session ends `user_stop` and the tunnel gets `BYE` |
| `GET /v1/exit/session` | — | the tunnel's state (phase 0 shape, plus `sessions`) |
| `GET /v1/exit/journal?limit=50` | — | `{"sessions": [Session]}`: ended ones, newest first (at most 200 kept) |
| `POST /v1/approve` | `{"agent", "run_id": "bip-exit-<id>", "choice": "once"\|"always"\|"deny", "request_id": "<id>"}` | `{"ok": true, "choice", "exit": {"session": Session, "tunnel": "/v1/exit/tunnel"}}`; `exit` absent on `deny` |
| `GET /v1/exit/tunnel` | WebSocket (§5) | — |

`Overview`:

```json
{"enabled": true,               // the feature exists on this place ([phone_exit] enabled)
 "settings": {"allowed": true, "cap_mb": 50, "wifi_only": false, "minimal_route": true},
 "cap_choices": [50, 200, 1000, 0],
 "sites": [{"site": "ameli.fr", "mode": "always", "minimal": true, "at": 1730000000.0}],
 "sessions": [Session], "tunnel": {"connected": true, "device": "…", "network": "wifi"}}
```

`Session`:

```json
{"id": "Xk3…", "agent": "vie", "sites": ["ameli.fr"], "state": "active",
 "cap_mb": 50, "max_minutes": 30, "wifi_only": false, "minimal": true,
 "up": 120034, "down": 3412991, "used_bytes": 3533025, "streams": 14,
 "hosts": {"www.ameli.fr": 3120044, "assure.ameli.fr": 412981},
 "created_at": 1730000000.0, "accepted_at": 1730000004.2, "ends_at": 1730001804.2,
 "ended_at": null, "ended": null, "network": "wifi", "via": "app"}
```

`used_bytes` = `up + down`; the data cap applies to it (none when `cap_mb` is 0).

### The card (in `GET /v1/approvals`, the conversation and the Boîte)

```json
{"run_id": "bip-exit-<id>", "request_id": "<id>", "agent": "vie",
 "choices": ["once", "always", "deny"],          // ["once", "deny"] when the site is already « Toujours »
 "description": "Ameli refuse les serveurs. Vie peut passer par ta connexion pour ameli.fr.",
 "bip_exit": {"site": "ameli.fr", "host": "www.ameli.fr", "reason": "Cloudflare challenge",
              "kind": "ask",                       // "ask": after a refusal; "restart": a « Toujours » site, no phone
              "cap_mb": 50, "wifi_only": false}}
```

Buttons: `once` « Passer par mon téléphone », `always` « Toujours pour ce site », `deny` « Non ». On `once`/`always`
the app that answered becomes the exit: it shows the consent sheet the first time (§8), then starts its relay with
the session it got back. An app that cannot relay answers anyway; the session waits `accepted` (streams `paused`)
until a phone carries it or `idle` ends it.

The push for a new card is a normal card push (category `CARD`, `t: "connector"`): it opens the conversation on the
card. Tapping « Passer par mon téléphone » there starts the relay (Android: a foreground-service start from a visible
activity, allowed).

## 5. The tunnel

`GET /v1/exit/tunnel`, WebSocket over the place's HTTPS door (port 443; a LAN door's certificate is pinned like the
app's other requests, a `*.bipagents.com` name has a public certificate).

- **Auth**: `Authorization: Bearer <bridge key>` (the app's key for this place). Debug builds and tests may use the
  `[phone_exit] token_sha256` token. Wrong or missing: `401`. Feature off: `404`.
- One tunnel per place: a new one replaces the one before (`BYE replaced` to the old).
- The bridge pings every 20 s; clients answer pongs (WebSocket libraries do).
- A frame is one **binary** message: `type u8 | stream u32 BE | length u32 BE | payload`, payload ≤ 64 KiB.
  A text message, a bad frame, or a bigger payload ends the tunnel (`protocol`).

| Type | Name | Way | Stream | Payload |
|---|---|---|---|---|
| 1 | `HELLO` | phone → bridge, first, within 10 s | 0 | JSON `{"v": 1, "device": "<stable random id>", "platform": "android"\|"ios", "app": "<version>", "network": "wifi"\|"cellular"\|"other", "sessions": ["<id>", …]}` |
| 13 | `WELCOME` | bridge → phone, after `HELLO` and each `SESSIONS` | 0 | JSON `{"v": 1, "sessions": [Session], "refused": [{"id", "why"}]}`: the sessions this tunnel carries now (their `used_bytes` so far), the ones it may not carry and why (`ended:<reason>`, `unknown`, `not_allowed`) |
| 14 | `SESSIONS` | phone → bridge | 0 | JSON `{"sessions": ["<id>", …]}`: the full set the phone carries now (a card accepted while relaying adds one) |
| 2 | `OPEN` | bridge → phone | new id | JSON `{"host", "port": 443, "agent", "session"}` |
| 3 | `OPEN_OK` | phone → bridge | id | empty |
| 4 | `OPEN_ERR` | phone → bridge | id | reason code (§6), ASCII |
| 5 | `DATA` | both | id | bytes (TLS records, opaque), ≤ 16 KiB per frame |
| 6 | `WINDOW` | both | id | u32 BE: bytes the receiver consumed (credit given back) |
| 7 | `FIN` | both | id | empty: this side will send no more |
| 8 | `RST` | both | id | empty: the stream is dead |
| 9 | `PAUSE` | phone → bridge | 0 | JSON `{"why": "wifi_only"\|"user"\|"network"}`: refuse new streams until `RESUME` |
| 10 | `RESUME` | phone → bridge | 0 | empty or `{}` |
| 11 | `STATS` | phone → bridge, every 5 s | 0 | JSON `{"network", "up", "down", "streams"}` (informative: the bridge counts the caps itself) |
| 15 | `END` | both | 0 | JSON `{"id", "why"}`: one session over (bridge → phone: `cap`, `expired`, `idle`, `user_stop`, `off`, `task_done`; phone → bridge: `user_stop`). Its open streams get `RST` |
| 16 | `USAGE` | bridge → phone, every 5 s while relaying | 0 | JSON `{"sessions": [{"id", "used_bytes", "cap_mb", "state"}]}`: for the notification's MB |
| 12 | `BYE` | both | 0 | reason: `user_stop`, `off`, `replaced`, `no_session` (nothing left to carry), `protocol` |

**Flow control.** Each side may have `WINDOW` = 256 KiB of a stream unacknowledged; the receiver sends `WINDOW n`
after consuming ≥ 64 KiB. A sender never exceeds its credit.

**Streams.** The bridge sends `OPEN`; the phone runs its checks (§6), resolves the name, connects to the checked
address (no second resolution), then `OPEN_OK` (or `OPEN_ERR reason`) within 25 s. The first bytes the bridge sends
are the ClientHello; the phone checks its server name equals `host` (else `RST`). Then `DATA` both ways; `FIN` each
way; `RST` on any error.

**Reconnect.** On a network change the phone reconnects **at once** (no back-off for the first try; then 1 s, 2 s, 5 s,
10 s) and sends `HELLO` with the same sessions. Streams of the old tunnel are lost (the agent reloads: it is told
`network_changed`). After `BYE`, an `END` of its last session, or a 401/404, the phone stops and does not reconnect.

## 6. Checks and refusal codes

The bridge refuses before asking the phone; **the phone checks again and has the last word** (a compromised place
cannot widen it): its own list (the sites of the sessions it carries, exact or subdomain), port 443 only, never an
address literal (IPv4, IPv6, decimal/hex forms, NAT64 `64:ff9b::/96` and `64:ff9b:1::/48`, 6to4 `2002::/16`,
`::ffff:`), never a local name (no dot, `.local`, `.lan`, `.home`, `.internal`, `.localhost`, `.arpa`, `.test`,
`.invalid`, `.localdomain`), after resolving never a private, loopback, link-local, shared (100.64/10), multicast,
reserved, ULA, or NAT64/6to4-wrapped private address (a carrier's DNS64 `64:ff9b::/96` answer is judged by the IPv4
inside), the SNI equal to the host.

Codes (snake_case; `OPEN_ERR` payloads, the hub's answers to the proxy, the agent's notes):

| Code | Who | Meaning |
|---|---|---|
| `paused` | bridge | no phone carries a session for this site now (stopped, network lost, not started) |
| `wifi_only` | both | the phone is on mobile data and the person allows Wi-Fi only |
| `not_allowed` | both | the site is not in a live session or « Toujours » for this agent (or the agent is not the person's own) |
| `cap` | bridge | the session's data cap is reached |
| `expired` | bridge | the session's time cap is reached |
| `off` | bridge | the person (« Autoriser ») or the place switched it off |
| `rate` | bridge | too many new streams this minute |
| `literal_ip`, `local_name`, `bad_name`, `port` | both | the name or port |
| `private` | both | the name resolves to a private/local address |
| `dns`, `connect` | phone | could not resolve / connect |
| `sni_mismatch`, `sni_missing`, `not_tls` | both | the first bytes are not a ClientHello for that host |

A stream the client closes before sending its ClientHello (Chrome's preconnects) is logged `closed_before_hello`,
never as a refusal.

## 7. What the agent is told

- The browser tool appends one plain line to a page that failed because the phone refused (the bridge keeps each
  agent's last refusals for 2 min), e.g. « Phone exit: this site goes through your person's phone, which is not
  relaying now (paused). Do not say the site blocks you … ».
- A page that is a challenge or block page for servers (Cloudflare « Just a moment », DataDome, PerimeterX,
  « Access denied » 403) gets a line too: the card was made for the person (or why not: « Jamais », not the person's
  own agent, feature off), and « never solve a CAPTCHA ».
- `ask_phone_exit` (bridge MCP tool, owned agents only): `{"site", "reason"}` makes the card (or says it is waiting,
  already live, or why not); `{"site", "done": true}` ends the agent's session for that site (`task_done`).

## 8. Apps

- **Consent, once per phone** (plain words; Play's prominent disclosure): what goes through the phone (encrypted
  connections of your own agent to the sites you accept, only while you allow it), what the phone sees (the site's
  name and volumes, never the content), that the site sees your internet address, that mobile data may be used up to
  your cap, that you can stop at any time, and that it is never for someone else. An explicit tap; refusing answers
  the card « Non ».
- **During a session**: Android, a `specialUse` foreground service (subtype `agent-browsing-relay`) with the
  notification « <Agent> navigue depuis ce téléphone : ameli.fr · 3,2 Mo » and « Arrêter »; iOS, the « Relais »
  screen (foreground; ~30 s grace in background, then `BYE`).
- **Settings › « Navigation par mon téléphone »**: « Autoriser mes agents à passer par ce téléphone », the per-site
  list (Toujours / Demander / Jamais, minimal route), « Wi-Fi seulement », the cap per session (50 Mo / 200 Mo / 1 Go /
  Sans limite), the journal (one line per session: agent, sites, MB, duration, how it ended).
- **Journal**: each ended session is also one Boîte line, e.g. « Vie a navigué via ton téléphone : ameli.fr ·
  6,2 Mo · 4 min · arrêté : tâche finie ».

## 9. bridge.toml

```toml
[phone_exit]
enabled = true            # the feature on this place (off by default)
max_minutes = 30          # time cap per session
idle_minutes = 10
connections_per_minute = 120
default_cap_mb = 50       # the person's cap until they choose one
# token_sha256 = "…"      # tests and debug builds only: SHA-256 of a debug token accepted instead of the bridge key
# hub_resolve = true      # the hub also resolves and refuses names pointing to private addresses here
```
