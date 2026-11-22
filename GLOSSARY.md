# Private Wispr

Deutsch-first Diktat-App für Windows; Sprache bleibt lokal auf dem Rechner. Dieses Glossar ist die verbindliche Sprache für Code-Kommentare, UI-Texte, Doku und Architektur-Reviews.

## Language

**Diktat**:
Eine zusammenhängende Sprachaufnahme von der aktivierten Diktat-Taste bis zu ihrem Ende; die Einheit, die verarbeitet, ausgeliefert und historisiert wird.
_Avoid_: Aufnahme (meint die Audiotechnik), Transkription (ist nur ein Schritt darin)

**Diktat-Verarbeitung**:
Der stille Vorgang von der beendeten Aufnahme bis zum fertigen Text — Wörterbuch einfließen lassen, transkribieren, Regeln anwenden, Text fügen, Audio verwerten. Liefert ein Ergebnis und hat keine Nebenwirkungen nach außen.
_Avoid_: Pipeline, Auswertung

**Cursor-Diktat**:
Diktat, dessen fertiger Text am Cursor der fokussierten App eingefügt wird; Taste wird gehalten (Push-to-Talk).
_Avoid_: Normaler Modus, Einfüge-Modus

**Hand-off**:
Diktat, dessen fertiger Text in die Tagesdatei wandert — für Ziele, die Strg+V nicht annehmen; Taste ist Umschalter (antippen startet, nochmal beendet).
_Avoid_: Handoff-Datei-Modus, Ablage-Modus

**Ausliefern**:
Der Vorgang, der den fertigen Text eines Diktats an sein Ziel bringt — am Cursor einfügen, in die Zwischenablage legen oder der Hand-off-Tagesdatei anhängen. Plant das Ziel als reine Entscheidung (inklusive Fokus-Semantik) und berichtet ehrlich, wohin der Text wirklich ging — auch im Fehlerfall.
_Avoid_: Ausgabe (sagt nicht, wohin), Einfügen (ist nur das Cursor-Ziel)

**Paste-Transaktion**:
Das Einfügen am Cursor als Transaktion statt Feuer-und-vergessen: Die Zwischenablage wird per Delayed-Render nur *versprochen*, das Einfügen per Tastenkombination ausgelöst, das erste Lesen der Ziel-App bestätigt den Vorgang, und danach wird die ursprüngliche Zwischenablage des Nutzers wiederhergestellt. Bleibt die Bestätigung aus, bleibt der Text in der Zwischenablage (manuelles Einfügen möglich).
_Avoid_: Auto-Paste (sagt nichts über die Transaktion), Clipboard-Trick

**Lesebestätigung**:
Das Signal, dass die Ziel-App die versprochene Zwischenablage tatsächlich gelesen hat (`WM_RENDERFORMAT` an den Verwalter der Paste-Transaktion) — der Moment, in dem das Einfügen wirklich stattfindet und das Wiederherstellen der alten Zwischenablage sicher werden kann. Erst Ab-senden der Tastenkombination zählende Bestätigungen verhindern, dass Clipboard-Manager eine falsche Bestätigung auslösen.
_Avoid_: Read-Receipt (englisch), Bestätigung allein (zu unspezifisch)

**Wörterbuch**:
Die editierbare Regelsammlung, die erkannte Schreibweisen in kanonische überführt — Kanones wirken zusätzlich als Fachwortschatz in der Erkennung.
_Avoid_: Ersetzungsliste, Fachwortschatz allein (ist nur eine Wirkung)

**Sprach-Aktivitätsprüfung (VAD)**:
Der lokale Silero-Durchlauf NACH dem Stopp einer Aufnahme, der misst, wo im Audio Sprache liegt — als Grundlage des Qualitätstors der Diktat-Verarbeitung: Aufnahmen ohne Sprache werden nicht transkribiert (Halluzinations-Filter), zu kurze nach Sprechzeit (statt Gesamtdauer) verworfen. Schneiden tut die Prüfung nichts — die volle Aufnahme geht zur Engine (Kürzung verschlechterte die Erkennung messbar, ADR 0004).
_Avoid_: Stille-Erkennung (nur eine Folge), Auto-Stop (gibt es bewusst nicht — die Taste entscheidet)

**Qualitätstor**:
Die Entscheidung der Diktat-Verarbeitung, OB eine Aufnahme überhaupt transkribiert wird: zu kurz (Gesamtdauer), keine Sprache, zu wenig gesprochene Zeit. Ein durchfallenes Diktat wird gemeldet („Zu kurz"/„Keine Sprache erkannt") und sein Audio ordentlich entsorgt — nie still verworfen.
_Avoid_: Filter (zu vage), Gate (englisch)
