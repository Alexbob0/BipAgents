[English](privacy.en.md) · [Français](privacy.fr.md) · Deutsch · [Español](privacy.es.md)

# Datenschutzerklärung von BipAgents

> Zuletzt aktualisiert: [Datum].

## Kurz gesagt

- BipAgents verbindet dein Telefon mit **deinen eigenen KI-Agenten**, die auf Hermes Agent aufbauen. Sie laufen
  entweder **auf deinem Computer oder deinem Server** oder, mit dem **Alles-inklusive-Angebot**, auf **deinem Platz**,
  einem Konto auf einem Server, den Kerfy Studio bei einem Hosting-Partner mietet. Deine Unterhaltungen, deine Dateien,
  deine Stimme und das Gedächtnis deiner Agenten bleiben auf diesem Server und beim KI-Modellanbieter, der ihnen
  antwortet. **Kerfy Studio, der Herausgeber der App, liest sie nicht.**
- Keine Werbung, keine Nutzungsstatistiken, kein Tracking. Kein Konto mit deinem eigenen Server; ein Konto für das
  Alles-inklusive-Angebot.
- Mit deinem eigenen Server kann Kerfy Studio nur drei Dinge erhalten:
  1. was nötig ist, um deine **Benachrichtigungen** über sein Relay zuzustellen: ein versiegeltes Gerätetoken, nie
     den Text deiner Nachrichten;
  2. was du uns schicken möchtest: eine **E-Mail** an den Support oder die **Meldung** einer Antwort aus der App;
  3. **Absturzberichte**, nur wenn du ihre Weitergabe in den Einstellungen deines Telefons eingeschaltet hast.
- Mit dem Alles-inklusive-Angebot verarbeitet Kerfy Studio außerdem dein Konto, den Status deines Abos, technische
  Daten über deinen Platz (nie seinen Inhalt) und deine Sicherungen, verschlüsselt mit einem Schlüssel, den es nicht
  hat; es hostet deinen Platz, ohne darauf zuzugreifen (Abschnitt 5).

## 1. Wer verantwortlich ist

**Kerfy Studio**, SAS (französische vereinfachte Aktiengesellschaft) mit einem Kapital von [Kapital] €, Sitz:
[Adresse des Sitzes], eingetragen im RCS [Ort des Handelsregisters] unter der Nummer [SIREN] („Kerfy Studio“, „wir“).
Kontakt: **support@kerfystudio.com**.

Kerfy Studio hat keinen Datenschutzbeauftragten benannt: Für seine Tätigkeit ist das nicht vorgeschrieben (Art. 37
DSGVO). Für alle Fragen zu deinen Daten: support@kerfystudio.com. Für das Recht von Singapur (Personal Data Protection
Act), das eine für den Datenschutz zuständige Person verlangt, ist das der Präsident von Kerfy Studio, [Vorname Name],
erreichbar unter derselben Adresse.

Kerfy Studio ist Verantwortlicher für die Verarbeitungen, die die App durchführt, um zu funktionieren, so wie Kerfy
Studio sie gestaltet hat (Abschnitte 3, 4 und 7), auch wenn sie auf deinem Telefon bleiben und Kerfy Studio keinen
Zugriff darauf hat, und für die Verarbeitungen des Alles-inklusive-Angebots (Abschnitt 5). Wenn du deinen eigenen
Server nutzt, ist Kerfy Studio für die Daten, die dieser Server, deine Agenten und dein Modellanbieter verarbeiten,
weder Verantwortlicher noch Auftragsverarbeiter (Abschnitt 2).

## 2. Wie BipAgents funktioniert und wer was sieht

Mit deinem eigenen Computer oder deinem eigenen Server:

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

**Mit dem Alles-inklusive-Angebot** tritt dein Platz an die Stelle deines Servers, und Kerfy Studio wählt den Hoster
und den KI-Router: Abschnitt 5 sagt, wer was sieht.

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

