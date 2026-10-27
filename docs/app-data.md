🇬🇧 English · [🇫🇷 Français](app-data.fr.md)

# What the BipAgents apps send over the network

**In short:** conversations, files, voice-note transcripts, health data, reminders, contacts and the agents' memory go only to the
person's own server (their bridge and Hermes). Kerfy Studio's services get account, purchase and push-routing data,
plus sealed boxes they cannot open. There are no analytics or crash SDKs. Apple and Google get what their stores,
sign-in and push need, and the voice for dictation only where the person agreed to online dictation. With the
all-included offer, an agent's own e-mail address keeps its mail on Kerfy Studio's mail server; the place's bridge
talks to that server, not the app.

This page was written from the source of the iPhone/iPad app (`App/`, `Widgets/`, `NotificationService/`,
`BipVoices/`, `Packages/`) and of the Android app (`android/`) as of 2026-10-21 (after commit `4b6eb92`). **The apps'
source is private:** the app file paths below are references, not public files. The public parts are: the bridge
([`bridge/`](../bridge/), [`bridge-rs/`](../bridge-rs/)), the installer ([`installer/`](../installer/)), the push
relay ([`relay/`](../relay/)) and the account encryption code
([`App/Features/Account/AccountCrypto.swift`](../App/Features/Account/AccountCrypto.swift),
[`android/…/account/AccountCrypto.kt`](../android/app/src/main/java/io/github/bipagents/account/AccountCrypto.kt) and
its test, [`AccountCryptoTest.kt`](../android/app/src/test/java/io/github/bipagents/account/AccountCryptoTest.kt)). It
is a reading of the code, not a capture of network traffic. The agent e-mail address (§1) was added on 2026-10-22 from
the bridge's interface contract, before the apps' code for it.

## 0. Libraries: no trackers

