# AGENTS.md — Arbeitsanleitung für Coding-Agenten (Private Wispr)

Dieses Repo wird maßgeblich mit AI-Coding-Agenten entwickelt. Diese Datei ist
der verbindliche Einstieg. **Die Spezifikation ist die Wahrheit für Verhalten
und Architektur:** [`specs/2026-10-24-private-wispr-spezifikation.md`](specs/2026-10-24-private-wispr-spezifikation.md)
(inkl. Entscheidungs-Log am Ende — Entscheidungen dort nachlesen, nicht neu
diskutieren). **[`GLOSSARY.md`](GLOSSARY.md) ist die verbindliche Sprache**
für Code, UI-Texte und Reviews; **[`docs/adr/`](docs/adr/) festhält
getroffene Architektur-Entscheidungen** (nicht neu aufrollen — neue
Entscheidungen als ADR dazu). **[`STATUS.md`](STATUS.md) ist die Wahrheit
für den Sprint-/Meilensteins-Fortschritt** — am Ende jedes Sprints
aktualisieren, Git-Tag `m<Meilenstein>-sprint<n>` setzen, pushen.

## Projekt in einem Satz

Private, deutsch-first Diktat-App für Windows 11: Taste halten, sprechen,
loslassen — Text erscheint am Cursor. Lokale STT via transcribe.cpp (GGUF,
Vulkan), Tauri 2 (Rust + React/TS). Deutsch zuerst, Englisch second.

## Architektur (Stand M1, Sprint 3)

