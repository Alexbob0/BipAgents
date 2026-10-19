[English](privacy.en.md) · [Français](privacy.fr.md) · Deutsch · [Español](privacy.es.md)

# Datenschutzerklärung von BipAgents

> Zuletzt aktualisiert: [Datum].

## Kurz gesagt

- BipAgents verbindet dein Telefon mit **deinen eigenen KI-Agenten**, die mit Hermes Agent auf deinem Computer oder
  deinem Server laufen. Deine Unterhaltungen, deine Dateien, deine Stimme und das Gedächtnis deiner Agenten bleiben
  auf deinem Server und beim KI-Modellanbieter, den **du** gewählt hast. **Kerfy Studio, der Herausgeber der App,
  sieht sie nicht.**
- Kein Konto, keine Werbung, keine Nutzungsstatistiken, kein Tracking.
- Kerfy Studio kann nur drei Dinge erhalten:
  1. was nötig ist, um deine **Benachrichtigungen** über sein Relay zuzustellen: ein versiegeltes Gerätetoken, nie
     den Text deiner Nachrichten;
  2. was du uns schicken möchtest: eine **E-Mail** an den Support oder die **Meldung** einer Antwort aus der App;
  3. **Absturzberichte**, nur wenn du ihre Weitergabe in den Einstellungen deines Telefons eingeschaltet hast.

## 1. Wer verantwortlich ist

**Kerfy Studio**, SAS (französische vereinfachte Aktiengesellschaft) mit einem Kapital von [Kapital] €, Sitz:
[Adresse des Sitzes], eingetragen im RCS [Ort des Handelsregisters] unter der Nummer [SIREN] („Kerfy Studio“, „wir“).
Kontakt: **support@kerfystudio.com**.

Kerfy Studio hat keinen Datenschutzbeauftragten benannt: Für seine Tätigkeit ist das nicht vorgeschrieben (Art. 37
DSGVO). Für alle Fragen zu deinen Daten: support@kerfystudio.com.

Kerfy Studio ist Verantwortlicher für die Verarbeitungen, die die App durchführt, um zu funktionieren, so wie Kerfy
Studio sie gestaltet hat (Abschnitte 3, 4 und 6), auch wenn sie auf deinem Telefon bleiben und Kerfy Studio keinen
Zugriff darauf hat. Für die Daten, die dein Server, deine Agenten und dein Modellanbieter verarbeiten, ist Kerfy
Studio weder Verantwortlicher noch Auftragsverarbeiter (Abschnitt 2).

## 2. Wie BipAgents funktioniert und wer was sieht

| Wo | Was dort liegt | Wer entscheidet |
|---|---|---|
| **Dein Server** (dein Computer oder ein Server, den du mietest) | Hermes Agent, deine Agenten, deine Unterhaltungen, deine Dateien, ihr Gedächtnis, ihre geplanten Aufgaben, der Passworttresor | Du |
| **Dein Modellanbieter** (eine API in der Cloud, zum Beispiel OpenRouter, oder ein Modell, das bei dir läuft) | Jede Nachricht, die deine Agenten ihm schicken, damit er antwortet | Du, nach den Bedingungen dieses Anbieters |
| **Dein Telefon** | Was die App speichert, um zu funktionieren (Abschnitt 3) | Du |
| **Kerfy Studio** | Nur das, was in Abschnitt 4 beschrieben ist | Kerfy Studio |

