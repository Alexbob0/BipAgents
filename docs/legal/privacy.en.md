English · [Français](privacy.fr.md) · [Deutsch](privacy.de.md) · [Español](privacy.es.md)

# BipAgents privacy policy

> Last updated: [date].

## In short

- BipAgents connects your phone to **your own AI agents**, which run on your computer or server with Hermes Agent.
  Your conversations, files, voice and your agents' memory stay on your server and with the AI model provider **you**
  chose. **Kerfy Studio, the app's publisher, does not see them.**
- No account, no ads, no usage analytics, no tracking.
- Kerfy Studio can receive only three things:
  1. what it takes to deliver your **notifications** through its relay: a sealed device token, never the text of your
     messages;
  2. what you choose to send us: an **email** to support, or a **report** of a reply from the app;
  3. **crash reports**, only if you turned on sharing them in your phone's settings.

## 1. Who is responsible

**Kerfy Studio**, a French SAS with a share capital of [capital] €, registered office at [registered office address],
registered with the RCS of [registry city] under number [SIREN] ("Kerfy Studio", "we"). Contact:
**support@kerfystudio.com**.

Kerfy Studio has not appointed a data protection officer: its activity does not require one (GDPR article 37). For
any question about your data: support@kerfystudio.com.

Kerfy Studio is the controller of the processing the app performs in order to work, as Kerfy Studio designed it
(sections 3, 4 and 6), even when it stays on your phone and Kerfy Studio has no access to it. It is neither the
controller nor a processor of the data handled by your server, your agents and your model provider (section 2).

## 2. How BipAgents works, and who sees what

| Where | What is there | Who decides |
|---|---|---|
| **Your server** (your computer, or a server you rent) | Hermes Agent, your agents, your conversations, your files, their memory, their scheduled tasks, the password vault | You |
| **Your model provider** (a cloud API such as OpenRouter, or a model running at home) | Each message your agents send it to be answered | You, under that provider's terms |
| **Your phone** | What the app keeps in order to work (section 3) | You |
| **Kerfy Studio** | Only what section 4 describes | Kerfy Studio |

- **BipAgents is built on Hermes Agent, open-source software by Nous Research**
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

This data stays on your phone and is **not** sent to Kerfy Studio. What is said here about the iPhone also applies to
the iPad.

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

**Why.** This processing only serves to provide the features you use (GDPR article 6(1)(b)). This data stays on your
phone until you remove the agent or the app.

**Backups.** On iPhone, this data is part of your iCloud or computer backup if you turned it on, except the keys,
which stay on the device. On Android, the app excludes its data from backups; a direct transfer to a new phone may
copy part of it, but not the keys, which cannot be read there.

**Deleting.** Remove an agent in the app, or uninstall the app to erase everything from the phone. This does not
erase what is on your server.

**Shares you start.** The diagnostic report (Settings › Diagnostics and latency › Export report) contains your
agents' names and addresses; it only goes to the app or person you pick in the share sheet.

## 4. What Kerfy Studio may receive

### 4.1 Notifications, through Kerfy Studio's relay

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
The relay's technical logs contain the platform, a 4-byte fingerprint of the token and Apple's or Google's answer;
they are kept for **6 months at most**. The relay's hosting provider may log the IP addresses of requests: [what the
relay's hosting provider logs, and for how long].

**Purposes and legal bases.** Delivering the notifications you turned on: performance of the terms of use (GDPR
article 6(1)(b)). Limiting abuse and protecting the service: Kerfy Studio's legitimate interest (article 6(1)(f)).

**Processors.** [relay hosting provider] (European Union) and Google (Firebase Cloud Messaging), bound to Kerfy Studio
by a contract that complies with GDPR article 28. Apple delivers notifications to iPhones through its APNs service.

**Without notifications.** You can refuse or turn off notifications in your phone's settings: the app works without
them. If you uninstall the app, Apple or Google report the token as invalid and your server forgets it. If you build
the app yourself with your own Apple account, Kerfy Studio's relay is not used.

**On Android**, the app only asks Google for a token when you turn notifications on.

### 4.2 Support and reports

- **Settings › Contact support** prepares an email to support@kerfystudio.com with a diagnostic you can read before
  sending: app version, phone model and system version, language, display style, demo mode or not, notification
  status and, for each agent, its category, whether it answers, whether it has a bridge and a local-network door, and
  its voice. **No conversation, no agent name, no address, no key.** Nothing is sent until you tap "Send" in your
  email app. We also receive your email address and anything you choose to write: avoid putting sensitive information
  in it.
