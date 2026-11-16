🇬🇧 English · [🇫🇷 Français](README.fr.md)

# BipAgents push relay

People run BipAgents on their own server; their bridge cannot hold the publisher's Apple key or Firebase account.
The relay does, and forwards their notifications to Apple and Google. It is one small Rust program (a 4.6 MB
binary, ~4 MB of memory idle) that **stores nothing** and **forwards no content**.

## How it works

1. The app registers its Apple or Firebase token: `POST /v1/register` → a **relay token** (`r2.…`), the device
   token sealed with a key only the relay holds (XChaCha20-Poly1305), with its own id, an end (90 days) and who
   may send with it (`audience`: `bridge`, or `guichet`). The app gives it to its bridge (or to the guichet), and
   takes a new one every 30 days.
2. The bridge sends its notification with the relay token: `POST /v1/relay/push` (the same shape as the bridges'
   own relay). The relay opens the token, cleans the payload, and sends to Apple (HTTP/2, ES256 provider token)
   or Google (FCM v1).
3. Apple's or Google's answer goes back to the bridge; `status: 410` means the device is gone, and the bridge
   forgets it.

Nothing to back up, no database: any number of copies can run side by side, and they can stop when idle.

## What it never sees, what it keeps out

- **No message text.** A bridge set to show previews puts a line of the message in the alert's body; the relay
  replaces any body that is not one of the app's generic, translated texts (`loc-key`) with « Nouveau message ».
  Next to `aps`, only short id-like values pass (agent id, outbox id, session id…). The app's notification
  extension fetches the real text from the person's own bridge.
- **No device token in clear**, in its storage (there is none) or its logs (a 4-byte fingerprint only).
- **Quotas**: per device (300 pushes an hour), per address (20,000 pushes, 60 registrations an hour).

## API

| Route | Body | Answer |
| --- | --- | --- |
| `POST /v1/register` | `{"platform": "apns"\|"fcm", "token": "…", "environment": "sandbox"\|"production", "audience": "bridge"\|"guichet"}` | `{"relay_token": "r2.…", "expires": 1800000000}` |
| `POST /v1/relay/push` (or `/v1/push`) | `{"token": "r2.…", "payload": {…}, "push_type": "alert"\|"background"\|"liveactivity", "priority": 5\|10, "collapse_id": "…"}`; a `guichet` token with `Authorization: Bearer <RELAY_GUICHET_KEY>` | `{"status": 200, "reason": null, "apns_id": "…"}` |
| `POST /v1/report` | `{"reason", "comment", "text", "category", "app", "model", "platform": "ios"\|"android", "lang"}` | `202 {"ok": true}`: « Report this reply » passed on to `RELAY_REPORT_URL` with the secret, kept nowhere; 503 when not set up, 502 when it couldn't go (the app then offers an e-mail) |
| `GET /health` | | `{"status": "ok", "apns": true, "fcm": true, "family": {"enabled": true}}` |

A token sealed by another key, or altered, answers `{"status": 410, "reason": "BadRelayToken"}`; an ended one, or
one of the first format (`r1.…`, without an end), `{"status": 410, "reason": "ExpiredRelayToken"}` (the bridge
forgets it, the app registers again); a raw device token is refused with HTTP 400 (a bridge set up for direct
delivery, not a dead device). A `guichet` token sent without the guichet's key is refused with HTTP 401: a copy of
the guichet's tokens is useless alone.

**Transition (until `RELAY_R1_UNTIL`, 2026-12-15 by default).** Apps, bridges and the guichet released before
2026-10-28 only know the first format, so until that day: `r1.` tokens still work (any sender, as before); an app
that registers without `audience` (an older one) gets an `r1.` token; a `bridge` registration also returns
`legacy_token` (`r1.`), which a newer app gives a bridge that refuses `r2.`; a `guichet` token sent with no
`Authorization` at all passes (an older guichet). Each such use is logged (`warn`), and `/health` adds `r1_until`,
`r1_used`, `guichet_without_key` and a `warning`. After that day, everything above applies.

What passes in `aps`: no `alert.title` (the apps name the agent from their own list); the guichet's texts (servers,
account, family, shared conversations) only on a `guichet` token; `category` `MESSAGE`/`APPROVAL`, `sound`
`default`, `interruption-level` `active`/`passive`, nothing else.

## Families: two households that can't reach each other

