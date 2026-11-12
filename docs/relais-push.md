🇬🇧 English · [🇫🇷 Français](relais-push.fr.md)

# Notification relay (for an App Store release)

The APNs `.p8` key and the Firebase account belong to the team publishing the app and cannot be distributed to
users' bridges. A central service, the only holder of both, receives the bridges' notifications and forwards them to
Apple and Google (the ntfy.sh model for self-hosted ntfy servers).

**Implemented in [`relay/`](../relay/README.md)** (2026-10-18), in Rust: one small binary, no database. The app
registers its device token and gets back a *relay token* (the device token sealed with a key only the relay holds);
its bridge sends that relay token with each notification. No message text goes through in clear: the agent's name and
the start of its message are sealed for the phone's own key (only the phone opens them), and the relay keeps only a
neutral line (« New message », « Approval needed »…, `contracts/notifications/loc-keys.json`) and short ids (the
agent's id among them).

Every install sends through the relay: `[push] service_url = "https://push.bipagents.com"`, set by the installer and,
on older installs, by the 2026-11-12 update. With it, the bridge no longer sends to Apple itself, even with an `[apns]`
section (which can be removed); phones register with the relay again at their next launch.

## Orders of magnitude (to be rechecked at launch)
- One copy (≈ 4 MB of memory) covers a large number of installs; it can run as a serverless container that stops
  when idle, or on a small VM.
- Running costs grow with the number of notifications, not with storage: there is none.