**Deine Gesundheit, wenn du sie einschaltest (« Santé », Gesundheit).** Für jeden Agenten kannst du Gesundheit in
seinen Werkzeugen einschalten. Mit deiner Erlaubnis in Apple Health (iPhone, einschließlich der Daten deiner Apple
Watch) oder Health Connect (Android) liest die App, ohne dort je etwas zu schreiben, eine Zusammenfassung jedes Tages:
deinen Schlaf (Schlafdauer, Zeit im Bett, Zubettgeh- und Aufstehzeit), deine Schritte, aktive Energie,
Trainingsminuten, Ruhepuls, Herzfrequenzvariabilität, dein Gewicht und deine Trainingseinheiten (Art, Dauer, Strecke,
Energie). Sie sendet beim ersten Mal die letzten 30 Tage, danach die Tage, die sich geändert haben, **nur an deinen
Server** (oder an deinen Platz beim Komplettangebot), wo sie als eine Datei pro Tag gespeichert werden, höchstens 400
Tage. Nur die Agenten, für die du Gesundheit eingeschaltet hast, lesen sie, um dir bei Schlaf, Aktivität und Training
zu helfen; sie stellen keine Diagnosen. Das KI-Modell des Agenten erhält, was der Agent liest, um dir zu antworten:
beim Komplettangebot ist Gesundheit nur mit einem Modell möglich, das in der Europäischen Union läuft. Das Telefon
speichert nur die Daten und einen Fingerabdruck dessen, was es schon gesendet hat, nie die Werte, und nichts in
iCloud. Kerfy Studio erhält sie nicht; nichts wird verkauft oder für Werbung genutzt. **Rechtsgrundlage**: deine
ausdrückliche Einwilligung (Art. 6 Abs. 1 lit. a und Art. 9 Abs. 2 lit. a DSGVO), die du gibst, indem du Gesundheit
einschaltest und es in Apple Health oder Health Connect erlaubst. **Beenden**: schalte Gesundheit aus; sobald kein
Agent es mehr hat, werden die auf deinem Server gespeicherten Tage gelöscht. Sie gehen auch mit dem Agenten, mit
deinem Platz, oder wenn du die Erlaubnis in Apple Health oder Health Connect entziehst (dann wird nichts mehr
gesendet).

**Dein Kalender, wenn du ihn teilst (« Calendrier », Kalender).** Für jeden Agenten kannst du Kalender in seinen
Werkzeugen einschalten. Mit deiner Erlaubnis (Kalender auf dem iPhone und auf Android) liest die App die Kalender
deines Telefons, auch die darauf hinzugefügten Konten (Google, Outlook…), und sendet eine Kopie ihrer Termine, von
einer Woche vor bis zwei Monate nach heute (Titel, Zeiten, Ort, Notizen), **nur an deinen Server** (oder an deinen
Platz beim Rundum-Angebot), wenn die App geöffnet wird. Die Kopie bleibt dort und wird bei jedem Senden ersetzt. Nur
die Agenten, die du gewählt hast, lesen sie; wenn sie einen Termin eintragen, ändern oder löschen möchten, erledigt
das dein Telefon. Das KI-Modell des Agenten erhält die Termine, die der Agent liest, um dir zu antworten. Kerfy Studio
erhält sie nicht. **Rechtsgrundlage**: deine Einwilligung (Art. 6 Abs. 1 lit. a DSGVO), die du gibst, indem du Kalender
einschaltest und es auf deinem Telefon erlaubst. **Beenden**: schalte Kalender für den Agenten aus oder entziehe die
Erlaubnis in den Einstellungen des Telefons (dann wird nichts mehr gesendet).

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
Server gespeichert; mit dem Alles-inklusive-Angebot bewahrt es auch der Guichet von Kerfy Studio auf, weiterhin
versiegelt (Abschnitt 5.6). Die technischen Protokolle des Relays enthalten die Plattform, einen 4-Byte-
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

## 5. Das Alles-inklusive-Angebot

Dieser Abschnitt betrifft Personen, die das Alles-inklusive-Angebot nutzen ([Bedingungen des Angebots](offer.de.md)):
einen Platz auf einem Server, den Kerfy Studio bei einem Hosting-Partner mietet, ein KI-Guthaben, Sicherungen und ein
Konto. Der Rest dieser Datenschutzerklärung gilt ebenfalls (die App auf deinem Telefon, die Benachrichtigungen, der
Support). Der Dienst von Kerfy Studio, der Konten, Abos und Plätze verwaltet, heißt Guichet.

### 5.1 Wer wofür verantwortlich ist

- **Kerfy Studio ist Verantwortlicher** für die Verarbeitungen, die das Angebot erfordert: dein Konto, den Status deines
  Abos, das Hosting deines Platzes, das Senden deiner Nachrichten an das KI-Modell, die Sicherungen, die technische
  Überwachung und die Bearbeitung von Missbrauch.
- **Der Inhalt deines Platzes** (deine Agenten, deine Unterhaltungen, deine Dateien, ihr Gedächtnis, ihre Routinen, der
  Passworttresor): Du entscheidest, was du dort ablegst. Kerfy Studio hostet ihn und lässt ihn vom KI-Modell nur
  verarbeiten, um dir den Dienst bereitzustellen (Art. 6 Abs. 1 lit. b DSGVO), **ohne darauf zuzugreifen**: Seine
  Software liest ihn nicht (die einzigen Befehle, die ein Server ausführt, sind einen Platz neu zu
  installieren, ihn auf eine veröffentlichte Version des quelloffenen Codes von BipAgents zu aktualisieren oder ihn wieder zu öffnen, und keiner davon liest seinen Inhalt). Der Zugang zu den
  Konten von Kerfy Studio bei den Hostern ist dem Unternehmen vorbehalten. Auf den Inhalt eines Platzes würde nur
  zugegriffen, um eine gesetzliche Pflicht zu erfüllen (zum Beispiel eine gerichtliche Anordnung), und du würdest
  informiert, wenn das Gesetz es erlaubt. Wie jeder Hoster haben OVHcloud und Hetzner physischen Zugang zu ihren
  Servern. Wenn du dort sensible Daten ablegst (zum Beispiel zu deiner Gesundheit), werden sie nur verarbeitet, um
  auszuführen, worum du deine Agenten bittest.
