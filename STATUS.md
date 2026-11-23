# Projekt-Status — Private Wispr

**Diese Datei ist die Quelle der Wahrheit für den Sprint-/Meilensteins-Fortschritt.**
Pflege-Konvention (auch in AGENTS.md): Am Ende jedes Sprints diese Datei
aktualisieren, einen Git-Tag `m<Meilenstein>-sprint<n>` setzen und pushen.

Stand: **2026-11-22** (Sprint-3-Abschluss) · Tags: `m0`, `m1-sprint1`, `m1-sprint2`, `m1-sprint3`

Legende: ✅ fertig & verifiziert · 🔄 in Arbeit · ⏳ geplant, nicht begonnen ·
„GUI offen" = muss der Nutzer selbst testen (Mikrofon + Ohren), siehe jeweilige
Session-Handoff-Datei in `hand-off/` (lokal, nicht im Repo).

## Meilensteine (Spec Abschnitt 8)

| Meilenstein | Inhalt | Status |
|---|---|---|
| **M0 — Walking Skeleton** | E2E-Diktat, Vulkan-Engine, Overlay, Paste, Hand-off-Modus | ✅ fertig 24.10.2026 (Tag `m0`) |
| **M1 — Produktkern** | Bedienbarkeit · Qualität/Nachvollziehbarkeit · Ausgabe-Stabilität | ✅ Sprint 1–3 implementiert (Tag `m1-sprint3`); GUI-Verifizierung durch Nutzer offen |
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

### M1 Sprint 3 — Ausgabe-Stabilität ✅ implementiert (22.11.2026, Tag `m1-sprint3`)

| Feature | Spec | Umsetzung | Headless |
|---|---|---|---|
| **3.1 Engine-Worker-Isolation** | 4.1 | ✅ `engine_worker.rs`: Kindprozess (`--engine-worker`), JSON-Protokoll über stdio + roher f32-Block; `SttEngine`-Trait (In-Process bleibt Selftest-Pfad); Supervisor startet nach Crash neu, Modell-Reload, Fehler sichtbar (kein stiller Retry) | ✅ 58 Unit + 1 E2E (echter Worker-Prozess, 2 s) |
| **3.2 Paste-Transaktion** (Delayed-Render, Read-Receipt, Clipboard-Restore, Terminal) | 4.6 | ✅ `paste_tx.rs` (Eigenimplementierung nach paste_tx-Blaupause, IP-Direktive; ADR 0003): Snapshot → Delayed-Render + Opt-out-Marker → Chord → Lesebestätigung (erstes WM_RENDERFORMAT nach dem Chord) → Restore mit Sequenznummer-Guard; kein Receipt ⇒ „Einfügen nicht bestätigt", Text bleibt in der Zwischenablage (elevated läuft natürlich dort hin). Paste-Chord wählbar + Terminal-Erkennung (EXE-Liste). `seam.rs` liefert echte Cursor-Zeichen per UIA als SeamContext (nur Cursor-Modus; Selftest weiter none()) | ✅ 87 Unit + 1 E2E + 1 #[ignore]-Clipboard-Integration (gezielt, grün) |
| **3.4 Silero-VAD in der Aufnahme** | 4.2 | ✅ `vad.rs` (ADR 0004, gegrillt): Silero v6.2 in-process auf CPU hinter `SpeechAnalyzer`-Trait, lazy beim ersten Diktat; **Qualitätstor in `dictation::process`** — `NoSpeech` (kein Engine-Call, Halluzinations-Filter) + Zu-kurz-Gate auf Sprechzeit. **Bewusst ohne Sample-Trim**: Nachmessung BT700 (Release) zeigte 45 → 123/109/59 Fehler je Trim-Variante (Anlaufsilence = Encoder-Warm-up); Gates-only hält 45/2413 exakt, Transkripte bytengleich (`eval/reports/nach-vad-run.txt`). Kein Auto-Stop (Taste entscheidet), Parameter als Konstanten (Threshold 0,35, Pad 200 ms — konservativ für leise Diktate), modusunabhängig. Chunking (~30 s) auf M3 verschoben, Segment-Basis steht. Modell `silero-vad-v6.2-F32.gguf` mit Paketen verbreitet (Download-Skript + portable-ZIP/NSIS; fehlend ⇒ Gates offen, laut + `pw://status` sichtbar, nie blockierend). Selftest misst jetzt über den Engine-Worker wie die App | ✅ 101 Unit + 1 E2E; WER neu gemessen & unverändert (1,9 %/45/2413); GUI ✅ 23.11. (stille Aufnahme → „Keine Sprache erkannt", normales Diktat unverändert) |
| **3.3 Setup-Assistent erster Start** | 4.10 | ✅ `setup.rs` (Veteranen-Migration: Bestand + Modell ⇒ erledigt, nur echte Erststarts sehen den Assistenten) + `setup.tsx` (Willkommen → Mikro mit 5-s-Pegeltest → Modell → Diktat-Test als E2E-Finale → Fertig; „Später" ohne setup_done; Re-Entry „Einrichtung wiederholen" in den Einstellungen). Diktat-Pfad unberührt, WER unverändert | ✅ 93 Unit + 1 E2E (Migrations-Matrix); GUI offen |

### Zwischensprint 23.11. — Architektur-Refactor K1 ✅

Kandidat 1 des Architektur-Reviews (gegrillt, dann gebaut): Diktat-Session
und Overlay-Pille als tiefe Module.

| Änderung | Umsetzung | Verifikation |
|---|---|---|
| `pill.rs` — Overlay-Pille | Phasen-Politik (Texte, Dauern, overlay_enabled, Position, Event-Name) hinter Pill-Konstruktoren + reiner `pill_for()`; totes `level`-Feld aus dem Payload entfernt, Anzeige-Defaults („fertig ✓", „schreibt…") nach Rust — overlay.tsx reiner Renderer | `f9e713a` · 7 neue Unit-Tests inkl. Serde-Wire-Vertrag |
| `session.rs` — Diktat-Session | on_ptt_down/up (inkl. Verarbeitungs-Thread), Hand-off-Toggle, LastPayload aus lib.rs; hotkey.rs verdrahtet auf `session::` | `c3a8531` · `dictation::process` unangetastet (WER 1,9 %) |
| K7-Mitreisender | VAD-App-Slot als AppState-Feld statt Global; Lock-Ordnung als Feld-Doku | `627f5ae` |

108 Unit-Tests + 1 E2E grün, `cargo check` 0 Warnungen, `bun run build` ok.
GUI-Verifizierung durch Nutzer ✅ (23.11., Aufnahme mit Live-Pille).
GLOSSARY: neu „Diktat-Session", „Overlay-Pille".

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
