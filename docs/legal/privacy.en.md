English · [Français](privacy.fr.md) · [Deutsch](privacy.de.md) · [Español](privacy.es.md)

# BipAgents privacy policy

> Last updated: [date].

## In short

- BipAgents connects your phone to **your own AI agents**, built on Hermes Agent. They run either **on your computer
  or server**, or, with the **all-included offer**, on **your place**, an account on a server that Kerfy Studio rents
  from a hosting partner. Your conversations, files, voice and your agents' memory stay on that server and with the AI
  model provider that answers them. **Kerfy Studio, the app's publisher, does not read them.**
- No ads, no usage analytics, no tracking. No account with your own server; an account for the all-included offer.
- With your own server, Kerfy Studio can receive only three things:
  1. what it takes to deliver your **notifications** through its relay: a sealed device token, never the text of your
     messages;
  2. what you choose to send us: an **email** to support, or a **report** of a reply from the app;
  3. **crash reports**, only if you turned on sharing them in your phone's settings.
- With the all-included offer, Kerfy Studio also processes your account, the state of your subscription, technical
  data about your place (never its content) and your backups, encrypted with a key it does not have; it hosts your
  place without accessing it (section 5).

## 1. Who is responsible

**Kerfy Studio**, a French SAS with a share capital of [capital] €, registered office at [registered office address],
registered with the RCS of [registry city] under number [SIREN] ("Kerfy Studio", "we"). Contact:
**support@kerfystudio.com**.

Kerfy Studio has not appointed a data protection officer: its activity does not require one (GDPR article 37). For
any question about your data: support@kerfystudio.com. For Singapore's law (Personal Data Protection Act), which
requires a person in charge of data protection, this is Kerfy Studio's president, [first and last name], reachable
at the same address.

Kerfy Studio is the controller of the processing the app performs in order to work, as Kerfy Studio designed it
(sections 3, 4 and 7), even when it stays on your phone and Kerfy Studio has no access to it, and of the processing of
the all-included offer (section 5). When you use your own server, it is neither the controller nor a processor of the
data handled by that server, your agents and your model provider (section 2).

## 2. How BipAgents works, and who sees what

With your own computer or your own server:

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

**With the all-included offer**, your place takes the place of your server, and Kerfy Studio chooses the hosting
provider and the AI router: section 5 says who sees what.

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

**What is kept.** The relay has no database. The sealed token is kept on your phone and on your server; with the
all-included offer, Kerfy Studio's guichet keeps it too, still sealed (section 5.6).
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

## 5. The all-included offer

This section is about people who use the all-included offer ([terms of the offer](offer.en.md)): a place on a server
that Kerfy Studio rents from a hosting partner, an AI credit, backups and an account. The rest of this policy also
applies (the app on your phone, notifications, support). Kerfy Studio's service that manages accounts,
subscriptions and places is called the guichet.

### 5.1 Who is responsible for what

- **Kerfy Studio is the controller** of the processing the offer requires: your account, the state of your
  subscription, the hosting of your place, sending your messages to the AI model, the backups, technical monitoring
  and handling abuse.
