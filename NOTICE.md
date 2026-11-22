# NOTICE — Attributionen

**IP-Direktive des Projekts (24.10.2026): Es wird kein Code 1:1 aus
Drittprojekten übernommen — auch nicht aus MIT-lizenzierten.** Die folgenden
Projekte dienen als Konzept- und Verhaltens-Blaupausen; alle Implementierungen
in diesem Repo sind Eigenentwicklungen. Verwendete Bibliotheken (Crates,
npm-Pakete) werden gemäß ihrer Lizenzen als Abhängigkeiten genutzt.

## Konzept-Blaupausen (kein Code übernommen)

- **Handy** (cjpais, MIT) — Muster für: Paste-Transaktion (Delayed-Render,
  Read-Receipt, Clipboard-Restore, Terminal-Erkennung), Worker-Isolation,
  Modellkatalog, VAD-Smoothing, Pausen-Chunking, Hotkey-Manager-Thread
- **OpenWhispr** (MIT) — Wörterbuch-Prompt-Budgets, Sidecar-Betrieb,
  Auto-Lerner-Idee, windows-key-listener-Muster (key-up-PTT)
- **Blitztext** (cmagnussen, MIT) — Prompt-Querschnittsregeln, Text-Fitting,
  Fokus-Check, „kein stiller Fallback", Meeting-Struktur; **UI-Design-Prinzipien**
  (M1: ruhige Nachtseite, Karten mit Sektionskopf darüber, ein Lilac-Akzent,
  Kapsel-Buttons, Public Sans) — als Konzept studiert und eigenständig in CSS
  umgesetzt, keine Übernahme von Stylesheet-/SwiftUI-Code
- **Epicenter/Whispering** (AGPL; nur Verhaltensanalyse) — Mikrofon-Handling

## Abhängigkeiten (normaler Gebrauch)

- transcribe.cpp (`transcribe-cpp`, Mozilla AI / handy-computer) — STT-Engine
- handy-keys, handy-recorder (cjpais) — Hotkeys/Audio
- GGUF-Konvertierungen überwiegend vom HF-User `handy-computer`
- **Mitgegebenes Modell:** `silero-vad-v6.2-F32.gguf` (handy-computer/
  silero-vad-v6.2-gguf, MIT) — GGUF-Konvertierung von snakers4/silero-vad
  (Silero Team, MIT); wird unverändert in den Paketen (ZIP/NSIS)
  weiterverbreitet, Download SHA256-gepinnt (scripts/download-vad-model.ps1)

## Recherche-Grundlage

- `Dictation-SW-Perplexity-Research_2026-10-24.md` (Perplexity-Recherche des Nutzers)
- SOTA-Deutsch-Modellrecherche vom 24.10.2026 (ASR-Leaderboard-DE:
  arxiv.org/html/2510.06961v4; NVIDIA Parakeet/Canary: arxiv.org/html/2509.14128v1)