- **Apple und Google** sind für ihre eigenen Verarbeitungen verantwortlich, wenn du dich bei ihnen anmeldest und wenn
  du in ihrem Store bezahlst.
- **Deine Agenten sind KIs und sagen das**: das Abzeichen „KI“, ein Hinweis zu Beginn jeder Live-Sitzung, ein Vermerk in
  den Benachrichtigungen und in den Nachrichten, die sie an andere Personen schicken (Verordnung (EU) 2024/1689,
  Art. 50).

### 5.2 Dein Konto

**Daten.** Die Kennung, die dir Apple oder Google gibt, und deine E-Mail-Adresse (bei Apple kann es eine
Weiterleitungsadresse sein); oder, mit einer E-Mail-Adresse, die Einmalcodes, die dir geschickt werden. Die Liste
deiner freigegebenen Geräte, der öffentliche Schlüssel jedes Geräts (was ein anderes deiner Geräte ihm schickt, ist
für es versiegelt) und ihre Benachrichtigungstokens, vom
Relay versiegelt (Abschnitt 4.1): Kerfy Studio sieht sie nie im Klartext. Die Einladungen, die du verschickst (die
E-Mail-Adresse der eingeladenen Person und ihr Status). Eine laufende Wiederherstellungsanfrage. Die Kopie deines
Sicherungsschlüssels, mit deinem Wiederherstellungscode verschlüsselt, die Kerfy Studio nicht lesen kann. Gekauftes
KI-Guthaben, das für ein nächstes Abo aufbewahrt wird (ein Betrag). Das Datum der Löschung des Kontos, wenn du sie
beantragt hast. Nicht dein Name, nicht dein Foto, nie die Schlüssel zu deinem Platz.

**Zwecke.** Dich anmelden, dein Abo und deine Plätze verbinden, deine Geräte freigeben, dein Konto wiederherstellen, dir
Anmeldecodes, Sicherheitshinweise (neues Gerät, laufende Wiederherstellung), die Bestätigung deines Kaufs, die
Eingangsbestätigung eines Widerrufs und die Hinweise zur Löschung schicken, die Einladungen verwalten. Keine
Werbe-E-Mails: Eine kommerzielle Nachricht würde nur verschickt, wenn du dein Einverständnis angekreuzt hast.
**Rechtsgrundlage.** Erfüllung des Vertrags (Art. 6 Abs. 1 lit. b DSGVO).
**Dauer.** Solange dein Konto besteht. Wenn du seine Löschung beantragst, wird es **30 Tage** danach gelöscht (deine
Plätze halten ab dem Antrag an, und bis dahin kannst du es dir anders überlegen), mit seinen Plätzen und ihren
Sicherungen, außer dem, was das Gesetz aufzubewahren verlangt: Als Hoster bewahrt Kerfy Studio getrennt, und nur um
Justizbehörden zu antworten, deine E-Mail-Adresse 5 Jahre und deine Apple- oder Google-Kennung 1 Jahr nach dem Ende des
Vertrags auf (französisches Gesetz über das Vertrauen in die digitale Wirtschaft, Art. 6 II; Dekret Nr. 2021-1362,
Art. 2, 3 und 8). Einmalcodes: solange sie gültig sind. Nicht angenommene Einladung: 30 Tage.
**Dein Konto löschen.** In der App, Einstellungen › Konto › Mein Konto löschen, oder, wenn du die App nicht mehr hast,
auf der Seite https://guichet.kerfystudio.com/account/delete mit einem Code, der an deine E-Mail-Adresse geschickt wird.
Die Personen, mit denen du deinen Tarif teilst, werden per E-Mail informiert: Ihre Plätze halten an dem Tag an, an dem
dein Konto gelöscht wird.

### 5.3 Dein Abo, deine Käufe und die kostenlose Woche

**Daten von den Stores.** Über die Benachrichtigungen von Apple (App Store Server Notifications) und von Google (Google
Play, Echtzeitbenachrichtigungen): die Kennung deines Abos beim Store, der Tarif, die Region, sein Status (kostenlose
Woche, aktiv, gesperrt, beendet, erstattet), das Ende des bezahlten Zeitraums, die Währung und den Monatspreis des Tarifs, ob die Verlängerung ausgeschaltet ist,
und für gekauftes KI-Guthaben die Kennung jedes Kaufs und sein Betrag. **Nie deine Zahlungsdaten**, die nur der Store
kennt. Das Land deines Store-Kontos, das die App schickt, um zu erfahren, ob das Angebot dort verkauft wird (es wird
nicht aufbewahrt).