Since 2026-11-15 the relay also lets two households' bridges link and talk when neither can reach the other (two
installs at home on different networks, or a place of the offer scanning a home's invitation). Both connect out; the
rendezvous is in memory (`src/family.rs`, the bridge's side: `bridge-rs/src/family_rendezvous.rs`).

| Route | | |
| --- | --- | --- |
| `POST /v1/family/mailbox` | `{}`, `{"token": "fm1.…"}` (renew), or `{"invite_until": <unix>}` (an invitation's) | `{"mailbox": "<22 chars>", "token": "fm1.…", "expires": …}`: the id sealed in the token, nothing stored |
| `POST /v1/family/pass` | the mailbox's token; `{"for": "<the other household's mailbox>", "after": [ids withdrawn]}` (newer than them, whatever the relay's clock), or `{"renew": "fp1.…"}` (only by the mailbox it opens) | `{"pass": "fp1.…", "id": …, "expires": …}`: lets that mailbox, and no other, call this one (sealed: both mailboxes, an id, an end) |
| `POST /v1/family/passes` | the mailbox's token; `{"accept": [ids], "seq": n}` | 204: the passes it accepts from now on (a link removed); its polls say it too (`x-bip-passes`, `x-bip-passes-seq`); the relay keeps the statement with the highest number (one made before a withdrawal never undoes it) |
| `GET /v1/family/next` | the mailbox's token | the next call for it (`x-bip-call`, `x-bip-from`, `x-bip-kind`, its bytes), or 204 after 25 s |
| `POST /v1/family/call/<mailbox>` | the caller's own token, `Content-Length`, `x-bip-kind: call` with `x-bip-pass`, or `invite` (≤ 64 KiB, to an invitation's mailbox), the sealed bytes (≤ 16 MiB) | the answer, streamed as the other bridge sends it; 503 `away` when that bridge is not connected **or the caller has no pass for it** (the same answer) |
| `POST /v1/family/answer/<call>` | the token of the mailbox that took the call | 204; streamed on to the caller (≤ 64 MiB) |

**Only linked households call a mailbox.** At the link each bridge gives the other a pass to its own mailbox, for the
other's mailbox only, sealed in their exchange; it says with each poll which passes it still accepts. A pass's id
carries its generation (when it was made, signed by the relay for that mailbox): the relay keeps, for a mailbox that
polls, only the oldest generation among the passes it says, one number. Removing a link, the bridge first gives the
households it keeps a pass of a newer generation (through their links, `POST /family/relay-pass`), so the pass
withdrawn, older, is refused at once and whatever the relay remembers (after a restart, nothing reaches a mailbox
before its first poll says it again). A pass works for the mailbox it was given to only, and only the mailbox it opens
renews it. A household's mailbox id is never in a QR code: an invitation has a mailbox of its own, ending with
it, that takes only an introduction (`invite`). A stranger who learns a mailbox id can't call it, use up its quotas,
nor tell whether its bridge is on.

**Sealed end to end**: the bridges seal everything with a key of each link that only the two households hold (an
X25519 exchange at the link, bound to the invitation's one-time code; ChaCha20-Poly1305 frames, a key per call,
direction and answer, replays refused by the bridge). The relay **sees** the bridges' addresses, which mailbox calls
which, when, how many bytes, and, while a bridge keeps its poll open, that it is on and from which address. It
**keeps** nothing on disk and logs no call that went through (no record of who talks to whom): only some refusals
(quota, blocked mailbox, answer cut), with a fingerprint of one mailbox keyed by the relay (HMAC-SHA256,
not computable from the id alone), and one count an hour. In memory: the ids of the mailboxes polling (until an hour
after), the quotas' counts under unkeyed SHA-256 hashes (forgotten within the hour), the calls while they pass.
`/health` says whether it is on, nothing of its activity.

**Limits.** A call's bytes are reserved as they are read, never on the length it says (256 MiB held at most, every
call together; 32 MiB per caller, per link to a callee and per address); a body that stops for 5 s is cut, so is one
slower than 16 KiB/s on average past its first second (a caller sending next to nothing holds no room; a slow upload
still has the 30 s), and all of it comes within 30 s; a refused call's body is never kept. The small routes
(`mailbox`, `pass`, `passes`) read 64 KiB at most, within 10 s. What a mailbox takes is one number kept with its
polls (nothing to bound). Quotas: calls and bytes per caller, and per pass (what one household may send another); mailboxes
and passes made per address. A mailbox or a pass can be refused by its fingerprint in `RELAY_BLOCKED`. Run **one** copy
for the families (the rendezvous is in memory). Deploy: `RELAY_TRUST_FORWARDED_FOR=1` behind a proxy, a high open-files
limit (one waiting request per household linked through the relay), `flush_interval -1` in Caddy, logs kept 14 days.
**Retiring a sealing key** (`RELAY_SEAL_KEYS`, the first seals): keep the old key listed, not first, for 210 days at
least. Bridges give their households fresh passes every 30 days and renew their mailbox's token when 200 days are left
of its 400; a bridge off for longer than that, or a link whose other bridge stayed off past it, is made again.

