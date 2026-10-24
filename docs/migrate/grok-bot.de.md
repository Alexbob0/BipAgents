[🇬🇧 English](grok-bot.en.md) · [🇫🇷 Français](grok-bot.fr.md) · 🇩🇪 Deutsch · [🇪🇸 Español](grok-bot.es.md)

# Von Grok Bot umziehen (oder von ChatGPT dots, Muse, einem anderen Agenten)

Grok Bot hat keinen Export-Knopf: Der einzige Weg, sein Wissen mitzunehmen, ist, ihn alles aufschreiben zu lassen.
Die Nachricht unten lässt ihn seine Rolle, sein Gedächtnis, die Personen, Aufgaben, Routinen und Fähigkeiten in
Dateien schreiben und sie dann in eine ZIP-Datei packen, die du herunterlädst.

1. Kopiere die Nachricht und füge sie in die Unterhaltung mit deinem Bot ein (ein Bot nach dem anderen).
2. Lade die Datei `bipagents-export.zip` herunter, die er seiner Antwort anhängt. Lies sie durch, wenn du magst: Du
   kannst entfernen, was du nicht mitnehmen willst.
3. In BipAgents: **Agent hinzufügen › Von einem anderen Assistenten kommen › Datei importieren**. Die App zeigt dir alles, was
   sie anlegen wird, bevor etwas gespeichert wird.

Was der Bot schreibt, ist seine eigene Fassung seines Gedächtnisses: Sie kann unvollständig sein. Fähigkeiten kommen
ausgeschaltet an, und Routinen werden nicht ohne dein Ja neu geplant. Nichts, was wie ein Passwort, ein Schlüssel
oder eine Kartennummer aussieht, wird behalten.

## Die Nachricht

```text
Ich wechsle den Assistenten und will alles mitnehmen. Lege in deinem Arbeitsbereich einen Ordner bipagents-export an und schreibe darin so vollständig wie möglich diese Markdown-Dateien:

1. SOUL.md: deine Rolle, deine Persönlichkeit, deinen Ton und alle dauerhaften Regeln, die ich dir gegeben habe, in der zweiten Person („Du bist …“, „Du machst nie …“).
2. USER.md: alles, was du über mich weißt: wer ich bin, meine Lage, meine Vorlieben, meine Gewohnheiten, wie ich schreibe.
3. MEMORY.md: alles andere aus deinem Gedächtnis, ein Fakt pro Zeile, beginnend mit „- “, mit Datum (JJJJ-MM-TT), wenn du es kennst.
4. people.md: jede Person, die du kennst, ein Abschnitt „## Name“ mit ihrer Beziehung zu mir und dem, was man über sie wissen muss.
5. tasks.md: laufende Aufgaben und Projekte, je eine Zeile „- “ mit Frist und Stand.
6. routines.md: jede Routine oder geplante Aufgabe, ein Abschnitt „## Name“ mit genauem Zeitplan und Zeitzone, der vollständigen Anweisung, dem Ergebnis und den benutzten Apps.
7. skills/<name>/SKILL.md: jede Fähigkeit oder Vorgehensweise, die du gelernt hast, vollständig. Beginne mit:
---
name: <name-klein-mit-bindestrichen>
description: <wann man sie benutzt, in einem Satz>
---
dann wann man sie benutzt, was zu Beginn nötig ist, die nummerierten Schritte, wie man prüft und was du lieferst.
8. apps.md: die Namen der Apps und Dienste, mit denen du verbunden bist, einer pro Zeile, ohne Zugangsdaten.
9. conversations.md: eine Zusammenfassung je Thema unserer bisherigen Unterhaltungen, je ein Abschnitt „## Thema (Zeitraum)“ mit den getroffenen Entscheidungen.

Regeln:
- Sei vollständig: lieber zu viel als zu wenig. Mach mehrere Durchgänge, wenn nötig. Eine Datei ohne Inhalt: schreibe „(nichts)“.
- Erfinde nichts. Ergänze „(unsicher)“ bei allem, wessen du dir nicht sicher bist.
- Schreibe NIE ein Passwort, einen Code, Token, API-Schlüssel, eine Karten- oder Kontonummer: setze stattdessen „[entfernt]“.
- Schreibe in der Sprache, in der ich mit dir spreche.

Mach danach eine einzige ZIP-Datei dieses Ordners mit dem Namen bipagents-export.zip, hänge sie an deine Antwort an, damit ich sie herunterladen kann (wenn du keine ZIP-Datei machen kannst, hänge die Dateien einzeln an), und antworte „Fertig“ mit der Anzahl der Dateien.
```