**Dein Vertrag.** Das Datum des Beginns, das Datum der Bestätigungs-E-Mail und, wenn du den Vertrag widerrufst, Datum
und Uhrzeit deines Widerrufs.

**Genutztes KI-Guthaben.** Der Betrag, den der KI-Schlüssel jedes Platzes im Monat ausgegeben hat, höchstens einmal
pro Minute beim KI-Router abgefragt, und das übertragene Guthaben: ein Betrag, nie deine Nachrichten.

**Nur eine kostenlose Woche pro Gerät.**

- **Auf dem iPhone** nutzt die App DeviceCheck von Apple: Apple bewahrt für Kerfy Studio zwei Bits pro Gerät auf
  („hatte schon eine kostenlose Woche“). Kerfy Studio erhält keine Kennung des Geräts: Die App gibt ihm ein
  Einmal-Token, das es an Apple weiterreicht.
- **Auf Android** schickt die App ein Google-Play-Integrity-Token (Google bestätigt, dass die App echt ist und auf einem
  echten Gerät läuft) und einen Fingerabdruck der Android-Kennung des Geräts, auf dem Telefon berechnet (SHA-256) und
  dann vom Guichet vor dem Speichern mit einem geheimen Schlüssel erneut gehasht. Kerfy Studio bewahrt nur diesen
  zweiten Fingerabdruck auf, und nur für diese Begrenzung.
- **Die IP-Adresse** deines Telefons wird so lange aufbewahrt, wie ein Testplatz reserviert ist (höchstens
  15 Minuten), um die Zahl der Reservierungen pro Adresse zu begrenzen.

**Zwecke und Rechtsgrundlagen.** Dein Abo, deine Plätze und dein Guthaben bereitstellen und verwalten: Erfüllung des
Vertrags (Art. 6 Abs. 1 lit. b DSGVO). Verhindern, dass dieselbe Person kostenlose Wochen aneinanderreiht:
berechtigtes Interesse von Kerfy Studio, Betrug vorzubeugen und weiterhin eine kostenlose Woche anbieten zu können
(Art. 6 Abs. 1 lit. f und Erwägungsgrund 47 DSGVO). Das Lesen der Gerätekennung erfordert nicht deine Einwilligung,
denn es dient nur dazu, den kostenlosen Zugang zum Dienst auf einen begrenzten Zeitraum zu beschränken (Art. 82 des
französischen Datenschutzgesetzes; CNIL, Beschluss Nr. 2020-091, Art. 5). Den Nachweis des Vertrags aufbewahren:
berechtigtes Interesse von Kerfy Studio, seine Rechte verteidigen zu können.
**Dauer.** Abo- und Kaufdaten: während des Abos, dann 5 Jahre im Archiv, als Nachweis des Vertrags (Art. 2224 des
französischen Zivilgesetzbuchs). Fingerabdruck des Android-Geräts: 24 Monate. IP-Adresse: höchstens 15 Minuten. Die
DeviceCheck-Bits bewahrt Apple auf.

### 5.4 Dein Platz und das KI-Modell

**Wo.** Auf einem Server, der bei OVHcloud (Rechenzentren in der Europäischen Union) oder bei Hetzner (Deutschland oder
Finnland) gemietet ist, für alle Personen des Angebots, auch für die, die außerhalb der Union leben.

**Was dort liegt.** Was Abschnitt 5.1 beschreibt, und die öffentlichen SSH-Schlüssel, die du in der App hinzufügst. Der
Guichet von Kerfy Studio (der Dienst, der die Plätze verwaltet) kennt die Adresse deines Platzes und den Fingerabdruck
seines Zertifikats, damit sich dein Telefon damit verbinden kann und um ihn bei einem Ausfall zu verlegen.

**Das KI-Modell.** Die Nachrichten, die deine Agenten an das Modell schicken (mit den Dateien und Fotos, die sie ihm
zeigen), laufen über einen KI-Router, **EUrouter** (EUrouter B.V., Amsterdam, Niederlande), für alle Personen des
Angebots, dann über den Anbieter, der das gewählte Modell betreibt: [zu ergänzen: Anbieter der angebotenen Modelle und
Länder, in denen sie laufen]. EUrouter gibt an, standardmäßig weder die Anfragen noch die Antworten aufzubewahren und
ihre Metadaten (Uhrzeit, Modell, Zahl der Tokens, Kosten, IP-Adresse) bis zu 12 Monate aufzubewahren. Sie handeln im
Auftrag von Kerfy Studio. **Vor der ersten Nachricht nennt dir die App den Router und den Hersteller des Modells und
fragt nach deinem Einverständnis**; die Liste der Modelle nennt den Hersteller jedes Modells.

**Der Ruhezustand.** Dein Platz notiert selbst die letzte Anfrage der App; der Guichet weiß nur, dass er schläft.

