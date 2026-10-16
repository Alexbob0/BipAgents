# BipAgents for Android

The Android app, native (Kotlin, Jetpack Compose), talking to the same bridge and Hermes as the iPhone app. Work in
progress, in three phases:

1. **Core** (done): agents added with the install's QR code, conversations sent as Hermes runs and followed through the
   bridge, approvals and questions, the Inbox, files, notifications (FCM, with approve/deny on the notification).
2. **Audio** (done): voice notes (hold the mic; transcribed on the phone, kept when the phone recognizes the language on
   the device), « Listen » under replies (the Bip's voice streamed from the bridge), spoken replies after a voice note.
3. **Live** (done, to tune on real phones): hands-free conversation — the phone's recognizer listens, the reply streams
   from Hermes (`chat/stream`, thinking off) and is spoken sentence by sentence in the Bip's live voice; speaking over
   it (the phone's echo canceller, then a voice clearly louder than the Bip's echo) or tapping the Bip interrupts it.
   In a debug build, a long press on the Bip says « Comment mieux dormir ? » (the emulator's microphone hears nothing).

## Building

Android Studio, or: `./gradlew :app:assembleDebug` (JDK 17+, the Android SDK in `local.properties`). Tests:
`./gradlew :app:testDebugUnitTest`.

On the emulator, a debug build can be paired without the camera:

```bash
adb shell am start -n io.github.bipagents/.MainActivity --es pair '{"v":2,"bridgeURL":"https://…","bridgeKey":"…"}'
```

`--es push '{"agent":"…","category":"APPROVAL","run_id":"…"}'` shows the notification such a push would.

Notifications need the publisher's Firebase project: `firebase.appId`, `firebase.apiKey`, `firebase.projectId` and
`firebase.senderId` in `local.properties` (from its `google-services.json`); without them the app builds without
notifications. The bridge sends them with the project's service account (`[fcm] service_account`).

Debug builds also accept plain HTTP to `10.0.2.2` (the development machine), for a bridge running there.

The Nunito font is under the SIL Open Font License (`licenses/Nunito-OFL.txt`).