- **The content of your place** (your agents, your conversations, your files, their memory, their routines, the
  password vault): you decide what you put there. Kerfy Studio hosts it and has it processed by the AI model only to
  provide you with the service (GDPR article 6(1)(b)), **without accessing it**: its software does not read it
  (the only orders a server carries out are to reinstall a place, update it to a published version of BipAgents' open source code, or reopen it, and none of them reads its content). Access to Kerfy Studio's accounts at the hosting providers is restricted to the company. The content of a
  place would only be accessed to meet a legal obligation (a court order, for instance), and you would be told when
  the law allows it. Like any hosting provider, OVHcloud and Hetzner have physical access to their servers. If you put
  sensitive data there (your health, for instance), it is only processed to carry out what you ask your agents to do.
- **Apple and Google** are responsible for their own processing when you sign in with them and when you pay in their
  store.
- **Your agents are AIs and say so**: the "AI" badge, a notice at the start of each Live session, a mention in
  notifications and in the messages they send to other people (Regulation (EU) 2024/1689, article 50).

### 5.2 Your account

**Data.** The identifier Apple or Google gives you and your email address (with Apple, it may be a relay address); or,
with an email address, the one-time codes sent to you. The list of your approved devices, the public key of each
(what another of your devices sends it is sealed for it) and their notification tokens, sealed by the relay (section 4.1): Kerfy Studio never sees them in clear. The
invitations you send (the invited person's email address and their status). A recovery request in progress. The copy
of your backup key, encrypted with your recovery code, which Kerfy Studio cannot read. The AI credit bought and kept
for a future subscription (an amount). The account deletion date, if you asked for it. Not your name, not your photo,
never the keys to your place.

**Purposes.** Signing you in, linking your subscription and your places, approving your devices, recovering your
account, sending you sign-in codes, security notices (new device, recovery in progress), the confirmation of your
purchase, the acknowledgement of a withdrawal and the deletion notices, handling invitations. No promotional email: a
commercial message would only be sent if you ticked a box to agree to it.
**Legal basis.** Performance of the contract (GDPR article 6(1)(b)).
**Retention.** As long as your account exists. When you delete it, it is erased **30 days** after your request (your
places stop as soon as you ask, and you can change your mind until then), with its places and their backups, except
what the law requires us to keep: as a hosting provider, Kerfy Studio keeps apart, and only to answer judicial
authorities, your email address for 5 years and your Apple or Google identifier for 1 year after the end of the
contract (French law on confidence in the digital economy, article 6, II; decree no. 2021-1362, articles 2, 3 and 8).
One-time codes: as long as they are valid. Invitation not accepted: 30 days.
**Deleting your account.** In the app, Settings › Account › Delete my account, or, if you no longer have the app, on
the page https://guichet.kerfystudio.com/account/delete with a code sent to your email address. The people you share
your plan with are told by email: their places stop on the day your account is erased.

### 5.3 Your subscription, your purchases and the free week

**Data received from the stores.** Through Apple's notifications (App Store Server Notifications) and Google's (Google
Play real-time notifications): the identifier of your subscription at the store, the plan, the region, its state (free
week, active, suspended, ended, refunded), the end of the paid period, whether the renewal is turned off, and for the
AI credit you buy, the identifier of each purchase and its amount. **Never your payment details**, which only the
store knows. Your store account's country, which the app sends to find out whether the offer is sold there (it is not
kept).

**Your contract.** The start date, the date of the confirmation email and, if you withdraw from the contract, the date
and time of your withdrawal.

**AI credit used.** The amount each place's AI key spent in the month, read from the AI router once a minute at most,
and the credit carried over: an amount, never your messages.

**Only one free week per device.**

- **On iPhone**, the app uses Apple's DeviceCheck: Apple keeps two bits per device for Kerfy Studio ("already had a
  free week"). Kerfy Studio receives no identifier of the device: the app gives it a single-use token that it passes
  on to Apple.
- **On Android**, the app sends a Google Play Integrity token (Google confirms that the app is genuine and runs on a
  real device) and a fingerprint of the device's Android identifier, computed on the phone (SHA-256), then hashed again
  by the guichet with a secret key before being stored. Kerfy Studio only keeps this keyed fingerprint, and only for
  this limit.
- **The IP address** of your phone is kept for as long as a trial place is reserved (15 minutes at most), to limit the
  number of reservations per address.

**Purposes and legal bases.** Providing and managing your subscription, your places and your credit: performance of
the contract (GDPR article 6(1)(b)). Preventing the same person from chaining free weeks: Kerfy Studio's legitimate
interest in preventing fraud and in being able to keep offering a free week (article 6(1)(f) and recital 47). Reading
the device's identifier does not require your consent, as it only serves to limit free access to the service to a
limited period (article 82 of the French Data Protection Act; CNIL, deliberation no. 2020-091, article 5). Keeping
proof of the contract: Kerfy Studio's legitimate interest in being able to defend its rights.
**Retention.** Subscription and purchase data: during the subscription, then 5 years in archive, as proof of the
contract (article 2224 of the French Civil Code). Android device fingerprint: 24 months. IP address: 15 minutes at
most. The DeviceCheck bits are kept by Apple.

### 5.4 Your place and the AI model

**Where.** On a server rented from OVHcloud (data centres in the European Union) or from Hetzner (Germany or Finland),
for everyone in the offer, including people who live outside the Union.

**What is there.** What section 5.1 describes, and the public SSH keys you add from the app. Kerfy Studio's guichet
(the service that manages the places) knows your place's address and its certificate's fingerprint, so that your phone
can connect to it and so that it can be moved in case of an outage.

**The AI model.** The messages your agents send to the model (with the files and photos they show it) go through an
AI router, **EUrouter** (EUrouter B.V., Amsterdam, Netherlands), for everyone in the offer, then through the provider
that runs the chosen model: [to be completed: providers of the models offered and the countries where they run].
EUrouter states that by default it keeps neither the requests nor the replies, and keeps their metadata (time, model,
number of tokens, cost, IP address) for up to 12 months. They act on Kerfy Studio's behalf. **Before the first
message, the app names the router and the model's maker and asks for your agreement**; the list of models shows each
one's maker.

**Sleep.** Your place itself notes the app's latest request; the guichet only knows that it is asleep.

**Legal basis.** Performance of the contract (GDPR article 6(1)(b)).
**Retention.** During the subscription. When it ends, your place is kept for 30 days (3 days after a free week that
was not continued), then its account is erased and reinstalled for someone else. The extra places of a reduced plan
are erased 30 days after they stopped.

### 5.5 Backups

Your place backs itself up every hour. The backup is **encrypted on the place**, before it leaves, with a key that only
your devices hold, plus a copy of this key encrypted with your recovery code (section 5.2). Kerfy Studio keeps these
backups at [to be completed: backup hosting provider] without being able to read them: one per hour for 48 hours, one
per day for 30 days, one per week for 12 weeks. They are erased with your place.

**OVHcloud's daily image.** On its servers, OVHcloud also makes a daily image of the whole server, which it keeps on
its premises: one restore point per day, not encrypted by Kerfy Studio. When a place is reinstalled for someone else,
the image that held it is replaced within about a day.

**Legal basis.** Performance of the contract (GDPR article 6(1)(b)).

### 5.6 Technical monitoring and outages

Every two minutes, each server sends the guichet: its memory, its disk and its load, and for each place, whether its
bridge answers, whether its Hermes Agent runs or sleeps, its BipAgents version and the time of its latest successful
backup. **Never any content.** This data is used to spot an outage, move places, roll out updates in waves and alert
Kerfy Studio.

When your place's server goes down, is moved or comes back, your phones are told through the notification relay, with
a generic text: for this, the guichet keeps your phones' sealed tokens linked to your place.

**Legal basis.** Performance of the contract (GDPR article 6(1)(b)).
**Retention.** Only the latest values are kept, replaced with each report. Event log (outages, moves, updates):
12 months. Sealed tokens: as long as the device is linked to your place.

### 5.7 Abuse reports and requests from authorities

**Data.** The report (who sends it and how to reach them, the facts, the IP address and the time), the place
concerned, our decision and its reasons, our exchanges with you.
**Purposes.** Handling reports ([acceptable use policy](acceptable-use.en.md)), protecting the servers and the other
people, answering authorities.
**Legal bases.** Legal obligation (GDPR article 6(1)(c); Regulation (EU) 2022/2065, articles 16 to 18) and Kerfy
Studio's legitimate interest in protecting its service and its customers (article 6(1)(f)).
**Retention.** 1 year after they were handled, longer if a dispute or proceedings require it.

### 5.8 The offer's service providers

| Provider | Role | Data | Where |
|---|---|---|---|
| OVHcloud (OVH SAS, Roubaix, France) | Hosts the places' servers; daily image of each server | Content of the places | European Union |
| Hetzner Online GmbH (Gunzenhausen, Germany) | Hosts some of the places' servers | Content of the places | Germany, Finland |
| EUrouter B.V. (Amsterdam, Netherlands) | AI router | Messages sent to the model, metadata | European Union |
| Model providers: [to be completed] | Run the chosen model | Messages sent to the model | [to be completed] |
| [to be completed: backup hosting provider] | Keeps the backups | Encrypted backups, unreadable for it and for Kerfy Studio | [to be completed] |
| [relay hosting provider] | Hosts the guichet and the relay | Account, subscription, technical data | European Union |
| [to be completed: email sending provider] | Sends the codes, the security notices, the invitations, the purchase confirmation, the withdrawal acknowledgement and the deletion notices | Email address, content of these emails | [to be completed] |

They are bound to Kerfy Studio by a contract that complies with GDPR article 28. Apple (DeviceCheck, App Store) and
Google (Play Integrity, Google Play) are also involved, as section 5.3 says; "Sign in with Apple" and signing in with
Google fall under their own responsibility.

## 6. What Kerfy Studio does not do

- No ads, no analytics tool, no tracking across apps or websites. No account, except for the all-included offer.
- No selling or renting of data.
- No training of AI models on your data by Kerfy Studio.
- No access to your server, to the content of your place, to your conversations or to your files.

## 7. Other services the app relies on

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

## 8. The demo

The built-in demo runs entirely on your phone: invented agents and data, no account, no server, and nothing is sent
to Kerfy Studio. You can open it from the first launch, with "Demo only". Its podcast is an audio file recorded in
advance and bundled with the app (voices: Fish Audio Drama 3, "Built with Fish Audio"). If you talk to the demo, the
phone's speech recognition applies (section 7).

## 9. Voices

The Bips' voices are synthesized **on your phone** when it can: the app downloads the voices of your language from
the App Store or Google Play, and your server (or your place) sends it the text to read, prepared. Otherwise, they are
synthesized on your server or on your place. If you plug in your own voice model, it is a server you run yourself.
Kerfy Studio receives nothing of what the voices read.

## 10. Partner links (hosting, later)

The app or the kerfystudio.com website may later suggest hosting providers through partner (affiliate) links. If so:

- these links will be marked "Partner link";
- neither the app nor the website will set any tracker for affiliation: the partner code will only be in the link's
  address;
- if you follow such a link, the provider will be able to know your visit came from BipAgents, and Kerfy Studio may
  receive a commission if you sign up;
- the app will send no personal data to the provider; what you give the provider afterwards is governed by its own
  privacy policy.

Before the first partner link, this section will state what Kerfy Studio receives from partners, and this change will
be announced as described in section 17.

## 11. Transfers outside the European Union

The notification relay is hosted in the European Union ([relay hosting provider]). To deliver notifications, Apple and
Google may process the token and the notification outside the European Union, notably in the United States:

- Google LLC (Firebase Cloud Messaging) is certified under the "EU-U.S. Data Privacy Framework", recognised as
  providing adequate protection by Commission Implementing Decision (EU) 2023/1795 of 10 July 2023;
- Apple states that it covers transfers from the European Economic Area with the standard contractual clauses adopted
  by the European Commission.

**The all-included offer.** The places' servers are in the European Union, for everyone in the offer. EUrouter
processes the messages in the European Union [to be completed: countries where the models offered run, and the
safeguard if one is outside the Union]. Apple and Google (sign-in, stores, DeviceCheck, Play Integrity) may process
data in the United States, with the safeguards stated above.

**If you live outside the European Union** (United States, United Kingdom, Israel, Singapore), your data comes to the
European Union, where Kerfy Studio processes it under the GDPR, which applies to all its processing. The United
Kingdom and Israel recognise the European Union as providing adequate protection; for Singapore, this level of
protection and our contracts with our service providers give the comparable protection that Singapore's law requires.

Email provider: [country; safeguard]. Email sending provider and backup hosting provider: [to be completed: country;
safeguard]. You can ask us for a copy of the applicable safeguards.

## 12. Retention periods

| Data | Retention |
|---|---|
| The app's data on your phone | Until you remove the agent or the app |
| Device token at the relay | Not kept: it only exists sealed, on your phone and your server (and at the guichet with the all-included offer, as long as the device is linked to your place) |
| IP address at the relay | In memory, one hour at most, as a fingerprint |
| Relay technical logs | 6 months at most |
| Support emails | 2 years after the last exchange |
| Reports | 1 year after they were handled |
| Crash reports | 1 year after receipt (at Kerfy Studio) |
| Account of the offer (identifiers, email, devices, invitations, kept credit) | As long as the account exists; erased 30 days after a deletion request |
| Invitation not accepted | 30 days |
| Email address and account identifier after the end of the contract (obligation of hosting providers) | 5 years for the email, 1 year for the identifier, kept apart |
| Subscription and purchases | During the subscription, then 5 years in archive |
| Keyed fingerprint of the Android device (free week) | 24 months |
| IP address when a free week is reserved | 15 minutes at most |
| Content of your place | During the subscription, then 30 days (3 days after a free week that was not continued) |
| Backups | 48 hours (hourly), 30 days (daily), 12 weeks (weekly); erased with the place |
| OVHcloud's daily image | One point per day, replaced within about a day after the place is reinstalled |
| Servers' technical data | Latest values only |
| The guichet's event log | 12 months |
| Metadata at EUrouter | Up to 12 months, according to EUrouter |
| Abuse reports | 1 year after they were handled |

## 13. Security

The relay only accepts encrypted connections (HTTPS), seals tokens (XChaCha20-Poly1305), has no database and never
writes a token in clear. In the app, your agents' keys are protected by the Keychain or the Keystore, and the
connection to your server goes through Tailscale or through an HTTPS door on your local network that only accepts your
server's certificate. The code of the app, the server side and the relay is public: [repository URL]. To report a
security flaw: **support@kerfystudio.com** (see the SECURITY.md file in the repository).

With the all-included offer, each place is a separate account on the server, without administrator rights; Kerfy
Studio has no SSH key on the servers, and a server only carries out orders signed by the guichet. The app connects to
your place over HTTPS and checks its certificate. Backups are encrypted on the place before they leave.

**In case of a data breach**, Kerfy Studio notifies the CNIL within 72 hours (GDPR article 33) and the people
concerned when the risk to them is high (article 34); for people in Singapore, the Personal Data Protection Commission
within 3 days when Singapore's law requires it.

## 14. Your rights

You can ask for access to your data, its correction, its erasure, the restriction of its processing and its
portability, and give instructions about your data after your death (French law).

**Right to object.** You can object at any time, on grounds relating to your particular situation, to processing
based on our legitimate interest (limiting abuse, emails, reports, crash reports, limiting free weeks).

Write to **support@kerfystudio.com**. We answer within one month, which can be extended by two months for a complex
request; we may ask for what we need to check your identity.

With your own server, since we keep nothing that links a notification token or a report to a person, we usually
cannot find this data about you: to stop notifications, turn them off or uninstall the app. You manage the data on
your server yourself; for the data held by your model provider or your hosting provider, contact them.

**With the all-included offer**, your account links this data to you: we can give you access to it, correct it or
erase it. You access the content of your place yourself, through the app or over SSH, and you can copy it at any time.
To delete your account: section 5.2.

**If you live in California or elsewhere in the United States.** Kerfy Studio does not sell or share your personal
data within the meaning of California law (California Consumer Privacy Act) and does no targeted advertising. The app
and our websites do not track you from one site to another: a "Do Not Track" signal from your browser therefore
changes nothing in what we do (California Online Privacy Protection Act). You exercise the rights in this section at
support@kerfystudio.com.

## 15. Complaints

If you believe your rights are not respected, you can lodge a complaint with the **CNIL** (3 place de Fontenoy,
TSA 80715, 75334 Paris Cedex 07, France — https://www.cnil.fr/fr/plaintes) or with the data protection authority of
your country: in the United Kingdom, the Information Commissioner's Office (https://ico.org.uk); in Israel, the
Privacy Protection Authority (https://www.gov.il/en/departments/the_privacy_protection_authority); in Singapore, the
Personal Data Protection Commission (https://www.pdpc.gov.sg).

## 16. Children

BipAgents is meant for people **aged 16 and over**. Between 16 and 18, use it with a parent's agreement and respect
your model provider's minimum age (often 18). **Under 16, the app may only be used on an install set up and
supervised by a parent or an adult in charge of the child** (the family feature): that adult chooses the agents,
their access and the model, accepts the terms of use for the child and remains responsible for their use.

Kerfy Studio does not ask for anyone's age and none of its processing relies on consent. Only Android's document
scanner asks for an agreement, for what Google receives from it (section 7): for a child under 15, the adult who set
up the install gives it together with the child. If a child wrote to us, a parent can ask us to delete the message.

**All-included offer**: it is subscribed by an adult, or with a parent's agreement. An invited person must be at least
16; under 16, a place may only be used if a parent or an adult in charge of the child has set it up and supervises it
([terms of the offer](offer.en.md), section 7). **In the United States, nobody under 13 may use a place**, and Kerfy
Studio does not knowingly collect any data from a child under 13 (Children's Online Privacy Protection Act); if that
happened, write to us and we will erase it.

## 17. Changes

We may change this policy, in particular before offering partner links. The date at the top changes each time. A
significant change is announced in the app at least 30 days before it applies.

## 18. Contact

Kerfy Studio — [registered office address] — **support@kerfystudio.com** — abuse: **abuse@kerfystudio.com**

This policy, the terms of use, the [terms of the all-included offer](offer.en.md), the
[acceptable use policy](acceptable-use.en.md) and the mentions légales (legal notice) are also in the app: Settings ›
About.