- **Report this reply** (long press on a reply) sends the report **from the app**, through Kerfy Studio's relay, to
  Kerfy Studio only: not to your server, not to the model provider. It contains the reason, your optional details,
  **the text of the reported reply**, the agent's category, the app version and the phone model. **Not your email
  address, not your name, not the agent's name.** Nothing is sent until you tap "Send". On the way, the relay sees
  your phone's IP address and only uses it to limit abuse (section 4.1). Since the report contains no way to reach
  you, we cannot answer it: write to support if you expect an answer.
- **If the relay does not answer**, the app offers to send the same report by email: we then also receive your email
  address.

The text of the reported reply may be about your health or about other people: read it again before sending, and
only write what is useful in the details. We delete whatever is not needed to handle the report.

**Purposes.** Answering you, handling reports, fixing bugs and improving the app.
**Legal bases.** Answering your messages and handling your reports: Kerfy Studio's legitimate interest in answering
you and making the app safer (GDPR article 6(1)(f)). Answering a request to exercise your rights: legal obligation
(article 6(1)(c); GDPR articles 12 to 22).
**Retention.** Support: 2 years after the last exchange. Reports: 1 year after they were handled.
**Recipients.** Kerfy Studio and its processors: its email provider ([email provider], [country]) and the relay's
hosting provider, through which reports travel.

### 4.3 Crash reports

If you turned on, in your phone's settings, sharing analytics with app developers (iPhone: Settings › Privacy &
Security › Analytics & Improvements › Share With App Developers; Android: usage and diagnostics sharing), Apple or
Google may pass crash reports and technical measurements (model, system version, where in the code the app crashed)
on to Kerfy Studio. The app contains no third-party analytics or crash-reporting tool: these reports only come from
Apple or Google.

**Purpose.** Fixing bugs.
**Legal basis.** Kerfy Studio's legitimate interest in fixing bugs (GDPR article 6(1)(f)). These reports only reach
us if you turned on this sharing in your phone's settings, where you can turn it off at any time. Apple and Google
collect them under their own responsibility.
**Retention.** Kerfy Studio deletes the reports it downloads 1 year after receiving them at the latest.

### 4.4 Store statistics

Apple and Google give Kerfy Studio statistics about the app (downloads, countries, versions). Apple and Google produce
them under their own responsibility and give them to us aggregated: they do not let us identify you.

## 5. What Kerfy Studio does not do

- No account, no ads, no analytics tool, no tracking across apps or websites.
- No selling or renting of data.
- No training of AI models on your data: we don't have it.
- No access to your server, your conversations or your files.

## 6. Other services the app relies on

Besides your server, the app relies on these services, which apply their own rules. Kerfy Studio receives nothing
from these exchanges. It chose to have the app use them and is answerable for what leaves your phone; each service is
then responsible for what it does with it.

- **Website icons.** To show the small icon of websites mentioned in replies, the app asks your server for these
  icons, and your server fetches them from DuckDuckGo (icons.duckduckgo.com): DuckDuckGo receives the **website's
  name** (for instance "example.com") and sees your server's address, not your phone's. It does not receive the
  message.