## Settings (environment only)

| Variable | |
| --- | --- |
| `RELAY_SEAL_KEYS` | Required. Base64 keys of 32 bytes, comma-separated; the first seals, all open (`bipagents-relay keygen`). To change keys: put the new one first, keep the old one a few months. |
| `APNS_KEY` or `APNS_KEY_FILE`, `APNS_KEY_ID`, `APNS_TEAM_ID`, `APNS_TOPIC` | Apple: the team's .p8 key and the app's bundle id. Without them, Apple devices are refused. |
| `FCM_SERVICE_ACCOUNT` or `FCM_SERVICE_ACCOUNT_FILE` | Google: the Firebase service account (JSON). Without it, Android devices are refused. |
| `PORT` or `RELAY_LISTEN` | `8080` / `0.0.0.0:8080` |
| `RELAY_PUSHES_PER_DEVICE_HOUR`, `RELAY_PUSHES_PER_IP_HOUR`, `RELAY_REGISTRATIONS_PER_IP_HOUR` | `300`, `20000`, `60` |
| `RELAY_TRUST_FORWARDED_FOR` | `1` behind a proxy that appends to `X-Forwarded-For` (its last address counts; IPv6 by /64) |
| `RELAY_REPORT_URL`, `RELAY_REPORT_SECRET` or `RELAY_REPORT_SECRET_FILE`, `RELAY_REPORTS_PER_IP_HOUR` | where reports go (the guichet's `/v1/reports`), the secret it checks, `20` |
| `RELAY_BLOCKED` or `RELAY_BLOCKED_FILE` | devices refused for abuse (terms of use), by the 8-hex fingerprint the logs show, or one relay token that leaked, by its 16-hex id (`token` in the logs); restart to apply |
| `RELAY_GUICHET_KEY` or `RELAY_GUICHET_KEY_FILE` | the key the guichet sends its pushes with (comma-separated to change it: old and new both pass); without it, `guichet` tokens are refused |
| `RELAY_R1_UNTIL` | `2026-12-15` (UTC midnight): the end of the transition from `r1.` tokens (see above); `0` ends it now |
| `RELAY_FAMILY` | `1`; `0` turns the families' rendezvous off (its routes answer 404) |
| `RELAY_FAMILY_REQUEST_MB`, `RELAY_FAMILY_ANSWER_MB` | `16`, `64`: a call's size, its answer's |
| `RELAY_FAMILY_BODY_KIB_PER_S` | `16`: how fast a call's body comes at least, past its first second (`0`: no minimum) |
| `RELAY_FAMILY_CALLS_PER_HOUR`, `RELAY_FAMILY_MB_PER_HOUR`, `RELAY_FAMILY_MAILBOXES_PER_IP_HOUR` | `3000` per mailbox as caller and as callee, `2048` MiB per mailbox, `20` mailboxes made per address |
| `RUST_LOG` | `info` (JSON lines) |

## Run it

```bash
cargo run --release -- keygen                      # a sealing key
RELAY_SEAL_KEYS=… APNS_KEY_FILE=AuthKey.p8 APNS_KEY_ID=… APNS_TEAM_ID=… APNS_TOPIC=com.kerfystudio.bipagents \
  cargo run --release
```

As a container (a 7.5 MB image, nothing in it but the binary): `podman build -t bipagents-relay relay/`.

It needs HTTPS in front: a serverless container offer (it starts in milliseconds and may scale to zero), or a small
VM with Caddy. One copy handles a large number of installs; add copies behind the same address as needed.

## Develop

`cargo test` (Apple and Google played by a local server), `cargo clippy --all-targets`, `cargo fmt --check`.
