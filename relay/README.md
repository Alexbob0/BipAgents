🇬🇧 English · [🇫🇷 Français](README.fr.md)

# BipAgents push relay

People run BipAgents on their own server; their bridge cannot hold the publisher's Apple key or Firebase account.
The relay does, and forwards their notifications to Apple and Google. It is one small Rust program (a 4.6 MB
binary, ~4 MB of memory idle) that **stores nothing** and **forwards no content**.

## How it works

1. The app registers its Apple or Firebase token: `POST /v1/register` → a **relay token** (`r1.…`), the device
   token sealed with a key only the relay holds (XChaCha20-Poly1305). The app gives it to its bridge.
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
| `POST /v1/register` | `{"platform": "apns"\|"fcm", "token": "…", "environment": "sandbox"\|"production"}` | `{"relay_token": "r1.…"}` |
| `POST /v1/relay/push` (or `/v1/push`) | `{"token": "r1.…", "payload": {…}, "push_type": "alert"\|"background", "priority": 5\|10, "collapse_id": "…"}` | `{"status": 200, "reason": null, "apns_id": "…"}` |
| `POST /v1/report` | `{"reason", "comment", "text", "category", "app", "model", "platform": "ios"\|"android", "lang"}` | `202 {"ok": true}`: « Report this reply » passed on to `RELAY_REPORT_URL` with the secret, kept nowhere; 503 when not set up, 502 when it couldn't go (the app then offers an e-mail) |
| `GET /health` | | `{"status": "ok", "apns": true, "fcm": true}` |

A token sealed by another key, or altered, answers `{"status": 410, "reason": "BadRelayToken"}` (the bridge forgets
it, the app registers again); a raw device token is refused with HTTP 400 (a bridge set up for direct delivery, not
a dead device).

## Settings (environment only)

| Variable | |
| --- | --- |
| `RELAY_SEAL_KEYS` | Required. Base64 keys of 32 bytes, comma-separated; the first seals, all open (`bipagents-relay keygen`). To change keys: put the new one first, keep the old one a few months. |
| `APNS_KEY` or `APNS_KEY_FILE`, `APNS_KEY_ID`, `APNS_TEAM_ID`, `APNS_TOPIC` | Apple: the team's .p8 key and the app's bundle id. Without them, Apple devices are refused. |
| `FCM_SERVICE_ACCOUNT` or `FCM_SERVICE_ACCOUNT_FILE` | Google: the Firebase service account (JSON). Without it, Android devices are refused. |
| `PORT` or `RELAY_LISTEN` | `8080` / `0.0.0.0:8080` |
| `RELAY_PUSHES_PER_DEVICE_HOUR`, `RELAY_PUSHES_PER_IP_HOUR`, `RELAY_REGISTRATIONS_PER_IP_HOUR` | `300`, `20000`, `60` |
| `RELAY_TRUST_FORWARDED_FOR` | `1` behind a proxy that sets `X-Forwarded-For` |
| `RELAY_REPORT_URL`, `RELAY_REPORT_SECRET` or `RELAY_REPORT_SECRET_FILE`, `RELAY_REPORTS_PER_IP_HOUR` | where reports go (the guichet's `/v1/reports`), the secret it checks (its `secrets/relay-report.secret`), `20` |
| `RELAY_BLOCKED` or `RELAY_BLOCKED_FILE` | devices refused for abuse (terms of use), by the 8-hex fingerprint the logs show; restart to apply |
| `RUST_LOG` | `info` (JSON lines) |

## Run it

```bash
cargo run --release -- keygen                      # a sealing key
RELAY_SEAL_KEYS=… APNS_KEY_FILE=AuthKey.p8 APNS_KEY_ID=… APNS_TEAM_ID=… APNS_TOPIC=io.github.bipagents \
  cargo run --release
```

As a container (a 7.5 MB image, nothing in it but the binary): `podman build -t bipagents-relay relay/`.

It needs HTTPS in front: a serverless container offer (it starts in milliseconds and may scale to zero), or a small
VM with Caddy. One copy handles a large number of installs; add copies behind the same address as needed.

## Develop

`cargo test` (Apple and Google played by a local server), `cargo clippy --all-targets`, `cargo fmt --check`.
