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

**Wörterbuch**:
Die editierbare Regelsammlung, die erkannte Schreibweisen in kanonische überführt — Kanones wirken zusätzlich als Fachwortschatz in der Erkennung.
_Avoid_: Ersetzungsliste, Fachwortschatz allein (ist nur eine Wirkung)