**Rechtsgrundlage.** Erfüllung des Vertrags (Art. 6 Abs. 1 lit. b DSGVO).
**Dauer.** Während des Abos. Wenn es endet, wird dein Platz 30 Tage aufbewahrt (3 Tage nach einer kostenlosen Woche,
die nicht fortgesetzt wurde), dann wird sein Konto gelöscht und für jemand anderen neu installiert. Die überzähligen
Plätze eines verkleinerten Tarifs werden 30 Tage nach ihrem Anhalten gelöscht.

### 5.5 Die Sicherungen

Dein Platz sichert sich jede Stunde. Die Sicherung wird **auf dem Platz verschlüsselt**, bevor sie ihn verlässt, mit
einem Schlüssel, den nur deine Geräte haben, plus einer Kopie dieses Schlüssels, die mit deinem Wiederherstellungscode
verschlüsselt ist (Abschnitt 5.2). Kerfy Studio bewahrt diese Sicherungen bei [zu ergänzen: Hoster der Sicherungen]
auf, ohne sie lesen zu können: eine pro Stunde für 48 Stunden, eine pro Tag für 30 Tage, eine pro Woche für 12 Wochen.
Sie werden mit deinem Platz gelöscht.

**Das tägliche Abbild von OVHcloud.** Auf seinen Servern erstellt OVHcloud außerdem ein tägliches Abbild des ganzen
Servers, das es bei sich aufbewahrt: ein Wiederherstellungspunkt pro Tag, nicht von Kerfy Studio verschlüsselt. Wird
ein Platz für jemand anderen neu installiert, wird das Abbild, das ihn enthielt, innerhalb von etwa einem Tag ersetzt.

**Rechtsgrundlage.** Erfüllung des Vertrags (Art. 6 Abs. 1 lit. b DSGVO).

### 5.6 Die technische Überwachung und die Ausfälle

Alle zwei Minuten schickt jeder Server dem Guichet: seinen Arbeitsspeicher, seine Festplatte und seine Last, und für
jeden Platz, ob seine Bridge antwortet, ob sein Hermes Agent läuft oder schläft, seine BipAgents-Version und die
Uhrzeit seiner letzten erfolgreichen Sicherung. **Nie Inhalte.** Diese Daten dienen dazu, einen Ausfall zu erkennen,
Plätze zu verlegen, Updates in Wellen auszurollen und Kerfy Studio zu alarmieren.

Wenn der Server deines Platzes ausfällt, verlegt wird oder zurückkommt, werden deine Telefone über das Relay für
Benachrichtigungen informiert, mit einem allgemeinen Text: Dafür bewahrt der Guichet die versiegelten Tokens deiner
Telefone auf, mit deinem Platz verbunden.

**Rechtsgrundlage.** Erfüllung des Vertrags (Art. 6 Abs. 1 lit. b DSGVO).
**Dauer.** Nur die letzten Werte werden aufbewahrt und bei jeder Meldung ersetzt. Ereignisprotokoll (Ausfälle,
Verlegungen, Updates): 12 Monate. Versiegelte Tokens: solange das Gerät mit deinem Platz verbunden ist.

### 5.7 Missbrauchsmeldungen und Anfragen von Behörden

**Daten.** Die Meldung (wer sie schickt und wie die Person zu erreichen ist, die Tatsachen, die IP-Adresse und die
Uhrzeit), der betroffene Platz, unsere Entscheidung und ihre Begründung, unser Austausch mit dir.
**Zwecke.** Meldungen bearbeiten ([Richtlinie zur zulässigen Nutzung](acceptable-use.de.md)), die Server und die anderen
Personen schützen, Behörden antworten.
**Rechtsgrundlagen.** Rechtliche Verpflichtung (Art. 6 Abs. 1 lit. c DSGVO; Verordnung (EU) 2022/2065, Art. 16 bis 18)
und berechtigtes Interesse von Kerfy Studio, seinen Dienst und seine Kunden zu schützen (Art. 6 Abs. 1 lit. f DSGVO).
**Dauer.** 1 Jahr nach ihrer Bearbeitung, länger, wenn ein Rechtsstreit oder ein Verfahren es erfordert.

### 5.8 Die Dienstleister des Angebots

| Dienstleister | Rolle | Daten | Wo |
|---|---|---|---|
| OVHcloud (OVH SAS, Roubaix, Frankreich) | Hostet die Server der Plätze; tägliches Abbild jedes Servers | Inhalt der Plätze | Europäische Union |
| Hetzner Online GmbH (Gunzenhausen, Deutschland) | Hostet einen Teil der Server der Plätze | Inhalt der Plätze | Deutschland, Finnland |
| EUrouter B.V. (Amsterdam, Niederlande) | KI-Router | An das Modell geschickte Nachrichten, Metadaten | Europäische Union |
| Anbieter der Modelle: [zu ergänzen] | Betreiben das gewählte Modell | An das Modell geschickte Nachrichten | [zu ergänzen] |
| [zu ergänzen: Hoster der Sicherungen] | Bewahrt die Sicherungen auf | Verschlüsselte Sicherungen, für ihn und für Kerfy Studio unlesbar | [zu ergänzen] |
| [Hoster des Relays] | Hostet den Guichet und das Relay | Konto, Abo, technische Daten | Europäische Union |
| [zu ergänzen: Anbieter für den E-Mail-Versand] | Verschickt die Codes, die Sicherheitshinweise, die Einladungen, die Kaufbestätigung, die Eingangsbestätigung eines Widerrufs und die Hinweise zur Löschung | E-Mail-Adresse, Inhalt dieser E-Mails | [zu ergänzen] |