```
src-tauri/src/
  lib.rs        Orchestrierung: AppState, PTT-Flow, Overlay-Steuerung, Tray
  dictation.rs  Diktat-Verarbeitung (GLOSSARY.md) — stilles tiefes Modul hinter
                einer Funktion: transcribe (SttEngine) + Regeln + Fitting +
                komplette Audio-Politik (specs 4.8, jeden Pfad entsorgt).
                App und Selftest rufen dieselbe process(); Abhängigkeiten
                injiziert, Fake-Engine als Test-Adapter
  deliver.rs    Ausliefern (GLOSSARY.md) — plan(mode, fokus, vordergrund)
                als reine Funktion: Fokus-Semantik vereinheitlicht (nicht
                prüfbar ⇒ vertrauen; geblockt nur bei nachgewiesenem
                Fensterwechsel) + dünne execute() mit ehrlichem Bericht
                (target/done_msg/history_mode). Cursor-Einfügen läuft über
                die Paste-Transaktion (paste_tx). Hand-off-Fehler ⇒
                Zwischenablage, NIEMALS Strg+V (elevated Ziele)
  hotkey.rs     PTT-Hotkeys (handy-keys, Low-Level-Hook, key-up), frei
                konfigurierbar: Konflikt-Validierung, Umregistrierung zur
                Laufzeit. Cursor = Halten (PTT), Hand-off = Toggle.
                Key-Capture bewusst im Frontend (ADR 0001)
  recording.rs  Aufnahme (handy-recorder: WASAPI → 16 kHz mono f32) mit
                Disk-Spooling (WAV wächst auf Platte, kein Längen-Limit);
                WAV wird nach Transkription default gelöscht (specs 4.8)
  stt.rs        SttEngine-Trait + In-Process-Engine: transcribe.cpp (Parakeet
                TDT v3 Q8, Backend::Auto→Vulkan) — Selftest/Eval-Pfad;
                transcribe() nimmt vocabulary-Feed aus dem Wörterbuch
  engine_worker.rs Engine-Isolation (specs 4.1, App-Betrieb): eigener
                Kindprozess (--engine-worker), JSON-Protokoll auf stdio +
                roher f32-Sample-Block; Supervisor startet den Worker nach
                Crash neu (GPU-Faults killen nur den Worker, Fehler sichtbar)
  dictionary.rs Wörterbuch-Pipeline (specs 4.5): YAML-Regeln+Aliase+Profile
                in %APPDATA%\PrivateWispr\dictionary\, NFC, Filler-Entfernung,
                längste Phrase zuerst, \p{L}-Wortgrenzen, Komposita-Toleranz
  fitting.rs    Text-Fitting (specs 4.6): Abstände, Satzanfang, Funktions-
                wörter klein, Nahtstellen + Schluss-Punkt via SeamContext
  focus.rs      Fokus-Check (specs 4.6): Ziel-HWND+Prozess bei Aufnahmebeginn
                merken; Vergleichs-Semantik liegt in deliver::plan (nicht
                prüfbar ⇒ vertrauen, Wechsel ⇒ Zwischenablage)
  history.rs    Historie (specs 4.8): SQLite in %APPDATA%\PrivateWispr\,
                raw/final/applied_rules+Version, Retention (default 90 d)
  model_download.rs  In-App-Modell-Download (ureq, Progress-Events, SHA256,
                abbrechbar, Resume via Range-Requests)
  paste.rs      Einfügen-Bausteine: copy_to_clipboard (arboard, plain) +
                send_paste_chord (Strg+V / Strg+Umschalt+V / Umschalt+
                Einfügen, SendInput) + select_chord (Terminal-Erkennung
                an EXE-Liste, Setting paste_chord gewinnt)
  paste_tx.rs   Paste-Transaktion (specs 4.6, ADR 0003, Eigenimplementierung
                nach Handys paste_tx-Blaupause — IP-Direktive): Delayed-
                Render-Clipboard, Lesebestätigung via WM_RENDERFORMAT
                (erst NACH dem Chord zählend — Clipboard-Manager), Clipboard-
                Restore aus Vollformat-Snapshot mit Sequenznummer-Guard;
                kein Receipt ⇒ Text bleibt in der Zwischenablage
  seam.rs       Naht-Kontext (specs 4.6): Cursor-Zeichen links/rechts per
                UIA-TextPattern der fokussierten App, Hilfsthread + 250-ms-
                Cap; Scheitern ⇒ SeamContext::none() (kosmetisch)
  feedback.rs   Dezenter Klick beim Aufnahme-Start (WAV-Synthese + PlaySoundW;
                abschaltbar). rodio geht nicht: cpal-Links-Konflikt mit handy-recorder
  handoff.rs    Hand-off-Diktat in Tagesdateien; Ablageort konfigurierbar
  settings.rs   Settings hinter einer Seam (ADR 0002): Prozess-Cache, lazy
                geladen; current() (Clone, keine Platte) + update() als
                einziger, atomarer Schreibpfad (Committ erst nach
                erfolgreichem Schreiben). Korrupte JSON ⇒ laut + Backup
                settings.json.corrupt, fehlende ⇒ still Defaults; Felder mit
                Defaults statt Migration
  setup.rs      Setup-Assistent-Logik (specs 4.10): Erststart-Erkennung.
                Veteranen-Migration einmalig beim Start (settings.json
                existierte + Modell auffindbar + setup_done fehlt ⇒ als
                erledigt markiert, geloggt); danach entscheidet allein
                settings.setup_done. UI in setup.tsx, Mikro-Test-Commands
                (mic_test_start/stop, 5 s Auto-Stopp) in lib.rs
  selftest.rs   Headless `--selftest [wav|verzeichnis]` — Einzel-WAV oder
                Batch mit WER gegen Ground-Truths (Eval-Harness); ASR läuft
                über den Engine-Worker wie in der App (ADR 0004)
  vad.rs        Sprach-Aktivitätsprüfung (specs 4.2, ADR 0004): Silero in-
                process auf CPU hinter SpeechAnalyzer-Trait, lazy beim ersten
                Diktat; Qualitätstor in dictation::process (NoSpeech, Zu-kurz
                auf Sprechzeit) — KEIN Sample-Trim (messbar schädlich für
                Parakeet), Segmente als M3-Chunk-Basis erhalten. Modell:
                silero-vad-v6.2-F32.gguf, mit den Paketen verbreitet
src/            Frontend: main.tsx→App.tsx (Tabs Übersicht/Einstellungen,
                Übersicht: Letzte Diktate-Liste aus der Historie; App reicht
                nur durch, was die Übersicht zeigt; rendert bis setup_done
                den Setup-Assistenten statt der Tabs), settings.tsx (Hotkeys,
                Ablageort, Mikrofon, Wörterbuch-Profile, Overlay, Sound,
                Autostart, Modell-Download, Version + „Einrichtung
                wiederholen" — lädt Mikrofon/Geräte/Ablageort selbst),
                setup.tsx (Assistent: Willkommen → Mikro mit Pegel-Test →
                Modell → Diktat-Test → Fertig; Events statt Polling),
                theme.ts (Farbschema: „system" auflösen,
                OS-Wechsel-Listener einmalig montiert), overlay.tsx (Pille)
```

