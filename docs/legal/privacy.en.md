English · [Français](privacy.fr.md)

# BipAgents privacy policy

> **Draft** — to be reviewed by a lawyer before publication. Items in square brackets are to be filled in.
> Last updated: [date]. The French version is the reference version [to confirm].

## In short

- BipAgents connects your phone to **your own AI agents**, which run on your computer or server with Hermes Agent.
  Your conversations, files, voice and your agents' memory stay on your server and with the AI model provider **you**
  chose. **Kerfy Studio, the app's publisher, does not see them.**
- No account, no ads, no usage analytics, no tracking.
- Kerfy Studio can receive only three things:
  1. what it takes to deliver your **notifications** through its relay: a sealed device token, never the text of your
     messages;
  2. the **emails** you choose to send (support, reporting a reply);
  3. **crash reports**, only if you agreed to share them in your phone's settings.

## 1. Who is responsible

**Kerfy Studio**, a French SAS with a share capital of [capital] €, registered office at [registered office address],
registered with the [RCS] trade register under number [SIREN] ("Kerfy Studio", "we"). Contact:
**support@kerfystudio.com**. [Data protection officer: none appointed at this time — to confirm.]

Kerfy Studio is the data controller **only** for the data described in section 4.

## 2. How BipAgents works, and who sees what

| Where | What is there | Who decides |
|---|---|---|
| **Your server** (your computer, or a server you rent) | Hermes Agent, your agents, your conversations, your files, their memory, their scheduled tasks, the password vault | You |
| **Your model provider** (a cloud API such as OpenRouter, or a model running at home) | Each message your agents send it to be answered | You, under that provider's terms |
| **Your phone** | What the app keeps in order to work (section 3) | You |
| **Kerfy Studio** | Only what section 4 describes | Kerfy Studio |

