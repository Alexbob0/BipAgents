🇬🇧 English · [🇫🇷 Français](relais-push.fr.md)

# Notification relay (for an App Store release)

Decision of 2026-10-04: **not needed as long as the app is used only by its author** (aibox's bridge sends to APNs itself with the team's `.p8` key). The day the app is published on the App Store, the relay will run on **a small Hetzner VM**.

## Why a relay
The APNs `.p8` key belongs to the team publishing the app and cannot be distributed to users' bridges. A central service, the only holder of the key, receives requests from the bridges and forwards them to APNs (the ntfy.sh model for self-hosted ntfy servers).

## Planned operation
1. The app registers with the relay using its APNs device token and receives an opaque token, which it passes on to its bridge.
2. The bridge sends the relay `{token, type: MESSAGE|APPROVAL|SILENT, ids}`: never any text (payloads are already generic by default, see `bridge/README.md`).
3. The relay signs the ES256 JWT, sends to APNs (HTTP/2, persistent connections) and deletes tokens that APNs reports as invalid (410, `BadDeviceToken`).
4. The Notification Service Extension fetches the content from the user's bridge, over their tailnet.

Safeguards: bridge authentication by token, per-token quotas, content-free logging.

## Orders of magnitude (to be rechecked at launch)
- Up to ~10,000 users: a ~€4–5/month VM is enough.
- ~1M users: a few hundred euros per month, mostly operating costs (multi-region, monitoring, GDPR, abuse).

## Code side
Starting point: `bridge/bipbridge/apns.py` (APNs client already written). On the bridge side, add a "via relay" mode next to the current "direct APNs" mode; the app only has to send the relay token instead of the device token.
