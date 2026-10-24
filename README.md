# Private Wispr

Private, deutsch-first Diktat-App für Windows 11: **Taste halten, sprechen,
loslassen — der Text erscheint am Cursor.** Spracherkennung läuft vollständig
lokal auf der GPU (transcribe.cpp, Vulkan); Audio und Transkripte verlassen
diesen Rechner nie. Kein Account, keine Cloud, keine Telemetrie.

![Status](https://img.shields.io/badge/Stand-M1_Sprint_1-8337A8) ![Plattform](https://img.shields.io/badge/Plattform-Windows_11-0078D4)

## Installation

### Variante A — Setup (empfohlen)

1. Auf dem Entwicklungsrechner einmalig bauen (s. unten), dann den Installer aus
   `src-tauri\target\release\bundle\nsis\` auf den Ziel-Laptop kopieren:
   `Private-Wispr_0.1.0_x64-setup.exe`
2. Installer ausführen → installiert Startmenü-Eintrag + Deinstallation.
3. App starten → in **Einstellungen → Modell** den „Modell herunterladen"-Button
   klicken (705 MB, SHA256-verifiziert, abbrechbar und fortsetzbar). Erst danach
   ist das Diktat bereit.

### Variante B — nur die exe (portabel)

Die einzelne `private-wispr.exe` aus `src-tauri\target\release\` auf den
Ziel-Laptop kopieren und direkt ausführen — keine Installation nötig (alle
UI-Assets sind in die exe eingebettet). Einstellungen und Modell liegen
unabhängig davon unter `%LOCALAPPDATA%\PrivateWispr\`.

Hinweise für beide Varianten:

- **Windows 11** mit WebView2 (ab Werk vorhanden) und einer **Vulkan-fähigen
  GPU** (NVIDIA/AMD/Intel ab ~Skylake; NVIDIA-Treiber bringt Vulkan mit).
  Ohne Vulkan läuft die Engine auf der CPU — deutlich langsamer, aber funktional.
- Modell, Einstellungen und Logs pro Rechner: `%LOCALAPPDATA%\PrivateWispr\`,
  App-Log: `%LOCALAPPDATA%\de.rrhausen.private-wispr\logs\private-wispr.log`.
- Autostart umschalten: **Einstellungen → Anzeige, Sound & Start** (Registry-
  Eintrag zeigt auf den Pfad der exe — bei portabler Variante den Ordner danach
  nicht mehr verschieben, sonst Autostart erneut aktivieren).

## Bedienung

| Aktion | So geht's |
|---|---|
| **Diktat am Cursor** | `Strg+Alt+Leertaste` halten, sprechen, loslassen — Text wird am Cursor eingefügt (Default, frei änderbar) |
| **Hand-off-Diktat** | `Strg+Alt+H` halten — Diktat landet als `## HH:MM`-Eintrag in der Tagesdatei `JJJJ-MM-TT.md` im Hand-off-Ordner |
| **Aufnahme abbrechen** | `Escape` während der Aufnahme |
| **Fenster öffnen** | Doppelklick auf das Tray-Icon oder Tray-Menü → „Private Wispr öffnen"; X minimiert in den Tray (App läuft weiter) |
| **Beenden** | Tray-Menü → „Beenden" |

**Einstellungen** (Tab im Hauptfenster):

- **Tastenkombinationen**: frei belegbar per „Ändern" → gewünschte Kombination
  drücken (auch nur Modifier wie Strg+Alt). Doppelbelegungen, nackte Tasten ohne
  Modifier (würden systemweit blocken) und Escape (Reserve für Abbruch) werden
  abgelehnt.
- **Hand-off-Ablageort**: Zielordner der Tagesdateien. Der Weg, Diktate an
  **elevated Apps** (Admin-Terminals, Agenten-IDEs) zu übergeben — Windows
  blockt dort injiziertes Strg+V (UIPI), Dateien gehen aber immer.
- **Mikrofon**: Gerät wählen; Bluetooth-Headsets mit Aufbau-Pause und
  Fallback-Kette (Gerät → Retry → System-Standard → erstes aktives Gerät).
- **Modell**: Download mit Fortschritt, Abbruch/Resume und SHA256-Prüfung in der App.
- **Overlay-Pille** (Zustand am unteren Bildschirmrand), **Start-Klick** und
  **Autostart** ein-/ausschaltbar.

**Bekannte Grenze:** Bei Fokus auf Fenstern mit Admin-Rechten (elevated)
bekommt die App keine Tastaturereignisse (Windows UIPI) — in normalen Fenstern
(Editor, Browser, VS Code) funktioniert alles. Für elevated Apps den
Hand-off-Modus nutzen.

## Entwicklung

```bash
bun install                  # Frontend-Abhängigkeiten
bun run tauri dev            # App im Dev-Modus (Hotkey aktiv, Rust-Watch)
bun run tauri build          # Release-Build → Installer + ZIP (s. Installation)
cd src-tauri && cargo test   # Unit-Tests
src-tauri/target/debug/private-wispr.exe --selftest   # Headless-Smoke-Test
```

Voraussetzungen: Rust, Bun/Node, CMake (portabler WinGet-Pfad genügt),
`VULKAN_SDK` (steht in `src-tauri/.cargo/config.toml`). Details, Konventionen
und Architektur: [`AGENTS.md`](AGENTS.md) · Spec:
[`specs/2026-10-24-private-wispr-spezifikation.md`](specs/2026-10-24-private-wispr-spezifikation.md).

## Technik (Kurzform)

Tauri 2 (Rust + React/TypeScript) · WASAPI-Aufnahme 16 kHz mono mit
Disk-Spooling (kein Längenlimit, Brain-Dump-tauglich) · transcribe.cpp mit
Parakeet TDT 0.6B v3 (GGUF, Q8) auf Vulkan · globale Hotkeys über
Low-Level-Hook mit key-up-Erkennung · Overlay ohne Fokusraub ·
Modell-Downloads von HuggingFace mit Hash-Verifikation. Lizenzen und
Blaupausen-Quellen: [`NOTICE.md`](NOTICE.md).

*Privates Projekt — Audio/Stimmdaten und Modelle werden nicht committet; das
Repo bleibt privat.*