**Datenfluss:** Hotkey Pressed → `recording::start` (+ `focus::capture`) →
Overlay „recording" → Released → `recording::stop` → Thread:
`dictation::process` (stille Pipeline: VAD-Qualitätstor → transcribe +
Regeln + Fitting + Audio-Politik) → Ausliefern: `deliver::plan` (reine
Ziel-Entscheidung mit Fokus-Semantik) + `deliver::execute` (dünn:
Zwischenablage/Strg+V/Hand-off, ehrlicher Bericht) → `history::insert` →
Overlay „done".
Events: `pw://status`, `pw://overlay`,
`pw://level`, `pw://last`, `pw://mic-fallback`, `pw://hook-status`,
`pw://model-download`, `pw://history-changed`.

## Wie baue und teste ich

```bash
# Frontend-Deps (einmalig / nach package.json-Änderungen)
bun install

# Rust-Kompilierung prüfen (schnellster Feedback)
cd src-tauri && cargo check

# App im Dev-Modus (öffnet Fenster, Hotkey aktiv)
bun run tauri dev

# Headless-Smoke-Test (Engine + Modell-Load, ohne GUI):
target/debug/private-wispr.exe --selftest

# Vollständiger Transkriptions-Test mit Audio (16 kHz mono direkt; M4A/MP3/
# OPUS/WAV anderer Rate via ffmpeg — PATH oder PW_FFMPEG):
target/debug/private-wispr.exe --selftest path/to/test.wav

# Eval-Batch: alle Clips eines Ordners mit WER gegen die .txt-Ground-Truths:
target/release/private-wispr.exe --selftest ../eval/audio

# Frontend-Build
bun run build
```