Sie sind mit Kerfy Studio durch einen Vertrag nach Art. 28 DSGVO verbunden. Apple (DeviceCheck, App Store) und Google
(Play Integrity, Google Play) sind ebenfalls beteiligt, wie in Abschnitt 5.3 beschrieben; „Mit Apple anmelden“ und die
Anmeldung mit Google liegen in ihrer eigenen Verantwortung.

## 6. Was Kerfy Studio nicht tut

- Keine Werbung, kein Statistik-Tool, kein Tracking über Apps oder Websites hinweg. Kein Konto, außer für das
  Alles-inklusive-Angebot.
- Kein Verkauf und keine Vermietung von Daten.
- Kein Training von KI-Modellen mit deinen Daten durch Kerfy Studio.
- Kein Zugriff auf deinen Server, auf den Inhalt deines Platzes, auf deine Unterhaltungen oder deine Dateien.

## 7. Andere Dienste, die die App einbezieht

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

## 8. Die Demo

Die eingebaute Demo läuft vollständig auf deinem Telefon: erfundene Agenten und Daten, kein Konto, kein Server, und
nichts wird an Kerfy Studio geschickt. Du kannst sie schon beim ersten Start mit „Nur die Demo“ öffnen. Ihr Podcast
ist eine vorab aufgenommene Audiodatei, die in der App enthalten ist (Stimmen: Fish Audio Drama 3, „Built with Fish
Audio“). Wenn du mit der Demo sprichst, gilt die Spracherkennung des Telefons (Abschnitt 7).

## 9. Die Stimmen

Die Stimmen der Bips werden **auf deinem Telefon** erzeugt, wenn es das kann: Die App lädt die Stimmen deiner Sprache
aus dem App Store oder von Google Play, und dein Server (oder dein Platz) schickt ihr den vorbereiteten Text zum
Vorlesen. Sonst werden sie auf deinem Server oder auf deinem Platz erzeugt. Wenn du dein eigenes Stimmmodell
anschließt, ist das ein Server, den du selbst betreibst. Kerfy Studio erhält nichts von dem, was die Stimmen
vorlesen.

## 10. Partnerlinks (Hosting, später)

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
wird wie in Abschnitt 17 beschrieben angekündigt.

## 11. Übermittlungen außerhalb der Europäischen Union

Das Relay für Benachrichtigungen wird in der Europäischen Union gehostet ([Hoster des Relays]). Um Benachrichtigungen
zuzustellen, können Apple und Google das Token und die Benachrichtigung außerhalb der Europäischen Union verarbeiten,
insbesondere in den USA:

- Google LLC (Firebase Cloud Messaging) ist nach dem „EU-U.S. Data Privacy Framework“ zertifiziert, das mit dem
  Durchführungsbeschluss (EU) 2023/1795 der Kommission vom 10. Juli 2023 als angemessenes Schutzniveau anerkannt
  wurde;
- Apple gibt an, Übermittlungen aus dem Europäischen Wirtschaftsraum durch die von der Europäischen Kommission
  erlassenen Standardvertragsklauseln abzusichern.

**Das Alles-inklusive-Angebot.** Die Server der Plätze stehen in der Europäischen Union, für alle Personen des Angebots.
EUrouter verarbeitet die Nachrichten in der Europäischen Union [zu ergänzen: Länder, in denen die angebotenen Modelle
laufen, und die Garantie, falls eines außerhalb der Union liegt]. Apple und Google (Anmeldung, Stores, DeviceCheck, Play
Integrity) können Daten in den USA verarbeiten, mit den oben genannten Garantien.

**Wenn du außerhalb der Europäischen Union lebst** (USA, Vereinigtes Königreich, Israel, Singapur), kommen deine Daten
in die Europäische Union, wo Kerfy Studio sie nach der DSGVO verarbeitet, die für alle seine Verarbeitungen gilt. Das
Vereinigte Königreich und Israel erkennen an, dass die Europäische Union ein angemessenes Schutzniveau bietet; für
Singapur sorgen dieses Schutzniveau und unsere Verträge mit unseren Dienstleistern für den vergleichbaren Schutz, den
das Recht von Singapur verlangt.

E-Mail-Anbieter: [Land; Garantie]. Anbieter für den E-Mail-Versand und Hoster der Sicherungen: [zu ergänzen: Land;
Garantie]. Du kannst bei uns eine Kopie der geltenden Garantien anfordern.

## 12. Speicherdauer

