[🇬🇧 English](what-the-bridge-adds.md) · [🇫🇷 Français](what-the-bridge-adds.fr.md) · 🇩🇪 Deutsch · [🇪🇸 Español](what-the-bridge-adds.es.md) · [🇳🇱 Nederlands](what-the-bridge-adds.nl.md)

# Was die Bridge zu Hermes Agent hinzufügt

BipAgents ist **auf [Hermes Agent](https://github.com/NousResearch/hermes-agent)** von Nous Research **aufgebaut**.
Hermes übernimmt das Denken: die Modellschleife, Werkzeuge, Gedächtnis, Skills, Routinen, Freigaben, seine Gateways
und seine Runs-API. Hermes ist für ein Terminal und Chat-Plattformen gemacht, und jeder Hermes-Client (die CLI, die
offizielle Desktop-App, das Telegram-Gateway, andere Apps) zeigt, was Hermes tut. BipAgents will etwas anderes:
**mehrere Agenten pro Person, bedient vom Handy aus von Menschen, die nie ein Terminal öffnen, sicher genug für ihre
Mails, ihren Kalender und ihre Karten, und günstig genug für ein Monatsabo.** Dafür braucht es eine Schicht zwischen
den Apps und Hermes: die **Bridge** ([`bridge-rs/`](../bridge-rs/), ein einziges Rust-Programm, `bipbridge`). Diese
Seite zeigt Bereich für Bereich, was sie hinzufügt, mit den Modulen (`bridge-rs/src/<name>.rs`), die es tun, und was
sie bewusst Hermes überlässt.

## Auf einen Blick

| Bereich | Hermes allein | Hermes + andere Clients | BipAgents-Bridge |
|---|---|---|---|
| Mit ihm sprechen | Terminal, Chat-Gateways, Runs-API | ein Chatfenster (Desktop-App, Telegram…) | Apps für iPhone, iPad und Android, Sprachnachrichten, freihändige Live-Anrufe, eine Web-Konsole |
| Stimmen | keine | keine oder die der Plattform | eine Stimme pro Agent, Satz für Satz während die Antwort läuft, Text für die Sprache in 5 Sprachen aufbereitet |
| Außerhalb der App | die Nachrichten des Gateways | die Benachrichtigungen der Plattform | Push, dessen Text von deinem Server kommt, Freigaben und Fragen vom Sperrbildschirm, Live Activities |
| Mehrere Agenten | Profile, von Hand eingerichtet | meist ein Agent auf einmal | Agenten in 2 s aus der App erstellt, jeder mit eigener Sandbox, eigenem Browser, eigenen Werkzeugen, eigener Stimme und Gedächtnissprache |
| Surfen | ein Browser-Werkzeug | dasselbe | das echte Chrome jedes Agenten, live zu sehen und vom Handy aus zu übernehmen |
| Sicherheit | Freigaben, `approvals.deny`, Docker-Sandbox | dasselbe | Freigaben immer gefragt und klar formuliert, Sandboxen ohne deine Ordner, Senden an unbekannte Seiten gedeckelt, Agenten untereinander begrenzt, Tresor nur zum Schreiben, Schleifenbremse |
| Gedächtnis | Gedächtnisdateien, Skills | dasselbe | ein lesbares Notizbuch, Gedächtnis-Editor, gemeinsames Gedächtnis, Gruppengespräche, Familienagenten |
| Deine Daten | was Werkzeuge erreichen | dasselbe | Postfächer, Kalender, Kontakte, Erinnerungen, Gesundheit, Notiz- und WhatsApp-Importe, Konnektoren mit OAuth |
| Ergebnisse | Text und Dateien | Text und Dateien | gesperrte Offline-Seiten, Karten, Podcasts, Dateien, die ankommen, wo auch immer der Agent sie geschrieben hat |
| Selbstständige Arbeit | Cron-Jobs | dasselbe | Hintergrundaufgaben mit Budget, Wächter, die Antwort jeder Routine in der App, eine Liste von allem, was von selbst läuft |
| Kosten | was das Modell kostet | dasselbe | Token-Diät, fester Anbieter für den Cache, Verbrauch pro Aufruf, Guthaben, Budgets, Ruhezustand |
| Installieren und behalten | Installation, dann Konfiguration von Hand | dasselbe | ein Befehl, Updates, Backups, Wiederherstellung, Umzug zwischen Servern |
| Gehostetes Angebot | — | — | ein Platz pro Person auf den Servern von Kerfy Studio: eigenes Zertifikat, eine Tür auf Port 443, Guthaben und Backups |

## Das Handy zuerst

- **Stimme.** `live_calls.rs` führt die Live-Gespräche;
  `textproc.rs` zerlegt Markdown in Sätze; `tts.rs` plant die Engines (Pocket TTS auf der CPU oder der
  eigene Stimmserver der Person); `voices.rs` gibt jedem Agenten seine Stimme. Der Text wird für die Sprache
  aufbereitet: `numbers_fr.rs`, `speech_intl.rs` (Englisch, Spanisch, Deutsch, Niederländisch), `durations.rs`,
  `pronounce.rs`, `pronunciations.rs`.
- **Live-Anrufe.** `live_brief.rs` beginnt einen Anruf mit einer kurzen Zusammenfassung des Gesprächs statt mit 100k
  Token Verlauf; `live_calls.rs` schreibt zurück ins Gespräch, was der Anruf getan hat.
- **Benachrichtigungen.** `push.rs` (APNs, `apns.rs`; Android, `fcm.rs`) schickt einen allgemeinen Hinweis, und das
  Handy holt den Text bei der Bridge ab: Apple und Google sehen ihn nie; `relay.rs` und das separate
  [`relay/`](../relay/) leiten Pushes ohne Inhalt weiter. `runs.rs` macht aus einer Freigabe, die niemand ansieht,
  eine Benachrichtigung; `agent_notify.rs` regelt das pro Agent; `live_activities.rs` zeigt lange Arbeit auf dem
  Sperrbildschirm und in der Dynamic Island.
- **Nachrichten während der Arbeit.** `msg_queue.rs`: Eine Nachricht, die während der Arbeit des Agenten kommt, geht in
  den laufenden Durchgang (Hermes' Steer, nach den aktuellen Werkzeugaufrufen); « Danach » legt sie stattdessen in die
  Warteschlange des Agenten auf der Brücke, auf allen Geräten gleich, als nächster Durchgang gestartet (Reihenfolge,
  Entfernen, Jetzt senden, Pause), eine einzige Benachrichtigung am Ende.
- **Nichts geht verloren.** `outbox.rs` und `cronwatch.rs` legen Agentennachrichten und die Antwort jeder Routine in
  einen Postausgang (`store.rs`), mit fertigem Audio.
- **Nach Hause kommen.** Über Tailscale oder `lan.rs`: eine gepinnte HTTPS-Tür im lokalen Netz, wenn das Internet weg
  ist. `device_keys.rs` gibt jedem Gerät einen eigenen Schlüssel; `guard.rs` zählt falsche Schlüssel; `console.rs` ist
  die Web-Konsole.
- **Einfache Worte.** `approval_text.rs`, `approval_command.rs`, `no_reply.rs`, `names.rs`, `answer_style.rs` und
  `welcome.rs` machen aus den technischen Ereignissen von Hermes kurze Sätze in der Sprache der Person.

## Mehrere Agenten pro Person

- `agents_admin.rs` erstellt, ändert und entfernt Agenten aus der App; `agent_pool.rs` hält Reserveprofile bereit,
  damit ein neuer Agent in etwa 2 Sekunden fertig ist; `souls.rs` gibt jeder Art von Agent ihre Anfangspersönlichkeit;
  `guide.rs` und `guide_team.rs` schlagen die ersten Agenten oder ein ganzes Team in einer Karte vor;
  `agent_groups.rs` sortiert sie (Privat, Beruf…).
- `spaces.rs` zieht einen Agenten in ein eigenes Hermes um, wenn er Geheimnisse halten muss; `own_space.rs` macht das
  beim ersten Geheimnis von selbst. `sandbox.rs` gibt jedem Agenten eine isolierte Sandbox (kein Ordner des Rechners,
  nur Paketquellen).
- `browsers.rs` startet das echte Chrome jedes Agenten in einem eigenen Container; `browser_mcp.rs` startet seine
  Werkzeuge bei Bedarf; `desktop.rs`, `desktop_page.rs` und `desktop_read.rs` zeigen es in der App und lassen die
  Person übernehmen.
- Zwischen Agenten: `peers.rs` (nur über Verbindungen, die die Person eingeschaltet hat), `agent_messages.rs`,
  `agent_pairs.rs` (ein eigenes Gespräch « A ⇄ B »), `agent_files.rs` (Dateien, die Hermes' `message_agent` nicht
  tragen kann), `shares.rs` (Daten nur zum Lesen).

## Sicherheit

- **Freigaben.** Die Bridge lässt Hermes' Freigaben im manuellen Modus (`sandbox.rs`), formuliert sie klar, behält
  beantwortete im Gespräch (`approvals_log.rs`) und folgt dem Anfrageprotokoll von Hermes 0.22.5
  (`hermes_prompts.rs`).
- **Prompt-Injection.** Die Persönlichkeit jedes Agenten sagt, dass Gelesenes (Seiten, Mails, Dateien, andere
  Agenten) Information ist, nie ein Befehl (`souls.rs`). Andere Agenten erreichen nur einen « Bot Chat »
  (`peers.rs`). Gast-Runden bekommen geschlossene Werkzeuge (Patch 0018).
- **Was hinausgeht.** `sandbox_sites.rs` und `send_guard.rs`: Lesen im Web ist frei, aber das Senden von Daten an
  Seiten, die die Person nicht bestätigt hat, ist gedeckelt (64 KB pro Seite und Stunde) und wird dann erfragt;
  `heavy_work.rs` lässt Agenten vor großen Installationen oder Builds auf geteilten Servern fragen; `body_limit.rs`
  begrenzt Anfragen; `beneath.rs` hält den Dateizugriff einer Sandbox in ihrem Ordner.
- **Tresor.** `vault.rs` speichert Zugangsdaten verschlüsselt, wo kein Agent sie lesen kann; Hermes füllt Logins aus,
  ohne dass das Modell das Passwort sieht. `vault_logins.rs` legt Konten mit Passwörtern an, die der Agent nie sieht;
  `vault_contacts.rs` speichert Adressen; `secrets.rs` hält die Geheimnisse eines Konnektors.
- **Zahlungen.** `cards.rs`, `card_pay.rs`, `card_fill.rs`: gedeckelte virtuelle Karten, jede Zahlung freigegeben;
  `handover.rs`: « der Agent bereitet vor, du bezahlst »; `link_wallet.rs`: Stripes Link-Wallet in den USA und
  Kanada.
- **Bremsen.** `loop_brake.rs` stoppt eine Runde, die sich im Kreis dreht (ein Modell machte einmal 267 Aufrufe in
  einer Runde); `agent_restart.rs` startet das Hermes eines Agenten nur mit Grund neu.

## Gedächtnis, Notizbuch, Räume und Familie

- `notebook/` ist das Notizbuch: Personen, Themen, Aufgaben, Unterlagen und Reisen, von denen die Person ihren
  Agenten erzählt hat, geordnet und lesbar; `archives/` indexiert große Dokumentenstapel; `message_search.rs` sucht in
  allen Gesprächen.
- `memory_edit.rs` zeigt und bearbeitet Hermes' Gedächtnis in der App; `shared_memory.rs` fügt ein gemeinsames « was
  meine Agenten über mich wissen » hinzu; `memory_language.rs` hält das Gedächtnis in der Sprache des Agenten;
  `own_skills.rs` listet Skills, die der Agent selbst geschrieben hat.
- `rooms.rs`, `rooms_handoff.rs`, `rooms_memory.rs`, `rooms_remote.rs`: Gruppengespräche zwischen Agenten, auch mit
  dem Agenten eines Angehörigen.
- `households.rs` (mit `door_tls.rs`) verbindet zwei Installationen bewusst; `family_agent.rs`, `family_news.rs`,
  `agent_shares.rs`, `meet.rs`: ein gemeinsamer Familienagent, Neuigkeiten erst nach Zustimmung weitergegeben, geteilte
  Agenten, zwei Agenten, die einen Termin finden. `read_shares.rs`, `project_agent.rs`, `friends_cards.rs` (mit `shared_expenses.rs`) und
  `templates.rs` (mit `templates_scrub.rs`) teilen ein Gespräch zum Lesen, den Agenten eines Anlasses oder einen
  Agenten als Vorlage.

## Konnektoren und deine Daten

- Mail: `mailbox.rs` (jedes IMAP-Postfach), `mail_scan.rs`, `mail_tidy.rs`; eine eigene Adresse für einen Agenten:
  `agent_email.rs`, `agent_email_smtp.rs`.
- Handydaten, nur mit ausgewählten Agenten geteilt: `calendar.rs`, `caldav.rs`, `contacts.rs`, `reminders.rs`,
  `health.rs`, `phone_asks.rs`, `phone_rights.rs`.
- Importe: `notes_import.rs`, `whatsapp_import/`, `assistant_import.rs` (ein Agent aus einem anderen Assistenten).
- Dienste: `services.rs` (MCP-Server aus dem Katalog der App), `connections.rs` (Anmeldung mit OAuth 2.1, kein
  Schlüssel zum Einfügen), `connectors.rs` und `connectors_run.rs` (Konnektoren, die ein Agent schreibt, ausgeführt
  in seiner Sandbox), `watchers.rs`.
- Werkzeuge, die jeder Agent über MCP bekommt (`agent_tools.rs`, `mcp_sessions.rs`): Audio und Podcasts,
  `weather.rs`, `routes.rs`, Karten.

## Dateien, Seiten und Karten

- `files.rs` legt ab, was die Person schickt (Teilen-Menü inklusive), wo die Sandbox des Agenten es lesen kann;
  `files_kept.rs` bestimmt, wie lange. `media.rs`, `sent_files.rs`, `give_file.rs`, `made_files.rs` und
  `cited_files.rs` sorgen dafür, dass eine Datei, die der Agent macht oder nennt, wirklich bei der Person ankommt.
- `pages.rs`: Dashboards, Tracker und kleine Werkzeuge, die der Agent als eine HTML-Datei schreibt, vor dem Anzeigen
  durch eine Content-Security-Policy gesperrt (kein Netz, keine Formulare).
- `maps.rs`, `map_tiles.rs`, `map_route.rs`: OpenStreetMap-Karten über die Bridge, interaktive Karten, echte Routen
  über die Straßen; `favicons.rs` holt Seiten-Icons, damit das Handy es nicht muss.

## Selbstständige Arbeit

- `jobs.rs`: Hintergrundaufgaben mit vorher gewähltem Budget, danach angezeigten Token, Pause und Fortsetzen;
  `handback.rs` bringt den Bericht in das Gespräch zurück, aus dem er kam.
- `cronwatch.rs` liefert die Antwort jeder Routine in die App; `autonomous.rs` listet alles, was Agenten unaufgefordert
  tun; `storage_watch.rs` lässt Agenten fragen, was sie behalten sollen; `space_tidy.rs` löscht von selbst, was in ihrem
  Bereich nur vorübergehend ist (alte Entwürfe, der Ordner einer beendeten Aufgabe, alte Downloads, Caches), nie die
  Dateien der Person; `feature_tips.rs` zeigt Funktionen nach und nach.

## Kostenkontrolle

- `tool_diet.rs` legt selten genutzte Werkzeuge je nach Agentenart hinter Hermes' Werkzeugsuche (gemessen: etwa 19k
  Eingabe-Token pro Aufruf, davon 11,8k Werkzeugschemata).
- `model_route.rs` legt bei EUrouter einen Anbieter fest, damit dessen Prompt-Cache greift (Eingabe aus dem Cache zu
  etwa 17,5 % des Preises); `model_choice.rs` lässt Selbsthoster ein Modell pro Agent wählen.
- Patch 0020 meldet den Verbrauch jedes Modellaufrufs, mit dem `jobs.rs` und `runs.rs` eine Aufgabe stoppen, bevor sie
  ihr Budget überschreitet. `credit.rs` pausiert, was von selbst läuft, wenn das Monatsguthaben aufgebraucht ist, nie
  die Gespräche.
- `sleep.rs` stoppt Hermes auf einem Platz, den seit Tagen niemand nutzt, und weckt es bei der nächsten Anfrage.

## Installation, Backups und Umzüge

- Der Installer ([`installer/`](../installer/)) richtet Hermes, die Bridge, Stimmen und Sandboxen mit einem Befehl
  ein und hält sie aktuell. `place_backup.rs` zeigt Backups und ihren Schlüssel; `place_import.rs` holt dein eigenes
  Hermes herein; `place_move.rs` zieht einen Platz auf einen anderen Server um; `ssh_keys.rs` lässt die Person sich in
  ihr eigenes Konto einloggen.
- `space.rs`: der Speicher eines Platzes, im Ruhezustand verschlüsselt (standardmäßig aus).

## Das Rundum-Angebot

Auf den Servern von Kerfy Studio bekommt jede Person einen **Platz**: eigenes Konto, eigenes Hermes, eigene Bridge.
`cloud_setup.rs` richtet einen neuen Platz auf einem vorbereiteten Server ein; `place_cert.rs` besorgt ihm ein echtes
Let's-Encrypt-Zertifikat unter `<label>.p.bipagents.com`; `proxy_header.rs` lässt jeden Platz hinter einem SNI-Router
auf Port 443 antworten (viele Netze sperren andere Ports); `credit.rs` und `beta_measures.rs` sprechen mit dem
Guichet, dem Konto- und Abrechnungsdienst von Kerfy Studio (ein privates Repository, nicht Teil dieses hier).

## Die Hermes-Patches, die wir mitführen

Einige Änderungen, die die Apps brauchen, sind noch nicht in Hermes. Sie liegen in
[`hermes/patches/`](../hermes/patches/README.md) auf der festgelegten stabilen Version (`BASE`: v0.22.5), und der
Installer wendet sie an.

| Patch | Warum |
|---|---|
| 0005 Tresor füllt nach Browser-Neustart | Logins funktionieren weiter, nachdem der Browser des Agenten geschlossen war |
| 0006 synchrone Antwort eines Peers | ein von einem anderen gefragter Agent antwortet im Gespräch des Fragenden |
| 0007 `browser.vault_only` | Agenten mit eigenem sichtbarem Browser behalten den Tresor ohne Hermes' Browser |
| 0008 Zeitzone, Routinen, Uhrzeit | Routinen laufen zur Uhrzeit zu Hause, einmal pro Uhrzeit |
| 0009 Werkzeugsuche `!name`, kompakte Skill-Kategorien | die Token-Diät: manche Werkzeuge bleiben direkt, seltene Skills nur mit Namen |
| 0010 leere Antworten | Werkzeugaufrufe, die ein Modell als Text schreibt, werden echte Aufrufe; Wiederholungen ändern die Anfrage |
| 0011 Titel mit GLM | GLM antwortet auf Aufrufe ohne Denken leer, also fragen Titel nach « low » |
| 0013 MCP-Ergebnisse: Tresor-Passwörter entfernt | ein Passwort kommt nie über ein Werkzeugergebnis zum Modell zurück |
| 0015 Zeitanker | die aktuelle Uhrzeit reist mit jeder Nachricht |
| 0016 ein `message_agent`, der der Bridge | jede Nachricht zwischen Agenten geht durch die Prüfungen der Bridge |
| 0017 Adresseinträge | Tresor-Adressen tragen Name, Telefon und E-Mail |
| 0018 Gast-Runden | die Runde eines Angehörigen bekommt nur die Werkzeuge des Gastes und keine USER.md |
| 0019 Routinen berichten in ihr Gespräch | eine Routine, die in einem Gespräch entstand, antwortet dort |
| 0020 Verbrauch pro Aufruf in der Runs-API | echte Kosten pro Modellaufruf, für Budgets |
| 0021 das Hauptgespräch bleibt das der App | das Hauptgespräch jedes Agenten behält Live-Anzeige und Verlauf auf v0.22.5 |

**Wie wir sie upstream bringen.** Die Serie ist ein Git-Branch auf Hermes, exportiert mit `scripts/hermes-patches.sh`
(`branch`, `export`, `check`): ein Commit pro Patch, stabile Namen, jedes Mal dieselben Bytes. Der Wechsel auf ein
neueres Hermes ist ein Rebase dieses Branches, dann eine neue `BASE`. Ein Patch fällt weg, sobald Hermes dasselbe
kann: 0001 (Clarify in der Runs-API) und 0004 (Bilder für Folgefragen behalten) gingen, als v0.22.5 eigene Lösungen
brachte. 0005 ist upstream vorgeschlagen (NousResearch/hermes-agent#136660); kleine, allgemeine Patches (der Fix in
0008, 0009, 0011, 0013, 0017, die Labels aus 0019) sind die nächsten Kandidaten. Details pro Patch:
[`hermes/patches/README.md`](../hermes/patches/README.md).

## Was wir bewusst Hermes überlassen

Die Modellschleife und ihre Werkzeuge, Gedächtnisdateien und Skills, den Cron-Planer, die Freigabe-Engine, den
Tresorspeicher und das Ausfüllen von Formularen, den Bot Mode, Profile, das Docker-Terminal-Backend, den Bot Desktop,
Sitzungen und ihre Datenbank sowie die Gateways. Die Bridge konfiguriert sie (über Hermes' eigene CLI und
Konfigurationsdateien) und spricht über die Runs-API mit Hermes; sie forkt Hermes nie. Was allgemein genug ist, wird
upstream angeboten, statt hier zu bleiben.