**CMake** liegt portabel unter
`%LOCALAPPDATA%\Microsoft\WinGet\Packages\Kitware.CMake_*\bin` — bei
Build-Fehlern („cmake not found") diesen Pfad in PATH aufnehmen.
**Modelle** liegen unter `%LOCALAPPDATA%\PrivateWispr\models\` (STT-Download:
`scripts/download-model.ps1`, 705 MB; VAD-Download:
`scripts/download-vad-model.ps1`, 1,2 MB — beide SHA256-geprüft; das
VAD-Modell brauchen Bundles zwingend, `build.rs` spiegelt es nach
`target/release`).

## Konventionen

- **IP-Direktive (vom Nutzer, 24.10.2026): KEIN 1:1-Code von Drittprojekten
  übernehmen — auch nicht aus MIT-lizenzierten Repos.** Wir studieren Konzepte,
  Verhalten und Architektur (Handy, OpenWhispr, Blitztext, Epicenter/Whispering)
  und schreiben jede Implementierung selbst. Reverse-Engineering dient nur dem
  Verständnis von Verhalten und Mustern, niemals der Code-Reproduktion.
  Zulässig: normale Abhängigkeits-Nutzung von Crates/npm-Paketen (handy-keys,
  handy-recorder, transcribe-cpp etc.) gemäß deren Lizenz, denn das ist
  Verwendung, nicht Übernahme. Verboten: Quelltext-Kopien, auch auszugsweise
  oder mit Umbenennung.

- **Sprache:** UI-Texte und User-facing Strings auf Deutsch; Code-Bezeichner
  und Kommentare auf Englisch ok, deutsche Kommentare für fachliche
  Begründungen willkommen (siehe Bestand).
- **Commits:** konventionell und klein (`feat:`, `fix:`, `docs:`, `refactor:`,
  `test:`, `chore:`), Deutsch oder Englisch, eine logische Änderung pro Commit.
- **Keine stillen Fallbacks** (spezifiziertes Vertrauenskonzept): Fehler
  melden, nie heimlich auf CPU/Cloud/anders umschalten — Ausnahme: Engine-internes
  GPU→CPU ist ok, muss aber im Status sichtbar bleiben.
- **Audio/Stimmdaten niemals committen** (`.gitignore` schließt `*.wav`,
  `eval/audio/` aus). Ground-Truth-Texte (`*.txt`) dürfen committet werden.
- **Modell-Dateien niemals committen** (`.gguf`, `.onnx`).
- Neue Backend-Fähigkeiten: Event `pw://…` + Frontend-Zustand, kein Polling.
- Tests: Rust-Unit-Tests (`cargo test`) für Regel-Logik; E2E über den
  Selftest mit festen WAVs unter `eval/audio/`.

## Wichtigste bewusste Abweichungen/Offenpunkte (M0 → M1)

1. ~~Engine läuft in-process~~ ✅ Sprint 3.1: isolierter Worker-Prozess
   (`engine_worker.rs`, Supervisor mit Neustart); `SttEngine`-Trait hält die
   Tür für ein CUDA-Sidecar offen.
2. ~~**Paste ist einfach** (kein Clipboard-Restore, keine Terminal-Erkennung,
   kein Delayed-Render)~~ ✅ Sprint 3.2: Paste-Transaktion als Eigenimplementierung
   nach Handys `paste_tx`-Blaupause (IP-Direktive beachtet — Konzept studiert,
   kein Code übernommen); Details und abgelehnte Alternativen in ADR 0003.
   Noch offen aus specs 4.6: Paste-Chord pro App merken (statt nur Terminal-
   Liste) — Backlog.
3. ~~**Kein VAD** in der Aufnahme~~ ✅ Sprint 3: Silero-VAD als Nachbearbeitungs-
   Qualitätstor (ADR 0004: Gates ja, Sample-Trim nein — Kürzung verschlechterte
   Parakeet messbar; Auto-Stop verworfen). Chunking an Sprechpausen (~30 s /
   Max 40 / Min 5) bewusst auf M3 verschoben — Parakeet hat kein Fensterlimit,
   die Silero-Segmentbasis steht bereit (specs 4.9 dockt dort an).
4. **Aufnahmemodus je Hotkey fest verdrahtet:** Cursor = Halten, Hand-off =
   Toggle (Sprint 3.1+). Specs 4.3 sieht „Toggle"/„Hold-or-Toggle" als
   **wählbare** Modi vor — ein Setting dafür (auch pro Hotkey) folgt später.
5. **Wörterbuch** ✅ Sprint 2 (`dictionary.rs`); Fuzzy/Phonetik bleibt v2
   (Kölner Phonetik, NICHT ASCII-Soundex — Handys Fuzzy ist für Deutsch ungeeignet).
6. **Historie** ✅ Sprint 2 (`history.rs`, SQLite inkl. applied_rules).

## Eval (specs Abschnitt 6)

Ablauf, Clipliste und Aufnahme-Anleitung: [`eval/README.md`](eval/README.md).
Stand: WER-Runner ✅ (`--selftest <ordner>`), BT700-Korpus gemessen
(11 Clips, **WER final 1,9 %** nach Regel-Ableitung; Reports in
`eval/reports/`). Nach VAD-Einbau am 22.11. neu gemessen und unverändert
bestätigt (45/2413, Transkripte bytengleich — Gates-only, s. ADR 0004 und
`eval/reports/nach-vad-run.txt`). Offen: Fachbegriff-Quote als eigene Metrik.
Der Headset-Vergleich über WL5024-Clips vom Arbeitslaptop ist vom Nutzer
abgesagt (22.11.2026).

## Bekannte Issues

- **Kein Hotkey bei Fokus auf elevated/Admin-Fenstern:** Windows liefert
  Tastenereignisse an Low-Level-Hooks normaler Integrität nicht, während ein
  Fenster mit erhöhten Rechten den Fokus hat (UIPI). Verhalten identisch zu
  Whispering/Handy (OpenWhispr #2059). Diktieren in normalen Fenstern
  (Editor/Browser/VSCode) funktioniert. Eine echte Lösung wäre ein kleiner
  elevated Helper-Prozess — Backlog, nicht M0.
- **WebView2-White-Screen auf diesem Rechner (gelöst):** Die GPU-Beschleunigung
  des WebView2 crasht hier still (Multi-GPU: 7900X3D-iGPU + RTX 4090, die beim
  Start gleichzeitig das Vulkan-Modell lädt) und lässt das Fenster weiß erscheinen.
  Fix: `additionalBrowserArgs: "--disable-gpu"` (plus Tauri-Defaults) für beide
  Fenster in `tauri.conf.json`. Für die winzige UI kostet das nichts. NICHT
  entfernen, ohne es auf dem Zielsystem zu verifizieren.

## Vorbilder & Lizenzen

`NOTICE.md` führt jede Übernahme (Handy, OpenWhispr, Blitztext — alle MIT).
Bei Code-Übernahme: Attribution in NOTICE.md ergänzen.
