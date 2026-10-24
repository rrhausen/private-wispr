# AGENTS.md — Arbeitsanleitung für Coding-Agenten (Private Wispr)

Dieses Repo wird maßgeblich mit AI-Coding-Agenten entwickelt. Diese Datei ist
der verbindliche Einstieg. **Die Spezifikation ist die Wahrheit für Verhalten
und Architektur:** [`specs/2026-10-24-private-wispr-spezifikation.md`](specs/2026-10-24-private-wispr-spezifikation.md)
(inkl. Entscheidungs-Log am Ende — Entscheidungen dort nachlesen, nicht neu
diskutieren).

## Projekt in einem Satz

Private, deutsch-first Diktat-App für Windows 11: Taste halten, sprechen,
loslassen — Text erscheint am Cursor. Lokale STT via transcribe.cpp (GGUF,
Vulkan), Tauri 2 (Rust + React/TS). Deutsch zuerst, Englisch second.

## Architektur (Stand M0)

```
src-tauri/src/
  lib.rs        Orchestrierung: AppState, PTT-Flow, Overlay-Steuerung, Tray
  hotkey.rs     PTT-Hotkeys (handy-keys, Low-Level-Hook, key-up) — frei
                konfigurierbar (M1): Key-Capture, Konflikt-Validierung,
                Umregistrierung zur Laufzeit mit Rollback
  recording.rs  Aufnahme (handy-recorder: WASAPI → 16 kHz mono f32) mit
                Disk-Spooling (WAV wächst auf Platte, kein Längen-Limit)
  stt.rs        Engine: transcribe.cpp (Parakeet TDT v3 Q8, Backend::Auto→Vulkan)
  model_download.rs  In-App-Modell-Download (ureq, Progress-Events, SHA256,
                abbrechbar, Resume via Range-Requests)
  paste.rs      Einfügen (M0: Clipboard + SendInput Strg+V)
  feedback.rs   Dezenter Klick beim Aufnahme-Start (WAV-Synthese + PlaySoundW;
                abschaltbar). rodio geht nicht: cpal-Links-Konflikt mit handy-recorder
  handoff.rs    Hand-off-Diktat in Tagesdateien; Ablageort konfigurierbar
  settings.rs   JSON-Settings (Felder mit Defaults statt Migration)
  selftest.rs   Headless-Modus `--selftest [wav]` (Eval-Keimzelle)
src/            Frontend: main.tsx→App.tsx (Tabs Übersicht/Einstellungen),
                settings.tsx (Hotkeys, Ablageort, Mikrofon, Overlay, Sound,
                Autostart, Modell-Download), overlay.tsx (Pille)
```

**Datenfluss:** Hotkey Pressed → `recording::start` → Overlay „recording" →
Released → `recording::stop` → Thread: `stt::transcribe` → `paste::paste_text`
→ Overlay „done". Events: `pw://status`, `pw://overlay`, `pw://level`,
`pw://last`, `pw://mic-fallback`, `pw://hotkey-captured`, `pw://model-download`.

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

# Vollständiger Transkriptions-Test mit WAV (16 kHz mono):
target/debug/private-wispr.exe --selftest path/to/test.wav

# Frontend-Build
bun run build
```

**CMake** liegt portabel unter
`%LOCALAPPDATA%\Microsoft\WinGet\Packages\Kitware.CMake_*\bin` — bei
Build-Fehlern („cmake not found") diesen Pfad in PATH aufnehmen.
**Modell** liegt unter `%LOCALAPPDATA%\PrivateWispr\models\` (Download:
`scripts/download-model.ps1`, 705 MB, SHA256-geprüft).

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
- **Keine stillen Fallbacks** (spezifiziertees Vertrauenskonzept): Fehler
  melden, nie heimlich auf CPU/Cloud/anders umschalten — Ausnahme: Engine-internes
  GPU→CPU ist ok, muss aber im Status sichtbar bleiben.
- **Audio/Stimmdaten niemals committen** (`.gitignore` schließt `*.wav`,
  `eval/audio/` aus). Ground-Truth-Texte (`*.txt`) dagegen schon.
- **Modell-Dateien niemals committen** (`.gguf`, `.onnx`).
- Neue Backend-Fähigkeiten: Event `pw://…` + Frontend-Zustand, kein Polling.
- Tests: Rust-Unit-Tests (`cargo test`) für Regel-Logik; E2E über `--selftest`
  mit festen WAVs unter `eval/audio/` (nicht im Repo, lokal erzeugt).

## Wichtigste bewusste Abweichungen/Offenpunkte (M0 → M1)

1. **Engine läuft in-process** — Worker-Prozess-Isolation (Handy-Muster
   `engine_supervisor`) ist M1-Pflicht, bevor Features auf die Engine gehen.
2. **Paste ist einfach** (kein Clipboard-Restore, keine Terminal-Erkennung,
   kein Delayed-Render) — M1-Sprint 3: Paste-Transaktion als Eigenimplementierung
   nach Handys `paste_tx`-Blaupause (IP-Direktive beachten, keine Code-Übernahme).
3. **Kein VAD** in der Aufnahme (Policy-Parameter folgen mit M1-Regel-Pipeline).
4. **Wörterbuch existiert noch nicht** — M1: YAML-Regeln + Unicode-Normalisierung
   + Profile (specs 4.5); Fuzzy/Phonetik erst v2 (Kölner Phonetik, NICHT
   ASCII-Soundex — Handys Fuzzy ist für Deutsch ungeeignet).
5. **Historie** noch nicht implementiert (M1, SQLite-Schema in specs 4.8).

## Eval (specs Abschnitt 6)

`eval/audio/` (lokal, nicht im Repo) enthält Clips + gleichnamige `.txt`
Ground-Truths, darunter **mindestens einen ≥10-Minuten-Clip** (Brain-Dump-
Regressionstest gegen das Whispering-Fehlerbild). `--selftest` misst
Transkriptionszeit und RTF; WER/Fachbegriff-Quote kommen mit dem
Regel-Modul (M1) in denselben Einstiegspunkt.

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
