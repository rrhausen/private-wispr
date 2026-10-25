# Projekt-Status — Private Wispr

**Diese Datei ist die Quelle der Wahrheit für den Sprint-/Meilensteins-Fortschritt.**
Pflege-Konvention (auch in AGENTS.md): Am Ende jedes Sprints diese Datei
aktualisieren, einen Git-Tag `m<Meilenstein>-sprint<n>` setzen und pushen.

Stand: **2026-10-25** · Head `2cc17e4` (= origin/main) · Tags: `m0`, `m1-sprint1`

Legende: ✅ fertig & verifiziert · 🔄 in Arbeit · ⏳ geplant, nicht begonnen ·
„GUI offen" = muss der Nutzer selbst testen (Mikrofon + Ohren), siehe jeweilige
Session-Handoff-Datei in `hand-off/` (lokal, nicht im Repo).

## Meilensteine (Spec Abschnitt 8)

| Meilenstein | Inhalt | Status |
|---|---|---|
| **M0 — Walking Skeleton** | E2E-Diktat, Vulkan-Engine, Overlay, Paste, Hand-off-Modus | ✅ fertig 24.10.2026 (Tag `m0`) |
| **M1 — Produktkern** | Bedienbarkeit · Qualität/Nachvollziehbarkeit · Ausgabe-Stabilität | 🔄 Sprint 1 von 3 fertig |
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

**Offen von Sprint 1:** GUI-Verifizierung durch Nutzer (4 Punkte in
`hand-off/SESSION-HANDOFF-2026-10-25.md`); Laptop-Installation.

### M1 Sprint 2 — Qualität & Nachvollziehbarkeit ⏳ nicht begonnen

Nächster Auftrag laut Handoff 25.10.; Reihenfolge = Spec-Reihenfolge.

| Feature | Spec | Betroffene Module |
|---|---|---|
| Wörterbuch-Pipeline (YAML-Regeln, Aliase, Profile, vocabulary-Feed) | 4.5 | neu `dictionary.rs`, `stt.rs` |
| Text-Fitting (Nahtstellen, Groß-/Klein, Schluss-Punkt) | 4.6 | neu `textfit.rs` |
| Fokus-Check (Ziel-HWND merken, Fokuswechsel → Clipboard) | 4.6 | `recording.rs`, `paste.rs`, `lib.rs` |
| Historie (SQLite inkl. applied_rules, Audio default löschen) | 4.8 | neu `history.rs`, Dashboard-UI |

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
  WER-/Fachbegriff-Metriken (Sprint 2); ohne echte Fehler keine guten Regeln.
- GUI-Verifizierung Sprint 1 (siehe oben).