- **BipAgents baut auf Hermes Agent auf, freier Software von Nous Research**
  (https://github.com/NousResearch/hermes-agent), die du auf deinem eigenen Rechner installierst. Die App verbindet
  sich direkt damit, über **Tailscale** (ein privates Netz zwischen deinen Geräten) oder über dein **lokales Netz**.
  Kerfy Studio hostet deine Agenten nicht und hat keinen Zugriff auf deinen Server. Für diese Daten ist Kerfy Studio
  weder Verantwortlicher noch Auftragsverarbeiter.
- **Wenn du einen Server mietest**, hat sein Hoster technisch Zugriff auf dessen Festplatte. Wähle einen, dem du
  vertraust (zum Beispiel in der Europäischen Union).
- **Dein Modellanbieter** erhält die Nachrichten, auf die er antwortet, und verarbeitet sie nach seinen eigenen
  Bedingungen und seiner eigenen Datenschutzerklärung. Manche bieten an, nichts zu speichern („zero data
  retention“): Das entscheidest du. Mit einem Modell, das bei dir läuft, verlässt nichts dein Netz.
- **Die Dienste, die du mit deinen Agenten verbindest** (E-Mail, Kalender, Websites, Smart Home …), erhalten, was
  deine Agenten ihnen schicken, nach ihren eigenen Regeln.
- **Mehrere Personen auf einem Server** (Familienfunktion): Jede Person hat ihre eigene Installation, getrennt von den
  anderen. Wer den Server verwaltet, ist den anderen gegenüber dafür verantwortlich.

## 3. Was die App auf deinem Telefon speichert

Diese Daten bleiben auf deinem Telefon und werden **nicht** an Kerfy Studio geschickt. Was hier über das iPhone
gesagt wird, gilt auch für das iPad.

- **Deine Agenten**: ihr Name, ihre Adressen auf deinem Server, der Fingerabdruck des Zertifikats deines lokalen
  Netzes, ihre Einstellungen (Maskottchen, Stimme, Sprache).
- **Die Zugangsschlüssel zu deinen Agenten**: im Schlüsselbund des iPhones (auf dieses Gerät beschränkt) oder
  verschlüsselt mit einem Schlüssel aus dem Android Keystore.
- **Eine Kopie deiner letzten Unterhaltungen**, damit du sie offline lesen kannst, deine Entwürfe, Nachrichten, die
  gerade gesendet werden, und Antworten, die während deiner Abwesenheit angekommen sind (der Eingang).
- **Deine Sprachnachrichten** (Aufnahmen und Transkripte), die Dateien, die du schickst, und die, die deine Agenten
  dir schicken.
- **Das Widget**, wenn du es hinzufügst, zeigt die letzte Nachricht eines Agenten, auch auf dem Sperrbildschirm des
  iPhones, wenn du es dort platzierst.
- **Auf dem iPhone** werden die Benachrichtigungen deiner Agenten wie Nachrichten angezeigt: iOS speichert den Namen
  des Agenten und den angezeigten Text auf dem Gerät, wie bei einem Messenger.
- **Der Browser des Agenten** (Ansicht Desktop): Auf dem iPhone wird nichts davon gespeichert; auf Android bleiben
  seine Cookies in der App.

**Warum.** Diese Verarbeitungen dienen nur dazu, dir die Funktionen bereitzustellen, die du nutzt (Art. 6 Abs. 1
lit. b DSGVO). Diese Daten bleiben auf deinem Telefon, bis du den Agenten oder die App löschst.

**Backups.** Auf dem iPhone sind diese Daten Teil deines iCloud-Backups oder des Backups auf deinem Computer, wenn du
es eingeschaltet hast, außer den Schlüsseln, die auf dem Gerät bleiben. Auf Android schließt die App ihre Daten von
Backups aus; eine direkte Übertragung auf ein neues Telefon kann einen Teil davon kopieren, aber nicht die Schlüssel,
die dort unlesbar bleiben.

**Löschen.** Lösche einen Agenten in der App, oder deinstalliere die App, um alles vom Telefon zu löschen. Das löscht
nicht, was sich auf deinem Server befindet.

**Was du selbst teilst.** Der Diagnosebericht (Einstellungen › Diagnose und Latenzen › Bericht exportieren) enthält
den Namen und die Adresse deiner Agenten; er geht nur an die App oder Person, die du im Teilen-Menü auswählst.

## 4. Was Kerfy Studio erhalten kann

### 4.1 Benachrichtigungen über das Relay von Kerfy Studio

**Warum ein Relay.** Um dir eine Benachrichtigung zu schicken, muss dein Server über Apple (iPhone) oder Google
(Android) gehen, mit Schlüsseln, die dem Herausgeber der App gehören und nicht jedem Server anvertraut werden können.
Das Relay von Kerfy Studio hält sie und leitet die Benachrichtigungen deines Servers weiter.

**Wie.** Wenn du Benachrichtigungen erlaubst, gibt Apple oder Google der App ein **Gerätetoken**. Die App schickt es
an das Relay, das es ihr **versiegelt** zurückgibt (verschlüsselt mit einem Schlüssel, den nur das Relay hat). Dein
Server bewahrt dieses versiegelte Token auf und schickt es mit jeder Benachrichtigung an das Relay; das Relay öffnet
es, leitet an Apple oder Google weiter und vergisst dann alles.

**Was das Relay verarbeitet:**

- das Gerätetoken, die Plattform (Apple oder Google) und die Umgebung (Test oder Produktion);
- die **IP-Adresse** deines Telefons (bei der Anmeldung) oder deines Servers (bei jeder Benachrichtigung), nur um
  Missbrauch zu begrenzen: Sie wird in einen Fingerabdruck umgewandelt, höchstens eine Stunde im Arbeitsspeicher
  gehalten und nie gespeichert;
- der Inhalt der Benachrichtigung, reduziert auf den **Namen des Agenten** (den Titel), einen **allgemeinen Text** wie
  „Neue Nachricht“ (jeder andere Text wird ersetzt) und **technische Kennungen** (Agent, Unterhaltung, Aufgabe, Art
  der Benachrichtigung). **Nie der Text deiner Nachrichten**: Die App holt ihn selbst von deinem Server.

**Was gespeichert wird.** Das Relay hat keine Datenbank. Das versiegelte Token wird auf deinem Telefon und auf deinem
Server gespeichert, nicht bei uns. Die technischen Protokolle des Relays enthalten die Plattform, einen 4-Byte-
Fingerabdruck des Tokens und die Antwort von Apple oder Google; sie werden **höchstens 6 Monate** aufbewahrt. Der
Hoster des Relays kann die IP-Adressen der Anfragen protokollieren: [was der Hoster des Relays protokolliert und wie
lange].

**Zwecke und Rechtsgrundlagen.** Die Benachrichtigungen zustellen, die du eingeschaltet hast: Erfüllung der
Nutzungsbedingungen (Art. 6 Abs. 1 lit. b DSGVO). Missbrauch begrenzen und den Dienst schützen: berechtigtes Interesse
von Kerfy Studio (Art. 6 Abs. 1 lit. f DSGVO).

**Auftragsverarbeiter.** [Hoster des Relays] (Europäische Union) und Google (Firebase Cloud Messaging), mit Kerfy
Studio durch einen Vertrag nach Art. 28 DSGVO verbunden. Apple stellt die Benachrichtigungen über seinen Dienst APNs
auf iPhones zu.

**Ohne Benachrichtigungen.** Du kannst Benachrichtigungen in den Einstellungen deines Telefons ablehnen oder
ausschalten: Die App funktioniert ohne sie. Wenn du die App deinstallierst, melden Apple oder Google das Token als
ungültig, und dein Server vergisst es. Wenn du die App selbst mit deinem eigenen Apple-Konto kompilierst, wird das
Relay von Kerfy Studio nicht genutzt.

**Auf Android** fragt die App Google erst dann nach einem Token, wenn du Benachrichtigungen einschaltest.

### 4.2 Support und Meldungen

- **Einstellungen › Support kontaktieren** bereitet eine E-Mail an support@kerfystudio.com vor, mit einer Diagnose,
  die du vor dem Senden lesen kannst: Version der App, Modell und Systemversion des Telefons, Sprache, Anzeigestil,
  Demo-Modus oder nicht, Status der Benachrichtigungen und für jeden Agenten seine Kategorie, ob er antwortet, ob er
  eine Bridge und eine Tür im lokalen Netz hat, und seine Stimme. **Keine Unterhaltung, kein Agentenname, keine
  Adresse, kein Schlüssel.** Nichts wird gesendet, bevor du in deiner Mail-App auf „Senden“ tippst. Wir erhalten auch
  deine E-Mail-Adresse und alles, was du schreibst: Schreib keine sensiblen Informationen hinein.
- **Diese Antwort melden** (lange auf eine Antwort drücken) schickt die Meldung **aus der App**, über das Relay von
  Kerfy Studio, nur an Kerfy Studio: weder an deinen Server noch an den Modellanbieter. Sie enthält den Grund, deine
  optionalen Angaben, **den Text der gemeldeten Antwort**, die Kategorie des Agenten, die Version der App und das
  Modell des Telefons. **Weder deine E-Mail-Adresse noch deinen Namen noch den des Agenten.** Nichts wird gesendet,
  bevor du auf „Senden“ tippst. Das Relay sieht dabei die IP-Adresse deines Telefons und nutzt sie nur, um Missbrauch
  zu begrenzen (Abschnitt 4.1). Da die Meldung keine Möglichkeit enthält, dich zu erreichen, können wir nicht darauf
  antworten: Schreib dem Support, wenn du eine Antwort erwartest.
- **Wenn das Relay nicht antwortet**, bietet dir die App an, dieselbe Meldung per E-Mail zu schicken: Dann erhalten
  wir auch deine E-Mail-Adresse.

Der Text der gemeldeten Antwort kann von deiner Gesundheit oder von anderen Personen handeln: Lies ihn vor dem Senden
noch einmal, und schreib in die Angaben nur, was nützlich ist. Wir löschen, was für die Bearbeitung der Meldung nicht
nötig ist.

**Zwecke.** Dir antworten, Meldungen bearbeiten, Fehler beheben und die App verbessern.
**Rechtsgrundlagen.** Auf deine Nachrichten antworten und deine Meldungen bearbeiten: berechtigtes Interesse von Kerfy
Studio, dir zu antworten und die App sicherer zu machen (Art. 6 Abs. 1 lit. f DSGVO). Auf einen Antrag zur Ausübung
deiner Rechte antworten: rechtliche Verpflichtung (Art. 6 Abs. 1 lit. c; Art. 12 bis 22 DSGVO).
**Dauer.** Support: 2 Jahre nach dem letzten Austausch. Meldungen: 1 Jahr nach ihrer Bearbeitung.
**Empfänger.** Kerfy Studio und seine Auftragsverarbeiter: sein E-Mail-Anbieter ([E-Mail-Anbieter], [Land]) und der
Hoster des Relays, über das die Meldungen laufen.

### 4.3 Absturzberichte

Wenn du in den Einstellungen deines Telefons die Weitergabe von Analysedaten an App-Entwickler eingeschaltet hast
(iPhone: Einstellungen › Datenschutz & Sicherheit › Analyse & Verbesserungen › Mit App-Entwicklern teilen; Android:
Weitergabe von Nutzungs- und Diagnosedaten), können Apple oder Google Kerfy Studio Absturzberichte und technische
Messwerte übermitteln (Modell, Systemversion, Stelle im Code, an der die App abgestürzt ist). Die App enthält kein
Statistik- oder Absturzbericht-Tool von Dritten: Diese Berichte kommen nur von Apple oder Google.

**Zweck.** Fehler beheben.
**Rechtsgrundlage.** Berechtigtes Interesse von Kerfy Studio, Fehler zu beheben (Art. 6 Abs. 1 lit. f DSGVO). Diese
Berichte erreichen uns nur, wenn du diese Weitergabe in den Einstellungen deines Telefons eingeschaltet hast, wo du
sie jederzeit ausschalten kannst. Apple und Google erheben sie in eigener Verantwortung.
**Dauer.** Kerfy Studio löscht die Berichte, die es herunterlädt, spätestens 1 Jahr nach ihrem Eingang.

### 4.4 Statistiken der Stores

Apple und Google stellen Kerfy Studio Statistiken über die App bereit (Downloads, Länder, Versionen). Apple und
Google erstellen sie in eigener Verantwortung und geben sie uns zusammengefasst: Sie erlauben uns nicht, dich zu
identifizieren.

## 5. Was Kerfy Studio nicht tut

- Kein Konto, keine Werbung, kein Statistik-Tool, kein Tracking über Apps oder Websites hinweg.
- Kein Verkauf und keine Vermietung von Daten.
- Kein Training von KI-Modellen mit deinen Daten: Wir haben sie nicht.
- Kein Zugriff auf deinen Server, deine Unterhaltungen oder deine Dateien.

## 6. Andere Dienste, die die App einbezieht

Neben deinem Server greift die App auf diese Dienste zurück, die ihre eigenen Regeln anwenden. Kerfy Studio erhält
nichts aus diesem Austausch. Kerfy Studio hat entschieden, sie in der App zu nutzen, und steht für das ein, was dein
Telefon verlässt; jeder Dienst ist dann für das verantwortlich, was er damit macht.

- **Symbole von Websites.** Um das kleine Symbol der Websites anzuzeigen, die in Antworten genannt werden, fragt die
  App deinen Server nach diesen Symbolen, und dein Server holt sie bei DuckDuckGo (icons.duckduckgo.com): DuckDuckGo
  erhält den **Namen der Website** (zum Beispiel „beispiel.de“) und sieht die Adresse deines Servers, nicht die
  deines Telefons. Die Nachricht erhält DuckDuckGo nicht.
- **Spracherkennung.** Sprachnachrichten und Live nutzen die Spracherkennung deines Telefons. Die App verlangt die
  Erkennung auf dem Gerät, wenn dein Telefon sie für deine Sprache anbietet. Sonst verarbeiten Apple oder Google
  (auf Android der Erkennungsdienst des Systems, oft der von Google) das Audio auf ihren Servern, in eigener
  Verantwortung. An deinen Server geht der erkannte Text. Rechtsgrundlage: dir die Funktion bereitzustellen, die du
  nutzt (Art. 6 Abs. 1 lit. b DSGVO).
- **Sprachausgabe des Telefons.** Wenn die Stimme des Telefons einen Text vorliest (in der Demo oder wenn dein Server
  nicht antwortet), nutzt sie die Sprachausgabe deines Telefons nach dessen Einstellungen.
- **Auf Android werden QR-Codes** auf dem Telefon gelesen, mit einer in die App eingebauten Bibliothek, ohne
  Google-Dienst.
- **Auf Android ist der Dokumentenscanner** eine Funktion der Google-Play-Dienste (Google ML Kit). Google gibt an, dass
  ML Kit ihm Informationen über das Gerät und die App, Installationskennungen sowie Nutzungs- und Leistungsdaten
  schickt, für die Diagnose und eigene Statistiken. Die App sagt dir das und fragt vor der ersten Nutzung des Scanners
  nach deinem Einverständnis; ohne dieses Einverständnis öffnet sich der Scanner nicht, und du kannst weiterhin ein
  Foto oder eine Datei anhängen. Du kannst deine Wahl jederzeit in den Einstellungen ändern.
- **Apple und Google** für die Benachrichtigungen (Abschnitt 4.1).
- **Tailscale**, wenn du es nutzt: eine eigene App mit eigenem Konto und eigener Datenschutzerklärung.
- **Links, die du öffnest** (Websites, die deine Agenten nennen, die Seite von Hermes Agent), öffnen sich in deinem
  Browser.

## 7. Die Demo

Die eingebaute Demo läuft vollständig auf deinem Telefon: erfundene Agenten und Daten, kein Konto, kein Server, und
nichts wird an Kerfy Studio geschickt. Du kannst sie schon beim ersten Start mit „Nur die Demo“ öffnen. Ihr Podcast
ist eine vorab aufgenommene Audiodatei, die in der App enthalten ist (Stimmen: Fish Audio Drama 3, „Built with Fish
Audio“). Wenn du mit der Demo sprichst, gilt die Spracherkennung des Telefons (Abschnitt 6).

## 8. Die Stimmen

Die Stimmen der Bips werden **auf deinem Server** erzeugt: Der vorzulesende Text geht an deinen Server, nicht zu uns.
Wenn du dein eigenes Stimmmodell anschließt, ist das ein Server, den du selbst betreibst. Kerfy Studio erhält nichts.

## 9. Partnerlinks (Hosting, später)

Die App oder die Website kerfystudio.com können später Hoster über Partnerlinks (Affiliate-Links) vorschlagen. In
diesem Fall:

- werden diese Links mit dem Hinweis „Partnerlink“ gekennzeichnet;
- setzen weder die App noch die Website einen Tracker für das Partnerprogramm: Der Partnercode steht nur in der
  Adresse des Links;
- kann der Hoster, wenn du einem solchen Link folgst, erfahren, dass dein Besuch von BipAgents kommt, und Kerfy Studio
  kann eine Provision erhalten, wenn du einen Vertrag abschließt;
- schickt die App dem Hoster keine personenbezogenen Daten; was du ihm danach gibst, unterliegt seiner eigenen
  Datenschutzerklärung.

Vor dem ersten Partnerlink wird dieser Abschnitt angeben, was Kerfy Studio von den Partnern erhält, und diese Änderung
wird wie in Abschnitt 16 beschrieben angekündigt.

## 10. Übermittlungen außerhalb der Europäischen Union

Das Relay für Benachrichtigungen wird in der Europäischen Union gehostet ([Hoster des Relays]). Um Benachrichtigungen
zuzustellen, können Apple und Google das Token und die Benachrichtigung außerhalb der Europäischen Union verarbeiten,
insbesondere in den USA:

- Google LLC (Firebase Cloud Messaging) ist nach dem „EU-U.S. Data Privacy Framework“ zertifiziert, das mit dem
  Durchführungsbeschluss (EU) 2023/1795 der Kommission vom 10. Juli 2023 als angemessenes Schutzniveau anerkannt
  wurde;
- Apple gibt an, Übermittlungen aus dem Europäischen Wirtschaftsraum durch die von der Europäischen Kommission
  erlassenen Standardvertragsklauseln abzusichern.

E-Mail-Anbieter: [Land; Garantie]. Du kannst bei uns eine Kopie der geltenden Garantien anfordern.

## 11. Speicherdauer

| Daten | Dauer |
|---|---|
| Daten der App auf deinem Telefon | Bis du den Agenten oder die App löschst |
| Gerätetoken beim Relay | Keine Speicherung: Es existiert nur versiegelt, auf deinem Telefon und deinem Server |
| IP-Adresse beim Relay | Im Arbeitsspeicher, höchstens eine Stunde, als Fingerabdruck |
| Technische Protokolle des Relays | Höchstens 6 Monate |
| Support-E-Mails | 2 Jahre nach dem letzten Austausch |
| Meldungen | 1 Jahr nach ihrer Bearbeitung |
| Absturzberichte | 1 Jahr nach Eingang (bei Kerfy Studio) |

## 12. Sicherheit

Das Relay akzeptiert nur verschlüsselte Verbindungen (HTTPS), versiegelt die Tokens (XChaCha20-Poly1305), hat keine
Datenbank und schreibt nie ein Token im Klartext. In der App sind die Schlüssel deiner Agenten durch den
Schlüsselbund oder den Keystore geschützt, und die Verbindung zu deinem Server läuft über Tailscale oder über eine
HTTPS-Tür in deinem lokalen Netz, die nur das Zertifikat deines Servers akzeptiert. Der Code der App, der
Serverseite und des Relays ist öffentlich: [URL des Repositorys]. Um eine Sicherheitslücke zu melden:
**support@kerfystudio.com** (siehe die Datei SECURITY.md im Repository).

## 13. Deine Rechte

Du kannst Auskunft über deine Daten, ihre Berichtigung, ihre Löschung, die Einschränkung ihrer Verarbeitung und ihre
Übertragbarkeit verlangen und Anweisungen geben, was nach deinem Tod mit deinen Daten geschehen soll (französisches
Recht).

**Widerspruchsrecht.** Du kannst jederzeit aus Gründen, die sich aus deiner besonderen Situation ergeben, den
Verarbeitungen widersprechen, die auf unserem berechtigten Interesse beruhen (Begrenzung von Missbrauch, E-Mails,
Meldungen, Absturzberichte).

Schreib an **support@kerfystudio.com**. Wir antworten innerhalb eines Monats; bei einem komplexen Antrag kann sich die
Frist um zwei Monate verlängern. Wir können dich um Angaben bitten, um deine Identität zu prüfen.

Da wir nichts speichern, das ein Benachrichtigungstoken oder eine Meldung mit einer Person verbindet, können wir diese
Daten über dich in der Regel nicht wiederfinden: Um Benachrichtigungen zu beenden, schalte sie aus oder deinstalliere
die App. Die Daten auf deinem Server verwaltest du selbst; für die Daten bei deinem Modellanbieter oder deinem Hoster
wende dich an sie.

## 14. Beschwerde

Wenn du meinst, dass deine Rechte nicht beachtet werden, kannst du dich bei der französischen Datenschutzbehörde
**CNIL** (3 place de Fontenoy, TSA 80715, 75334 Paris Cedex 07, Frankreich — https://www.cnil.fr/fr/plaintes) oder
bei der Datenschutzbehörde deines Landes beschweren.

## 15. Kinder

BipAgents richtet sich an Personen **ab 16 Jahren**. Zwischen 16 und 18 Jahren nutze die App mit dem Einverständnis
eines Elternteils und beachte das Mindestalter deines Modellanbieters (oft 18 Jahre). **Unter 16 Jahren darf die App
nur auf einer Installation genutzt werden, die ein Elternteil oder ein für das Kind verantwortlicher Erwachsener
eingerichtet hat und beaufsichtigt** (Familienfunktion): Dieser Erwachsene wählt die Agenten, ihre Zugänge und das
Modell, akzeptiert die Nutzungsbedingungen für das Kind und bleibt für dessen Nutzung verantwortlich.

Kerfy Studio fragt nicht nach dem Alter, und keine seiner Verarbeitungen beruht auf einer Einwilligung. Nur der
Dokumentenscanner unter Android fragt nach einem Einverständnis, für das, was Google davon erhält (Abschnitt 6): Für
ein Kind unter 15 Jahren gibt es der Erwachsene, der die Installation eingerichtet hat, gemeinsam mit dem Kind. Wenn
ein Kind uns geschrieben hat, kann ein Elternteil verlangen, dass wir die Nachricht löschen.

## 16. Änderungen

Wir können diese Datenschutzerklärung ändern, insbesondere bevor wir Partnerlinks anbieten. Das Datum oben auf der
Seite ändert sich jedes Mal. Eine wichtige Änderung wird mindestens 30 Tage, bevor sie gilt, in der App angekündigt.

## 17. Kontakt

Kerfy Studio — [Adresse des Sitzes] — **support@kerfystudio.com**

Diese Datenschutzerklärung, die Nutzungsbedingungen und das Impressum (mentions légales, auf Französisch) findest du
auch in der App: Einstellungen › Über.