| Daten | Dauer |
|---|---|
| Daten der App auf deinem Telefon | Bis du den Agenten oder die App löschst |
| Gesundheitsdaten (« Santé ») auf deinem Server oder Platz | Höchstens 400 Tage; gelöscht, sobald kein Agent Gesundheit mehr hat, mit dem Agenten oder mit dem Platz |
| Gerätetoken beim Relay | Keine Speicherung: Es existiert nur versiegelt, auf deinem Telefon und deinem Server (und beim Guichet mit dem Alles-inklusive-Angebot, solange das Gerät mit deinem Platz verbunden ist) |
| IP-Adresse beim Relay | Im Arbeitsspeicher, höchstens eine Stunde, als Fingerabdruck |
| Technische Protokolle des Relays | Höchstens 6 Monate |
| Support-E-Mails | 2 Jahre nach dem letzten Austausch |
| Meldungen | 1 Jahr nach ihrer Bearbeitung |
| Absturzberichte | 1 Jahr nach Eingang (bei Kerfy Studio) |
| Konto des Angebots (Kennungen, E-Mail, Geräte, Einladungen, aufbewahrtes Guthaben) | Solange das Konto besteht; 30 Tage nach einem Löschantrag gelöscht |
| Nicht angenommene Einladung | 30 Tage |
| E-Mail-Adresse und Kontokennung nach dem Ende des Vertrags (Pflicht der Hoster) | 5 Jahre für die E-Mail, 1 Jahr für die Kennung, getrennt aufbewahrt |
| Abo und Käufe | Während des Abos, dann 5 Jahre im Archiv |
| Fingerabdruck des Android-Geräts, mit einem geheimen Schlüssel gehasht (kostenlose Woche) | 24 Monate |
| IP-Adresse bei der Reservierung einer kostenlosen Woche | Höchstens 15 Minuten |
| Inhalt deines Platzes | Während des Abos, dann 30 Tage (3 Tage nach einer nicht fortgesetzten kostenlosen Woche) |
| Sicherungen | 48 Stunden (stündlich), 30 Tage (täglich), 12 Wochen (wöchentlich); mit dem Platz gelöscht |
| Tägliches Abbild von OVHcloud | Ein Punkt pro Tag, innerhalb von etwa einem Tag nach der Neuinstallation des Platzes ersetzt |
| Technische Daten der Server | Nur die letzten Werte |
| Ereignisprotokoll des Guichet | 12 Monate |
| Metadaten bei EUrouter | Bis zu 12 Monate, laut EUrouter |
| Missbrauchsmeldungen | 1 Jahr nach ihrer Bearbeitung |

## 13. Sicherheit

Das Relay akzeptiert nur verschlüsselte Verbindungen (HTTPS), versiegelt die Tokens (XChaCha20-Poly1305), hat keine
Datenbank und schreibt nie ein Token im Klartext. In der App sind die Schlüssel deiner Agenten durch den
Schlüsselbund oder den Keystore geschützt, und die Verbindung zu deinem Server läuft über Tailscale oder über eine
HTTPS-Tür in deinem lokalen Netz, die nur das Zertifikat deines Servers akzeptiert. Der Code der App, der
Serverseite und des Relays ist öffentlich: [URL des Repositorys]. Um eine Sicherheitslücke zu melden:
**support@kerfystudio.com** (siehe die Datei SECURITY.md im Repository).

Mit dem Alles-inklusive-Angebot ist jeder Platz ein eigenes Konto auf dem Server, ohne Administratorrechte; Kerfy
Studio hat keinen SSH-Schlüssel auf den Servern, und ein Server führt nur Befehle aus, die der Guichet signiert hat.
Die App verbindet sich per HTTPS mit deinem Platz und prüft sein Zertifikat. Die Sicherungen werden auf dem Platz
verschlüsselt, bevor sie ihn verlassen.

**Bei einer Datenschutzverletzung** informiert Kerfy Studio die CNIL innerhalb von 72 Stunden (Art. 33 DSGVO) und die
betroffenen Personen, wenn das Risiko für sie hoch ist (Art. 34); für Personen in Singapur die Personal Data Protection
Commission innerhalb von 3 Tagen, wenn das Recht von Singapur es verlangt.

## 14. Deine Rechte

Du kannst Auskunft über deine Daten, ihre Berichtigung, ihre Löschung, die Einschränkung ihrer Verarbeitung und ihre
Übertragbarkeit verlangen und Anweisungen geben, was nach deinem Tod mit deinen Daten geschehen soll (französisches
Recht).

**Widerspruchsrecht.** Du kannst jederzeit aus Gründen, die sich aus deiner besonderen Situation ergeben, den
Verarbeitungen widersprechen, die auf unserem berechtigten Interesse beruhen (Begrenzung von Missbrauch, E-Mails,
Meldungen, Absturzberichte, Begrenzung der kostenlosen Wochen).

Schreib an **support@kerfystudio.com**. Wir antworten innerhalb eines Monats; bei einem komplexen Antrag kann sich die
Frist um zwei Monate verlängern. Wir können dich um Angaben bitten, um deine Identität zu prüfen.

