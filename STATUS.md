# Projekt-Status — Private Wispr

**Diese Datei ist die Quelle der Wahrheit für den Sprint-/Meilensteins-Fortschritt.**
Pflege-Konvention (auch in AGENTS.md): Am Ende jedes Sprints diese Datei
aktualisieren, einen Git-Tag `m<Meilenstein>-sprint<n>` setzen und pushen.

Stand: **2026-10-25** · Head `4274fcf` (= origin/main) · Tags: `m0`, `m1-sprint1`, `m1-sprint2`

Legende: ✅ fertig & verifiziert · 🔄 in Arbeit · ⏳ geplant, nicht begonnen ·
„GUI offen" = muss der Nutzer selbst testen (Mikrofon + Ohren), siehe jeweilige
Session-Handoff-Datei in `hand-off/` (lokal, nicht im Repo).

## Meilensteine (Spec Abschnitt 8)

| Meilenstein | Inhalt | Status |
|---|---|---|
| **M0 — Walking Skeleton** | E2E-Diktat, Vulkan-Engine, Overlay, Paste, Hand-off-Modus | ✅ fertig 24.10.2026 (Tag `m0`) |
| **M1 — Produktkern** | Bedienbarkeit · Qualität/Nachvollziehbarkeit · Ausgabe-Stabilität | 🔄 Sprint 1+2 von 3 fertig |
| **M2 — Modi & Cloud** | Bereinigt-Modus (LLM), Prompt-Editor, OpenAI-kompat. Client | ⏳ |
| **M3 — Lange Formate** | Datei-Import, lange Live-Aufnahmen, Zusammenfassungen | ⏳ |
| v2-Backlog | Fuzzy (Kölner Phonetik), Auto-Lernen, Diarization, elevated Helper | ⏳ |

## Sprint-Detail

### M1 Sprint 1 — Bedienbarkeit ✅ (24.–25.10.2026, Tag `m1-sprint1`)

| Feature | Umsetzung | Headless | Nutzer-GUI |
|---|---|---|---|
| Einstellungen: Hand-off-Ablageort, Overlay abschaltbar, Autostart | `f541b46` | ✅ Tests | GUI offen |
| Hotkeys frei konfigurierbar (Capture, Konflikt-Prüfung, Persistenz) | `eceed78` | ✅ 22 Tests | Capture läuft `b29e3fe` direkt im fokussierten Fenster (unempfindlich gegen Hook-Lotterie `92887bc`/`3d1438f` und UIPI bei elevated Fenstern); E2E verifiziert inkl. nackter F-Tasten; Nutzer-GUI offen |
| Feedback-Klick beim Aufnahme-Start (PlaySound; rodio ❌ cpal-Konflikt) | `04efb61` | ✅ Tests | GUI offen |
| Modell-Download in der App (Progress, SHA256, Resume) | `61077fa` | ✅ 3 Tests | GUI offen |
| UI-Redesign (Blitztext-Prinzipien; CSS-Einbindung war kaputt) | `a035d1b` | ✅ Browser | GUI offen |
| Tray-Fix (X versteckt, Doppelklick öffnet) + README | `121a162` | ✅ build | GUI offen |
| Installationspaket (NSIS + Engine-DLLs als Resources, verifiziert) | `2cc17e4` | ✅ 7-Zip-Check | Laptop-Test offen |

**Nachgetragen 25.10. (Session Hotkey-/Mikrofon-Fixes):**
`f4549e0` PTT-Down-Logging, `846e4b0` Panic-Sichtbarkeit + Hook-Watchdog +
UIPI-bewusster Selbsttest, `b29e3fe` Webview-Key-Capture, `2596b2f` warm
gehaltenes Mikrofon (BT-Anfangsverlust, 5-Min-Reaper) — Live-Nachweis des
Warmhalts durch Nutzer offen.

**Offen von Sprint 1:** GUI-Verifizierung durch Nutzer (4 Punkte in
`hand-off/SESSION-HANDOFF-2026-10-25.md`); Laptop-Installation.

