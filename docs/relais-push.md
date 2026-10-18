🇬🇧 English · [🇫🇷 Français](relais-push.fr.md)

# Notification relay (for an App Store release)

The APNs `.p8` key and the Firebase account belong to the team publishing the app and cannot be distributed to
users' bridges. A central service, the only holder of both, receives the bridges' notifications and forwards them to
Apple and Google (the ntfy.sh model for self-hosted ntfy servers).

**Implemented in [`relay/`](../relay/README.md)** (2026-10-18), in Rust: one small binary, no database. The app
registers its device token and gets back a *relay token* (the device token sealed with a key only the relay holds);
its bridge sends that relay token with each notification. No message text goes through: previews are replaced by the
generic, translated text, and the app's notification extension fetches the content from the person's own bridge.

While the app is used only by its author, the bridge can keep sending to Apple itself with the team's key (decision of
2026-10-04); the relay is for the public release.

## Orders of magnitude (to be rechecked at launch)
- One copy (≈ 4 MB of memory) covers a large number of installs; it can run as a serverless container that stops
  when idle, or on a small VM.
- Running costs grow with the number of notifications, not with storage: there is none.