- **iOS** (`BipAgents.xcodeproj/.../Package.resolved`, `Packages/*/Package.swift`): one outside package,
  `onnxruntime-swift-package-manager` 1.24.2 (it runs the Bips' voices on the phone). There are no analytics, crash,
  advertising or attribution SDKs. The app does not use MetricKit, AdSupport or AppTrackingTransparency.
- **Android** (`android/app/build.gradle.kts`, `android/gradle/libs.versions.toml`): AndroidX/Compose, OkHttp,
  kotlinx-serialization, CameraX, Media3, Glance, ZXing, ONNX Runtime, Health Connect client, and these Google
  libraries: Play Billing, Play Asset Delivery, Play Integrity, Credential Manager + Google ID, ML Kit document scanner
  (Play services) and **Firebase Cloud Messaging only** (`firebase-messaging`; no Analytics, no Crashlytics, no
  `google-services` plugin).
- ONNX Runtime's Android usage telemetry to Microsoft is turned off twice: its `TelemetryInitializer` is removed from
  the manifest (`AndroidManifest.xml`), and `PocketEngine.kt` calls `setTelemetry(false)`. The iOS app has no such call.
- Firebase starts only once notifications are turned on (`firebase_messaging_auto_init_enabled=false`, `push/Push.kt`).
- Transport: HTTPS only on Android release builds (`res/xml/network_security.xml`, no cleartext). iOS has no App
  Transport Security exceptions (`Config/Info.plist`). The person's own servers on the local network or the Internet
  use self-signed certificates, accepted only when they match the pinned fingerprint (`App/Model/LocalLink.swift`,
  `net/Http.kt`, `CloudPairing`).

## 1. The person's own server: bridge and Hermes

This is either the person's own machine or VPS (reached through their tailnet, their local network or its "door"), or
their place on the all-included offer. Everything personal goes here, with that server's bearer key
(`App/Model/BridgeClient.swift`, `Packages/HermesKit/Sources/HermesKit/Client/HermesClient.swift`,
`Input/DocumentUploader.swift`, `android/.../net/Bridge.kt`, `net/Hermes.kt`, `net/Http.kt`). Public code: `bridge/`,
`bridge-rs/`.

- **Conversations:** messages, the text of voice notes and Live calls (recognised on the phone, or online only where
  the person allowed it, see §4), photos and files (`/v1/runs`, `/v1/files`, `/v1/media`), approvals and answers, live events (SSE).
  **The audio of a voice note stays on the phone.** The apps do not use WebSockets.
- **Agents:** settings, model, language, appearance, tools, connected services, memory pages (read and edited), notes
  import, pages, background jobs, scheduled tasks, shares between agents, family links (`/v1/family/*`).
- **Secrets the person enters:** e-mail account passwords (`/v1/mail/accounts`, and an agent's own mailbox at the
  person's provider in `/v1/agents/{id}/email`), vault items (`/v1/vault`), SSH public keys (`/v1/ssh/keys`), voice
  server settings.
- **Health ("Santé")** (`App/Features/Settings/HealthSync.swift`, `android/.../health/HealthSync.kt`): one summary a
  day, sent with `PUT /v1/health-data/days` to every bridge where at least one agent has "Santé" turned on, with only
  the figures of the categories the agents of that bridge read (`shared` in `GET /v1/health-data`; each agent's own
  list is set with `PATCH /v1/agents/{id}/settings {"health_categories": […]}` from the "Santé" tile's details):
  activity (steps, active energy, exercise minutes, workouts, distance, flights, stand hours), sleep (asleep/in bed,
  times, stages, breathing rate, wrist temperature), heart (resting HR, HRV, the day's average/lowest/highest HR, blood
  oxygen, VO2 max), body (weight, body fat, BMI, blood pressure), food and water (energy eaten, water, caffeine),
  well-being (mindful minutes, iOS 18 state of mind), and the sensitive ones, off by default and left out of « Share
  everything »: blood glucose (mmol/L) and cycle (menstrual flow, period starts). A new switch starts with activity,
  sleep and heart. Stand hours, wrist temperature, BMI and moods are iPhone-only; mindfulness on Android only where
  Health Connect has it. Each category asks the system's permission for its types when turned on. Only days that are new or
  have changed are sent (the last 7 days are rechecked every time, and up to 30 days back). The iPhone also sends them
  in the background when Health has new data (`enableBackgroundDelivery`, daily). Android reads only while the app is
  open. The phone keeps only hashes of what it has sent. On the bridge, the days are kept with only the categories some
  agent reads (one no agent reads any more is wiped from them), and each agent with the switch on reads only its own
  categories (`bridge-rs/src/health.rs`, `Day::restricted`).
- **iPhone Reminders** (`App/Features/Settings/RemindersSync.swift`): a copy of the reminders goes to the bridges the
  person chose, and the requests the agents queued are carried out. This feature exists only on iOS. A reminder by
  place (`reminders_add`'s `place`) becomes an EventKit location alarm: Reminders watches the place, the app has no
  location access; a place's name is looked up with Apple Maps (`MKLocalSearch`).
- **Reminders by place on Android** (`reminders/PlaceReminders.kt`): the agents' reminders that have a place are read
  from the bridge's queue (`/v1/reminders/queue`) and kept on the phone as geofences (Google Play services location;
  the name looked up with the phone's `Geocoder`). The location permission (with background) is asked only once such a
  reminder exists; the position never leaves the phone. The other reminders stay queued for an iPhone.
- **Shared to the app** (`ShareExtension/`, `App/Model/SharedInbox.swift`, Android `ui/SharedMessage.kt`): a message or
  screenshots shared to BipAgents are handed to the agent the person picks, as a draft and attachments in its
  conversation, sent only when the person taps Send. The iOS share extension writes them to the App Group
  (`share/<id>/`, deleted when opened, or after an hour). A reply the agent drafts (a ```draft block) gets « Envoyer
  avec… », the system share sheet: the app never sends it.
- **Contacts** (`App/Features/Settings/ContactsSync.swift`, `android/.../contacts/ContactsSync.kt`): a copy of the
  address book (`PUT /v1/contacts`) goes to every bridge where at least one agent may read it, narrowed to what the
  person chose for that bridge (`PUT /v1/contacts/scope`: the whole address book or some groups, the notes only when
  ticked; no photos), and the requests the agents queued (a contact added or changed, never deleted) are carried out.
  The bridge keeps only what the choice covers and drops the copy when no agent may read it (`bridge-rs/src/contacts.rs`).
  On the iPhone the notes are never read (they need an Apple entitlement the app doesn't have).
- **An agent's e-mail address** (`GET/POST/PATCH/DELETE /v1/agents/{id}/email`,
  `POST /v1/agents/{id}/email/held/{mail}`): turning it on (a Kerfy Studio address on a place of the offer, or the person's
  own mailbox with its address, password and IMAP/SMTP servers), the allowed addresses, the daily limit, and approving
  or dropping a mail the agent wants to send to a new address (the app shows its recipients and subject). For a Kerfy
  Studio address, the place's bridge itself asks the guichet for the mailbox, with the place's backup token; the
  guichet gives the password once to the bridge, which keeps it in the agent's settings: **the phone never sees it**.
  A held mail triggers a notification (`email_held`).
- **Push registration:** the APNs or Firebase token (or a relay token instead, see §3), with the agents' ids
  (`/v1/devices`). The notification extension (`NotificationService/NotificationService.swift`) and the Android
  service (`push/BipMessagingService.kt`) then fetch the reply text, the approval command and the audio from the bridge
  (`/v1/replies`, `/v1/outbox`, `/v1/approvals`). Notification content does not come through Apple or Google.
- **Speech:** the agents' replies go to the bridge to be spoken (`/v1/tts/*`) when the phone does not speak them
  itself.
- **Site icons** before links: requested from the bridge (`/v1/favicon/<host>`), which asks DuckDuckGo's icon service.
  The phone never contacts the icon service or the site.
- **Weather and routes** (`bridge-rs/src/weather.rs`, `routes.rs`): the « Météo » switch is an agent setting
  (`PATCH /v1/agents/{id}/settings {"weather": bool}`). The bridge, not the phone, asks OpenStreetMap's Nominatim for
  the place and MET Norway for the forecast, with only the place's name and its coordinates rounded to 2 decimals. A
  route is a Google Maps directions link in the reply; the iPhone app opens it as an Apple Maps link
  (`ChatText.appleMaps`), the Android app in Google Maps. With `[maps] google_key` on the bridge, the route's places,
  mode and times go to Google's Routes API for its duration and distance; nothing is kept.
- **The agent's visible browser** (`App/Features/Agents/DesktopView.swift`): a web view of the bridge's viewer page,
  with no stored website data.
- **Pairing a place of the offer** (`App/Features/Cloud/CloudPairing.swift`, `android/.../cloud/CloudPairing.kt`):
  `GET /v1/setup?nonce=`, then `POST /v1/setup/claim` over the certificate that was proved and pinned. The claim
  carries the model, **the AI key the guichet issued for this place** (from the phone), the agents to create and the
  language. For a restore, it carries the backup key the phone kept instead.
- **Backups:** the phone reads the place's backup key and status (`/v1/backup`) and keeps the key (see §7).
- **New device on the account:** the phone asks each of its places for a new key for that device (`/v1/device-keys`),
  then seals them (§2).

The widgets make no network calls. They read what the app has already saved in the shared App Group (iOS,
`Widgets/BipWidgets.swift`) and open the app through a link (Android, `widget/AgentWidget.kt`).

## 2. Kerfy Studio's guichet (`https://guichet.bipagents.com`)

This is the all-included offer's service, with accounts and family links (`App/Features/Cloud/Guichet.swift`,
`CloudOffer.swift`, `PlaceRescue.swift`, `App/Features/Account/BipAccount.swift`, `FamilyLinks.swift`;
`android/.../cloud/Guichet.kt`, `CloudOffer.kt`, `PlaceRescue.kt`, `DeviceProof.kt`, `account/BipAccount.kt`,
`account/FamilyLinks.kt`). **The guichet's code is not in the public repository.**

| When | Call | What is sent |
|---|---|---|
| When the person opens the offer (not on the welcome screen, not in demo mode) | `GET /v1/where?country=` | the store country code (App Store storefront, Play billing config) |
| Offer screens, and choosing a model for an agent on a place of the offer | `GET /v1/models?region=` | the region only |
| Before buying | `POST /v1/offer` | plan, region; for the free week: iOS a **DeviceCheck token**, Android a **Play Integrity token + SHA-256 of `"bipagents-trial:"` + ANDROID_ID** (the raw ID is not sent) |
| Purchase, restore, plan change | `POST /v1/activate` | iOS: the App Store's **signed transaction (JWS)** + a DeviceCheck token; Android: the Play **purchase token**; region, ticket |
| AI credit bought | `POST /v1/subscription/credit` | the store proof, product id |
| Following the subscription and the places | `GET /v1/subscription`, `/v1/places/mine`, `POST …/claimed`, `…/withdraw` | the subscription token or the place's claim token |
| A Duo's or a family's shared AI credit: the buyer sets one person's monthly limit | `POST /v1/subscription/places/{number}/cap` | the subscription token, `cap_eur` (an amount, or null for none). `GET /v1/subscription` returns each place's AI use this month (`ai.people`: number, amount spent, limit) to the buyer only; `GET /v1/places/mine` returns an invited person only what is left for everyone and their own use and limit. Amounts in euros, never content |
| Outage notices | `POST /v1/subscription/phone`, `/v1/places/phone` | the **relay token** (never the APNs or Firebase token) |
| Account sign-in | `POST /v1/account/email`, `/v1/account/signin` | e-mail address (and the code received), or the **Apple or Google ID token**; device name (iOS: "iPhone"/"iPad"; Android: `Build.MODEL`, e.g. "Pixel 8"), platform, **this device's P-256 public key**, app language |
| Account | `GET/PUT /v1/account*` | relay token, device name; approve or remove devices; delete or keep the account |
| Approving a new device | `POST /v1/account/devices/{id}/approve` | a **sealed box** (below) |
| Recovery code | `PUT/POST /v1/account/recovery` | a **verifier** of the code (never the code itself) and the **escrow** (below) |
| Inviting someone to a shared place (Duo, Famille) | `POST /v1/account/invitations` | the place's number, the invitee's e-mail, the inviter's first name (`who`), the app language. Nothing about the place itself: the guichet's e-mail only says to open the app and sign in with that address |
| Invitations received (signed-in, approved account) | `GET /v1/account/invitations/received` | the account token. The answer holds the places offered to this address and still ready to pair, with what pairing needs (address, port, certificate fingerprint, pairing secret, the AI key the guichet issued, claim token), over HTTPS. Once paired, `POST /v1/account/places` with the claim binds the place to the account |
| Family links | `POST /v1/account/family/devices` (body `{email, siblings}`), `POST /v1/account/family/invitations` | the other person's e-mail (in the request body, not in the URL), the household name (`from`), one **sealed box** per device of theirs |

**What is encrypted end to end (public code):** `AccountCrypto`. A box is sealed on one device for
another device's P-256 key: an ephemeral ECDH key, HKDF-SHA256 (info `bipagents-box-v1`), then AES-256-GCM. Only the
device holding the private key can open it. Boxes carry the device handover (for each place: address, certificate
fingerprint, the new device's bridge key, backup key, claim token, and the escrow key) and family invitations. The
**escrow** (each place's backup key) is sealed with AES-256-GCM under a key derived from the 24-character recovery
code (HKDF, 120 bits). The guichet keeps only another value derived from the code.

**Comparison code for a new device** (`AccountCrypto.deviceCode`): the waiting device shows a 12-digit code from its own
public key; the device that approves it shows the code of the key the guichet handed it, in the alert and next to
« Approuver », and asks the person to approve only if both match. The code is SHA-256 over `"bipagents-device-code-v1"`
‖ 0x00 ‖ the device's id ‖ 0x00 ‖ its public key (65 bytes), hashed 10,000 times in all; the first 8 bytes, modulo
10¹², give the 12 digits. A key that is not a valid P-256 key gets no code and no « Approuver ». A reference vector is
checked in both apps (`AccountCryptoTest.kt`, and a debug-build assertion in `AccountCrypto.swift`).

**Never sent to the guichet** (no such call exists in the code): conversations, files, memory, health data, reminders,
contacts,
bridge keys in clear, the recovery code itself. The agents' mailboxes are asked for by the place's bridge, not by the
apps (§1).

## 3. Kerfy Studio's push relay (public: `relay/`)

Used only when the build sets a relay host (`BIPPushRelayHost` / `push.relayHost`) (`App/Features/Push/PushRelay.swift`,
`AppDelegate.swift`, `android/.../push/Push.kt`).

- `POST /v1/register` `{platform, token, environment}`: the APNs or Firebase token, exchanged for a **relay token**
  (the token sealed with XChaCha20-Poly1305, under a key only the relay holds; `relay/src/seal.rs`). The relay token
  goes to bridges that have no push key of their own, and to the guichet. Registration happens once notifications are
  allowed, and only when one of the bridges, the offer or the account needs it.
- `POST /v1/report` ("Report this reply"): **the text of the reported reply**, the reason, the optional comment, the
  agent's category, app version, phone model (iOS: hardware id such as `iPhone17,1`; Android: manufacturer + model),
  platform, language. There is no name, address or account. The relay forwards it to the guichet's `/v1/reports`
  (`relay/src/routes.rs`). If the relay is not set up or does not answer, the app opens an e-mail to
  support@bipagents.com with the same text instead. **This is the one case where conversation text reaches Kerfy
  Studio, and only when the person sends it.**
- Pushes themselves are sent by the bridge or the guichet, not by the app. The relay keeps only ids, the agent's name in
  the title and a fixed list of generic texts (`relay/src/payload.rs`).

## 4. Apple and Google

- **Stores:** StoreKit 2 (products, purchases, `Transaction.updates`, offer codes, `Storefront`), Play Billing
  (purchases, billing config country). The stores see the purchase. The guichet gets the proof (§2).
- **Free-week checks:** DeviceCheck (`DCDevice.generateToken`, `CloudOffer.swift`); Play Integrity (`DeviceProof.kt`,
  only if `play.cloudProject` is set).
- **Sign-in:** Sign in with Apple (scope: e-mail) (`AccountSignIn.swift`). On iOS, Google uses the authorization-code
  flow with PKCE in a web authentication session: `accounts.google.com/o/oauth2/v2/auth`, then
  `oauth2.googleapis.com/token` with scope `openid email` (`AccountView.swift`, `GoogleSignIn`). On Android, Google uses
  Credential Manager (`GetSignInWithGoogleOption`, `ui/AccountScreen.kt`). The ID token then goes to the guichet.
- **Push:** APNs registration (`registerForRemoteNotifications`) and Firebase Messaging (token, plus Firebase's own
  installation id). Payloads carry ids, not text (§3); a bridge that has its own push key sends directly.
- **Voices:** the Bips' voice packages are Apple-hosted asset packs (`BipVoices/`, `VoicePackages.swift`; iOS 26.4+)
  and Play Asset Delivery packs. They are downloaded on demand and contain no user data. Apple's on-device speech
  models may also be downloaded (`AssetInventory`).
- **Speech recognition stays on the phone by default.** iOS: `SpeechAnalyzer` (iOS 26+, language model installed),
  otherwise `SFSpeechRecognizer` with `requiresOnDeviceRecognition`. For a language the phone cannot recognise by
  itself, the app asks once, « Dictée en ligne ? », whether the voice may go to Apple's servers; until the person says
  yes, dictation is unavailable for that language and writing still works (`Packages/VoiceKit/…/SpeechRecognizerBackend.swift`,
  `App/Features/Conversation/OnlineDictation.swift`). Android: the on-device recogniser when the language is installed
  (Android 13+); if the system can download the language, it is offered once (Android asks the person); otherwise
  the same question, for Google's servers, and only a yes lets the app use the phone's default recogniser
  (`SpeechRecognizer.createSpeechRecognizer`, usually Google's, which may work online) (`voice/OnlineDictation.kt`,
  `voice/Recorder.kt`, `voice/Live.kt`). The answer is kept; the « Dictée en ligne (Apple) » / « (Google) » setting,
  off by default, changes it. This covers voice notes, quick voice notes and Live calls.
- **Dictation vocabulary:** the recogniser also gets, as hints, the names of the person's agents, of their connected
  apps and of the people in their Carnet (at most 100, cached on the phone), plus a fixed list of common English words
  and brands (`DictationVocabulary`, `DictationWords`; Android `voice/DictationVocabulary.kt`). They go only where the
  voice already goes: nowhere with on-device recognition, with the voice to Apple or Google with online dictation.
  The accurate dictation (Parakeet) takes no hints; the phone puts the person's names right in its text afterwards.
- **Android ML Kit document scanner** (Play services): the scan is done on the phone, and the PDF is attached like any
  file. Google says ML Kit may send usage and diagnostic data. The app does not change this.
- Apple's and Google's own crash and usage reports (shared with developers if the person allowed it in the system
  settings) are a system setting, not code in the app.

## 5. AI model providers

**The apps never call a model provider** (no OpenAI, Anthropic, EUrouter, OpenRouter or similar address in the app
code). Messages reach the model through the person's Hermes. What the provider sees is whatever the agent puts in its
request: the conversation, the files, and the results of its tools, including **health summaries when an agent with
"Santé" uses its `health` tool**. On the offer, the app allows turning "Santé" on only with a model the guichet marks
as `eu_hosted`, and when the agent's model is changed in the app to one that is not, it turns "Santé" off on the
bridge at once and says so (`AgentSettingsView.swift`, `AgentSettingsScreen.kt`, `health/HealthSync.kt`). The AI key of an offer place is created by the
guichet and goes through the phone to the bridge (§1).

**The Carnet's nightly sorting** (bridge, `notebook/tidy.rs`): once a night at most, the person's own bridge sends
the day's conversation passages that its free pre-filter kept (those the agents did not file and that look like they
hold dated or personal facts), with the Carnet's page names, to a small model at the agents' own provider (or
`[notebook] base_url`), under a monthly cap. Never the conversations of an agent with « Santé », never its memory.
The apps play no part in it.

## 6. Anything else

- **Links the person taps:** links in replies, the legal texts (`github.com/Alexbob0/BipAgents/blob/main/docs/legal/…`),
  Hermes Agent and the source on GitHub, CC BY 4.0, Play's subscription and redeem pages. These open in the browser or
  the store; nothing is sent except the address itself.
- **Install commands** (`raw.githubusercontent.com/…/install.sh`, `install.ps1`): shown, copied or shared. The app does
  not fetch them.
- **Pages the agents make** (`AgentPages.swift`): shown in a locked web view. Every `http(s)`, `ws(s)`, `ftp` and
  `file` load is blocked, along with navigation, pop-ups and stored data.
- **E-mail to support** (person-initiated): a diagnostic with versions, settings and each agent's state; no
  conversation, name, address or key (`Support.swift`, `ui/Support.kt`).
- **Debug-only test addresses** (a local guichet, local voice packages): the overrides (`guichetURL` on iOS and
  Android, `voicePackagesURL` on Android) are read only in debug builds, so store builds always use
  `guichet.bipagents.com`.

## 7. Kept on the phone only

- iOS Keychain, `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly` (`App/Model/Keychain.swift`, `OfferVault` in
  `Guichet.swift`): the agents' keys, the guichet tokens, the account's P-256 private key, the escrow key, and the
  places with their backup keys. These are not in iCloud Keychain or in backups that move to another device.
- Android: the same secrets in the app's private preferences, encrypted with an AES key that stays in the Android
  Keystore (`model/Secrets.kt`). `allowBackup="false"`.
- Conversation caches, voice-note recordings and downloaded voices stay in the app's own storage. The relay token cache
  and the health hashes are in UserDefaults or SharedPreferences.

## 8. Demo mode

The demo (`App/Features/Demo/`, `AgentStore.tour`, `ui/DemoViews.kt`) uses the bundled `demo.json` and recordings. Its
agents point at `https://demo.invalid`. In demo mode the app does not ask for push, health, reminder or backup syncing,
the account or the guichet's place checks (`BipAgentsApp.swift`, `HealthSync`), and conversations and Live calls play
recorded lines. The offer screens ask the guichet nothing (`OfferFlow.swift`, `OfferScreen.kt`): prices come from the
store alone (StoreKit products and storefront, Play Billing products and country, or the phone's region).

## 9. Limits and gaps, said plainly

- **The device comparison code makes key substitution costly, not impossible.** The devices' public keys still come
  from the guichet. A dishonest guichet that hands its own key for « the new device » would have to find a key whose
  code matches the real one (12 digits, 10,000 SHA-256 rounds per try), and the code only helps if the person compares
  it. A full commit-then-reveal exchange, where neither side can choose its key after seeing the other's, is planned.
- **Family invitations have no comparison code.** They are sealed for the other person's device keys as the guichet
  gives them (`POST /v1/account/family/devices`); a dishonest guichet could hand its own key and open the invitation.
- Recovery without the code (after 24 hours) lets the guichet have a place paired again with a new device. That is by
  design, and it is a power the guichet holds.
- On a place of the offer, the server is installed for Kerfy Studio and runs at a hosting provider. The app's
  encryption does not protect against whoever runs that server.
- **Invitations to a shared place:** the guichet holds the place's pairing secret, AI key and claim token (it issued
  them) and gives them, over HTTPS, to the app signed in with the invited address. Whoever can sign in with that address
  (for example by reading its e-mail codes) can take the place while it is unpaired.
- **« Partager le lien » of a place** (Duo, Famille) still puts the place's address, port, pairing secret, AI key and
  claim token, base64 and not encrypted, after the link's `#`. The person chooses where to send it; the link works once.
- **« Signaler cette réponse »** sends the reply's text to Kerfy Studio (through the relay to the guichet, or by e-mail
  if the relay does not answer), only when the person sends the report (§3).
- **Online dictation, once allowed,** sends the voice to Apple or to the phone's default recogniser (usually Google)
  for the languages the phone cannot recognise by itself (§4). On Android 12 and older, the app cannot check on-device
  support, so dictation there works only with that yes.
- **ML Kit's document scanner** on Android may send usage and diagnostic data to Google, according to Google. The app
  does not change this.
- **"Santé" and the model:** the EU-hosted check is in the apps and relies on the guichet's `eu_hosted` mark. It runs
  when the switch is turned on and when the model is changed in the app; the bridge does not check it, so a model
  changed another way does not turn "Santé" off.
- **An agent's Kerfy Studio e-mail address** keeps its mail on Kerfy Studio's mail server (a Hetzner server, apart
  from the places), stored as any mail server stores mail, not encrypted for the person: whoever runs that server can
  read it. Mails are deleted after 90 days, and the whole mailbox when the address goes. Between mail servers, mail is
  encrypted in transit only when the other server supports it.
- **The guichet's code is private.** What it keeps and does with what it receives cannot be checked from public code.
  The relay and the bridge are public.
- **The apps' code is private too:** this page and the public `AccountCrypto` files are what can be checked; the rest is
  a description of code that readers cannot see.