- **BipAgents is built on Hermes Agent**, open-source software by Nous Research
  (https://github.com/NousResearch/hermes-agent), which you install on your own machine. The app connects to it
  directly, through **Tailscale** (a private network between your devices) or your **local network**. Kerfy Studio
  does not host your agents and has no access to your server. For this data, Kerfy Studio is neither a controller
  nor a processor.
- **If you rent a server**, its hosting provider technically has access to its disk. Pick one you trust (for
  instance in the European Union).
- **Your model provider** receives the messages it answers and processes them under its own terms and privacy
  policy. Some offer to keep nothing ("zero data retention"): that choice is yours. With a model running at home,
  nothing leaves your network.
- **The services you connect to your agents** (email, calendar, websites, smart home…) receive what your agents send
  them, under their own rules.
- **Several people on one server** (the family feature): each person has their own install, separate from the
  others. Whoever administers the server is responsible towards the others.

## 3. What the app keeps on your phone

This data stays on your phone and is **not** sent to Kerfy Studio:

- **Your agents**: their name, their addresses on your server, the fingerprint of your local network's certificate,
  their settings (mascot, voice, language).
- **The keys to your agents**: in the iPhone Keychain (limited to this device) or encrypted with an Android Keystore
  key.
- **A copy of your recent conversations**, to read them offline, your drafts, messages being sent, and replies that
  came in while you were away (the Inbox).
- **Your voice notes** (recordings and transcripts), the files you send and the files your agents send you.
- **The widget**, if you add it, shows an agent's latest message, including on the iPhone lock screen if you put it
  there.
- **On iPhone**, your agents' notifications are shown as messages: iOS keeps the agent's name and the displayed text
  on the device, as it does for a messaging app.
- **The agent's browser** (Desktop view): on iPhone, nothing is kept; on Android, its cookies stay in the app.

**Backups.** On iPhone, this data is part of your iCloud or computer backup if you turned it on, except the keys,
which stay on the device. On Android, the app excludes its data from backups; a direct transfer to a new phone may
copy part of it, but not the keys, which cannot be read there.

**Deleting.** Remove an agent in the app, or uninstall the app to erase everything from the phone. This does not
erase what is on your server.

**Shares you start.** The diagnostic report (Settings › Diagnostics and latency › Export report) contains your
agents' names and addresses; it only goes to the app or person you pick in the share sheet.

## 4. What Kerfy Studio may receive

### 4.1 Notifications, through Kerfy Studio's relay

> [At the time of writing, the relay is not in service yet: to be updated at launch.]

**Why a relay.** To send you a notification, your server has to go through Apple (iPhone) or Google (Android), with
keys that belong to the app's publisher and cannot be handed to every server. Kerfy Studio's relay holds them and
forwards your server's notifications.

**How.** When you allow notifications, Apple or Google gives the app a **device token**. The app sends it to the
relay, which gives it back **sealed** (encrypted with a key only the relay holds). Your server keeps this sealed token
and sends it to the relay with each notification; the relay opens it, forwards to Apple or Google, then forgets
everything.

**What the relay processes:**

- the device token, the platform (Apple or Google) and the environment (test or production);
- the **IP address** of your phone (when registering) or of your server (with each notification), only to limit
  abuse: it is turned into a fingerprint, kept in memory for one hour at most, never written down;
- the notification's content, reduced to the **agent's name** (the title), a **generic text** such as "New message"
  (any other text is replaced) and **technical identifiers** (agent, conversation, task, notification type).
  **Never the text of your messages**: the app fetches it from your server itself.

**What is kept.** The relay has no database. The sealed token is kept on your phone and on your server, not with us.
The relay's technical logs contain the platform, a 4-byte fingerprint of the token and Apple's or Google's answer,
kept for [relay log retention period]. [The relay's hosting provider may log the IP addresses of requests: nature and
duration to be specified once the provider is chosen.]

**Purposes and legal bases.** Delivering the notifications you turned on: performance of the terms of use (GDPR
article 6(1)(b)). Limiting abuse and protecting the service: Kerfy Studio's legitimate interest (article 6(1)(f)).

**Recipients.** Apple (Apple Push Notification service), Google (Firebase Cloud Messaging), and the relay's hosting
provider, located in the European Union: [relay hosting provider].

**Without notifications.** You can refuse or turn off notifications in your phone's settings: the app works without
them. If you uninstall the app, Apple or Google report the token as invalid and your server forgets it. If you build
the app yourself with your own Apple account, Kerfy Studio's relay is not used.

**On Android**, Google's Firebase Cloud Messaging library starts with the app and gets a token from Google as soon as
the app opens, even before you add an agent. [To check and, if possible, to delay until an agent is added.]

### 4.2 Emails you send us: support and reports

- **Settings › Contact support** prepares an email to support@kerfystudio.com with a diagnostic you can read before
  sending: app version, phone model and system version, language, display style, demo mode or not, notification
  status and, for each agent, its category, whether it answers, whether it has a bridge and a local-network door, and
  its voice. **No conversation, no agent name, no address, no key.**
- **Report this reply** (long press on a reply) prepares an email with the reason, your optional details, **the text
  of the reported reply**, the agent's category, the app version and the phone model. It goes to Kerfy Studio only:
  not to your server, not to the model provider.
- Nothing is sent until you tap "Send" in your email app. We also receive your email address and anything you choose
  to write: avoid putting sensitive information in it.

**Purposes.** Answering you, handling reports, fixing bugs and improving the app.
**Legal basis.** Kerfy Studio's legitimate interest in answering requests and making the app safer (GDPR
article 6(1)(f)).
**Retention.** [Proposal to validate: as long as needed to handle the request, then 3 years after the last exchange;
reports, 1 year after they were handled.]
**Recipients.** Kerfy Studio and its email provider: [email provider and hosting country].

### 4.3 Crash reports

If you agreed, in your phone's settings, to share analytics with app developers (iPhone: Settings › Privacy &
Security › Analytics & Improvements › Share With App Developers; Android: usage and diagnostics sharing), Apple or
Google may pass crash reports and technical measurements (model, system version, where in the code the app crashed)
on to Kerfy Studio. The app contains no third-party analytics or crash-reporting tool: these reports only come from
Apple or Google.

**Purpose.** Fixing bugs. **Legal basis.** Your consent, given to Apple or Google and withdrawable in the same
settings (GDPR article 6(1)(a)). **Retention.** [Retention period set by Apple and Google, to be specified.]

### 4.4 Store statistics

Apple and Google give Kerfy Studio aggregated statistics about the app (downloads, countries, versions), under their
own rules. They do not let us identify you.

## 5. What Kerfy Studio does not do

- No account, no ads, no analytics tool, no tracking across apps or websites.
- No selling or renting of data.
- No training of AI models on your data: we don't have it.
- No access to your server, your conversations or your files.

## 6. Services the app contacts directly

Besides your server, the app may contact these services directly; they apply their own rules. Kerfy Studio receives
nothing from these exchanges.

- **Website icons (DuckDuckGo).** To show the small icon of websites mentioned in replies, the app asks DuckDuckGo
  for it (icons.duckduckgo.com). DuckDuckGo receives the **website's name** (for instance "example.com") and your IP
  address, not the message.
