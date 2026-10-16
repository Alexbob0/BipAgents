# BipAgents for Android

The Android app, native (Kotlin, Jetpack Compose), talking to the same bridge and Hermes as the iPhone app. Work in
progress, in three phases:

1. **Core** (in progress): agents added with the install's QR code, conversations sent as Hermes runs and followed through
   the bridge, approvals and questions, the Inbox, files, notifications.
2. **Audio**: voice notes, « Listen » under replies.
3. **Live**: hands-free conversation you can interrupt.

## Building

Android Studio, or: `./gradlew :app:assembleDebug` (JDK 17+, the Android SDK in `local.properties`). Tests:
`./gradlew :app:testDebugUnitTest`.

On the emulator, a debug build can be paired without the camera:

```bash
adb shell am start -n io.github.bipagents/.MainActivity --es pair '{"v":2,"bridgeURL":"https://…","bridgeKey":"…"}'
```

Debug builds also accept plain HTTP to `10.0.2.2` (the development machine), for a bridge running there.

The Nunito font is under the SIL Open Font License (`licenses/Nunito-OFL.txt`).