- **Speech recognition.** Voice notes and Live use your phone's speech recognition. The app asks for on-device
  recognition when your phone offers it for your language. Otherwise, Apple or Google (on Android, the system's
  recognition service, often Google's) process the audio on their servers, under their own responsibility. It is the
  transcribed text that goes to your server. Legal basis: providing the feature you use (GDPR article 6(1)(b)).
- **The phone's text-to-speech.** When the phone's voice reads a text (in the demo, or when your server does not
  answer), it uses your phone's speech engine, according to its settings.
- **On Android, QR codes** are read on the phone, with a library built into the app, without any Google service.
- **On Android, the document scanner** is a Google Play services feature (Google ML Kit). Google states that ML Kit
  sends it information about the device and the app, installation identifiers and usage and performance metrics, for
  diagnostics and its own statistics. The app tells you so and asks for your agreement before the scanner's first
  use; without that agreement, the scanner does not open, and you can still attach a photo or a file. You can change
  your mind at any time in Settings.
- **Apple and Google** for notifications (section 4.1).
- **Tailscale**, if you use it: a separate app, with its own account and its own privacy policy.
- **Links you open** (websites your agents mention, the Hermes Agent page) open in your browser.

## 7. The demo

The built-in demo runs entirely on your phone: invented agents and data, no account, no server, and nothing is sent
to Kerfy Studio. You can open it from the first launch, with "Demo only". Its podcast is an audio file recorded in
advance and bundled with the app (voices: Fish Audio Drama 3, "Built with Fish Audio"). If you talk to the demo, the
phone's speech recognition applies (section 6).

## 8. Voices

The Bips' voices are synthesized **on your server**: the text to read goes to your server, not to us. If you plug in
your own voice model, it is a server you run yourself. Kerfy Studio receives nothing.

## 9. Partner links (hosting, later)

The app or the kerfystudio.com website may later suggest hosting providers through partner (affiliate) links. If so:

- these links will be marked "Partner link";
- neither the app nor the website will set any tracker for affiliation: the partner code will only be in the link's
  address;
- if you follow such a link, the provider will be able to know your visit came from BipAgents, and Kerfy Studio may
  receive a commission if you sign up;
- the app will send no personal data to the provider; what you give the provider afterwards is governed by its own
  privacy policy.

Before the first partner link, this section will state what Kerfy Studio receives from partners, and this change will
be announced as described in section 16.

## 10. Transfers outside the European Union

The notification relay is hosted in the European Union ([relay hosting provider]). To deliver notifications, Apple and
Google may process the token and the notification outside the European Union, notably in the United States:

- Google LLC (Firebase Cloud Messaging) is certified under the "EU-U.S. Data Privacy Framework", recognised as
  providing adequate protection by Commission Implementing Decision (EU) 2023/1795 of 10 July 2023;
- Apple states that it covers transfers from the European Economic Area with the standard contractual clauses adopted
  by the European Commission.

Email provider: [country; safeguard]. You can ask us for a copy of the applicable safeguards.

## 11. Retention periods

| Data | Retention |
|---|---|
| The app's data on your phone | Until you remove the agent or the app |
| Device token at the relay | Not kept: it only exists sealed, on your phone and your server |
| IP address at the relay | In memory, one hour at most, as a fingerprint |
| Relay technical logs | 6 months at most |
| Support emails | 2 years after the last exchange |
| Reports | 1 year after they were handled |
| Crash reports | 1 year after receipt (at Kerfy Studio) |

## 12. Security

The relay only accepts encrypted connections (HTTPS), seals tokens (XChaCha20-Poly1305), has no database and never
writes a token in clear. In the app, your agents' keys are protected by the Keychain or the Keystore, and the
connection to your server goes through Tailscale or through an HTTPS door on your local network that only accepts your
server's certificate. The code of the app, the server side and the relay is public: [repository URL]. To report a
security flaw: **support@kerfystudio.com** (see the SECURITY.md file in the repository).

## 13. Your rights

You can ask for access to your data, its correction, its erasure, the restriction of its processing and its
portability, and give instructions about your data after your death (French law).

**Right to object.** You can object at any time, on grounds relating to your particular situation, to processing
based on our legitimate interest (limiting abuse, emails, reports, crash reports).

Write to **support@kerfystudio.com**. We answer within one month, which can be extended by two months for a complex
request; we may ask for what we need to check your identity.

Since we keep nothing that links a notification token or a report to a person, we usually cannot find this data
about you: to stop notifications, turn them off or uninstall the app. You manage the data on your server yourself;
for the data held by your model provider or your hosting provider, contact them.

## 14. Complaints

If you believe your rights are not respected, you can lodge a complaint with the **CNIL** (3 place de Fontenoy,
TSA 80715, 75334 Paris Cedex 07, France — https://www.cnil.fr/fr/plaintes) or with the data protection authority of
your country.

## 15. Children

BipAgents is meant for people **aged 16 and over**. Between 16 and 18, use it with a parent's agreement and respect
your model provider's minimum age (often 18). **Under 16, the app may only be used on an install set up and
supervised by a parent or an adult in charge of the child** (the family feature): that adult chooses the agents,
their access and the model, accepts the terms of use for the child and remains responsible for their use.

Kerfy Studio does not ask for anyone's age and none of its processing relies on consent. Only Android's document
scanner asks for an agreement, for what Google receives from it (section 6): for a child under 15, the adult who set
up the install gives it together with the child. If a child wrote to us, a parent can ask us to delete the message.

## 16. Changes

We may change this policy, in particular before offering partner links. The date at the top changes each time. A
significant change is announced in the app at least 30 days before it applies.

## 17. Contact

Kerfy Studio — [registered office address] — **support@kerfystudio.com**

This policy, the terms of use and the mentions légales (legal notice) are also in the app: Settings › About.