### M1 Sprint 2 — Qualität & Nachvollziehbarkeit ✅ implementiert (25.10.2026, Tag `m1-sprint2`)

Alle vier Features implementiert; 49 Unit-Tests grün, `--selftest` ok
(Vulkan, voller Pfad inkl. Wörterbuch-Anlage). GUI-Verifizierung durch
Nutzer offen (s. u.).

| Feature | Umsetzung | Headless | Nutzer-GUI |
|---|---|---|---|
| Wörterbuch-Pipeline (YAML, Aliase, Profile, NFC, Filler, Wortgrenzen, vocabulary-Feed) | `f082c36` | ✅ 15 Tests | offen |
| Text-Fitting (Abstände, Funktionswörter klein, Sie bleibt groß, Naht-/Schluss-Punkt-Regeln) | `61406da` | ✅ 8 Tests | offen |
| Fokus-Check (HWND+Prozess merken, Fokuswechsel → Zwischenablage + Meldung) | `abb2b60` | ✅ 1 Test + Build | offen (Fokuswechsel beim Sprechen provozieren) |
| Historie (SQLite, applied_rules+Version, target_app, Retention 90 d, Audio default löschen, Dashboard-Liste) | `4274fcf` | ✅ 3 Tests (In-Memory) | offen |

**Entwurfsentscheidungen Sprint 2** (Details in Modul-Köpfen):
- Kanone matcht case-insensitiv sich selbst → Fachwort-Kasuschreibung wird
  erzwungen, Treffer zählen als Fachbegriff-Quote-Daten (specs 6).
- `SeamContext` (Zeichen am Cursor) ist implementiert + getestet, läuft
  bis Sprint 3 mit `none()` — Erstwort-Großschreibung nur bei bekanntem
  Satzende links (Parakeet kasusiert selbst).
- Historie liegt laut Spec in `%APPDATA%` (Roaming); Modelle/settings.json
  historisch in `%LOCALAPPDATA%` — dokumentiert in dictionary.rs/history.rs.
- Wörterbuch-Ladefehler blockieren das Diktat nicht (leeres Wörterbuch,
  Fehler im Log) — Datei ist vom Nutzer editierbar.
- Spool-WAV: Löschung nach Transkription; bei Transkriptionsfehler bleibt
  es (Pfad geloggt) für Retry/Fehlersuche.

### M1 Sprint 3 — Ausgabe-Stabilität ⏳ geplant

| Feature | Spec | Hinweis |
|---|---|---|
| Paste-Transaktion (Delayed-Render, Read-Receipt, Clipboard-Restore, Terminal) | 4.6 | Eigenimplementierung nach paste_tx-Blaupause (IP-Direktive) |
| Engine-Worker-Isolation (Supervisor-Muster) | 4.1 | Pflicht bevor weitere Features auf die Engine gehen |
| Silero-VAD in der Aufnahme | 4.2 | mit Policy-Parametern |
| Setup-Assistent erster Start (Mikro → Modell → Hotkey-Test) | 4.10 | Restaufgabe für M1-Abschluss |

## Wartet auf den Nutzer

- **Eval-Clips** (10–20, eigene Stimme, inkl. ≥10-Min-Brain-Dump +
  Fachbegriffssätze) → `eval/audio/` + `.txt` — Voraussetzung für
  WER-/Fachbegriff-Metriken; die exemplarischen Wörterbuch-Regeln sind
  Platzhalter bis echte ASR-Fehler als Regelfälle vorliegen.
- GUI-Verifizierung Sprint 1 (siehe oben).
- GUI-Verifizierung Sprint 2: Wörterbuch-Karte (Profile umschalten, YAML
  im Editor ändern → wirkt ab nächstem Diktat), Fachwort diktieren (z. B.
  „Share Point"), Filler („ähm") weg, Funktionswörter klein, Fokuswechsel
  während Aufnahme → Zwischenablage-Meldung, Historie-Liste füllt sich,
  Klick kopiert, ✕/Alles löschen, `history.db` prüfen.