Mit deinem eigenen Server speichern wir nichts, das ein Benachrichtigungstoken oder eine Meldung mit einer Person
verbindet, und können diese Daten über dich daher in der Regel nicht wiederfinden: Um Benachrichtigungen zu beenden,
schalte sie aus oder deinstalliere die App. Die Daten auf deinem Server verwaltest du selbst; für die Daten bei deinem
Modellanbieter oder deinem Hoster wende dich an sie.

**Mit dem Alles-inklusive-Angebot** verbindet dein Konto diese Daten mit dir: Wir können dir Auskunft geben, sie
berichtigen oder löschen. Auf den Inhalt deines Platzes greifst du selbst zu, über die App oder per SSH, und du kannst
ihn jederzeit kopieren. Um dein Konto zu löschen: Abschnitt 5.2.

**Wenn du in Kalifornien oder anderswo in den USA lebst.** Kerfy Studio verkauft und teilt deine personenbezogenen Daten
nicht im Sinne des kalifornischen Rechts (California Consumer Privacy Act) und macht keine zielgerichtete Werbung. Die
App und unsere Websites verfolgen dich nicht von einer Website zur anderen: Ein „Do Not Track“-Signal deines Browsers
ändert daher nichts an dem, was wir tun (California Online Privacy Protection Act). Die Rechte aus diesem Abschnitt übst
du unter support@kerfystudio.com aus.

## 15. Beschwerde

Wenn du meinst, dass deine Rechte nicht beachtet werden, kannst du dich bei der französischen Datenschutzbehörde
**CNIL** (3 place de Fontenoy, TSA 80715, 75334 Paris Cedex 07, Frankreich — https://www.cnil.fr/fr/plaintes) oder
bei der Datenschutzbehörde deines Landes beschweren: im Vereinigten Königreich beim Information Commissioner's Office
(https://ico.org.uk); in Israel bei der Privacy Protection Authority
(https://www.gov.il/en/departments/the_privacy_protection_authority); in Singapur bei der Personal Data Protection
Commission (https://www.pdpc.gov.sg).

## 16. Kinder

BipAgents richtet sich an Personen **ab 16 Jahren**. Zwischen 16 und 18 Jahren nutze die App mit dem Einverständnis
eines Elternteils und beachte das Mindestalter deines Modellanbieters (oft 18 Jahre). **Unter 16 Jahren darf die App
nur auf einer Installation genutzt werden, die ein Elternteil oder ein für das Kind verantwortlicher Erwachsener
eingerichtet hat und beaufsichtigt** (Familienfunktion): Dieser Erwachsene wählt die Agenten, ihre Zugänge und das
Modell, akzeptiert die Nutzungsbedingungen für das Kind und bleibt für dessen Nutzung verantwortlich.

Kerfy Studio fragt nicht nach dem Alter, und keine seiner Verarbeitungen beruht auf einer Einwilligung. Nur der
Dokumentenscanner unter Android fragt nach einem Einverständnis, für das, was Google davon erhält (Abschnitt 7): Für
ein Kind unter 15 Jahren gibt es der Erwachsene, der die Installation eingerichtet hat, gemeinsam mit dem Kind. Wenn
ein Kind uns geschrieben hat, kann ein Elternteil verlangen, dass wir die Nachricht löschen.

**Alles-inklusive-Angebot**: Es wird von einem Erwachsenen abgeschlossen oder mit dem Einverständnis eines Elternteils.
Eine eingeladene Person muss mindestens 16 Jahre alt sein; unter 16 Jahren darf ein Platz nur genutzt werden, wenn ein
Elternteil oder ein für das Kind verantwortlicher Erwachsener ihn eingerichtet hat und beaufsichtigt
([Bedingungen des Angebots](offer.de.md), Abschnitt 7). **In den USA darf keine Person unter 13 Jahren einen Platz
nutzen**, und Kerfy Studio erhebt wissentlich keine Daten eines Kindes unter 13 Jahren (Children's Online Privacy
Protection Act); falls es doch geschehen ist, schreib uns, und wir löschen sie.

## 17. Änderungen

Wir können diese Datenschutzerklärung ändern, insbesondere bevor wir Partnerlinks anbieten. Das Datum oben auf der
Seite ändert sich jedes Mal. Eine wichtige Änderung wird mindestens 30 Tage, bevor sie gilt, in der App angekündigt.

## 18. Kontakt

Kerfy Studio — [Adresse des Sitzes] — **support@kerfystudio.com** — Missbrauch: **abuse@kerfystudio.com**

Diese Datenschutzerklärung, die Nutzungsbedingungen, die [Bedingungen des Alles-inklusive-Angebots](offer.de.md), die
[Richtlinie zur zulässigen Nutzung](acceptable-use.de.md) und das Impressum (mentions légales, auf Französisch) findest
du auch in der App: Einstellungen › Über.