- **Speech recognition.** Voice notes and Live use your phone's speech recognition. On iPhone, it runs on the device
  when the device supports it for the chosen language; otherwise Apple may process the audio on its servers. On
  Android, the system's recognition service (often Google's) may process the audio on its servers, especially during
  Live. It is the transcribed text that goes to your server.
- **The phone's text-to-speech.** When the phone's voice reads a text (in the demo, or when your server does not
  answer), it uses your phone's speech engine, according to its settings.
- **On Android, Google ML Kit** reads QR codes (on the device) and scans documents (through Google Play services).
  Google states that ML Kit may send it usage and performance metrics. [To check.]
- **Apple and Google** for notifications (section 4.1).
- **Tailscale**, if you use it: a separate app, with its own account and its own privacy policy.
- **Links you open** (websites your agents mention, the Hermes Agent page) open in your browser.

## 7. The demo

The built-in demo runs entirely on your phone: invented agents and data, no account, no server, and nothing is sent
to Kerfy Studio. Its podcast is an audio file recorded in advance and bundled with the app (voices: Fish Audio
Drama 3, "Built with Fish Audio"). If you talk to the demo, the phone's speech recognition applies (section 6).

## 8. Voices

The Bips' voices are synthesized **on your server**: the text to read goes to your server, not to us. If you plug in
your own voice model, it is a server you run yourself. Kerfy Studio receives nothing.

## 9. Partner links (hosting)

The app or the kerfystudio.com website may later suggest hosting providers through partner (affiliate) links. If you
follow such a link, the provider may know your visit came from BipAgents, and Kerfy Studio may receive a commission if
you sign up. The app sends no personal data to the provider; what you give the provider afterwards is governed by its
own privacy policy. [To complete depending on the contracts: data possibly received from the partner, for instance a
number of sign-ups.]

## 10. Transfers outside the European Union

The notification relay is hosted in the European Union ([relay hosting provider]). Apple and Google may process
tokens, notifications and crash reports outside the European Union, notably in the United States; they rely for this
on [the EU-US Data Privacy Framework adequacy decision and/or standard contractual clauses — to check]. Email provider:
[hosting country].

## 11. Retention periods

| Data | Retention |
|---|---|
| Device token at the relay | Not kept: it only exists sealed, on your phone and your server |
| IP address at the relay | In memory, one hour at most, as a fingerprint |
| Relay technical logs | [relay log retention period] |
| Support emails | [proposal: 3 years after the last exchange] |
| Reports | [proposal: 1 year after they were handled] |
| Crash reports | [as set by Apple and Google] |

## 12. Security

The relay only accepts encrypted connections (HTTPS), seals tokens (XChaCha20-Poly1305), has no database and never
writes a token in clear. In the app, your agents' keys are protected by the Keychain or the Keystore, and the
connection to your server goes through Tailscale or through an HTTPS door on your local network that only accepts your
server's certificate. The code of the app, the server side and the relay is public: [repository URL].

## 13. Your rights

You can ask for access to your data, its correction, its erasure, the restriction of its processing, its
portability, object to processing based on our legitimate interest, withdraw your consent at any time, and give
instructions about your data after your death (French law).

Write to **support@kerfystudio.com**. We answer within one month, which can be extended by two months for a complex
request; we may ask for what we need to check your identity.

Since we keep nothing that links a notification token to a person, we usually cannot find relay data about you: to
stop, turn off notifications or uninstall the app. You manage the data on your server yourself; for the data held by
your model provider or your hosting provider, contact them.

## 14. Complaints

If you believe your rights are not respected, you can lodge a complaint with the **CNIL** (3 place de Fontenoy,
TSA 80715, 75334 Paris Cedex 07, France — https://www.cnil.fr/fr/plaintes) or with the data protection authority of
your country.

## 15. Children

> [To validate with the lawyer: the age of 15 (article 45 of the French Data Protection Act), the stores' age
> ratings, consistency with the family feature.]

BipAgents is not directed at children. **Under 15, the app should only be used with a parent's supervision.** The
family feature lets an adult set up an install for a relative: that adult is responsible for supervising a minor's
use, choosing suitable agents and a suitable model, and checking the minimum age required by the model provider. The
app has no account, so it does not know its users' age. If a child under 15 wrote to us without a parent's consent,
contact us and we will delete the message.

## 16. Changes

We may change this policy, in particular when the relay or partner links launch. The date at the top changes each
time; a significant change is announced in the app or on kerfystudio.com [how to inform users, to be specified].

## 17. Contact

Kerfy Studio — [registered office address] — **support@kerfystudio.com**
