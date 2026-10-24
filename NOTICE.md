# NOTICE — Attributionen

Private Wispr baut auf folgenden Open-Source-Arbeiten auf (alle MIT-lizenziert).
Übernommener Code wird in den Dateien zusätzlich lokal attributioniert.

## Übernommener Code / Abhängigkeiten

- **Handy** — https://github.com/cjpais/Handy — © CJ Pais
  - `paste_tx/windows.rs` (Paste-Transaktion: Delayed-Render-Clipboard, Read-Receipt,
    Clipboard-Restore, Terminal-Erkennung) — Übernahme mit Attribution
  - Crates `handy-keys` (PTT-Hotkeys), `handy-recorder` (Audio) — als Abhängigkeit oder Fork
  - Muster/Blaupausen: `engine_supervisor` (Worker-Isolation), Modellkatalog-Konzept,
    VAD-Smoothing-Parameter, Pausen-Chunking (30 s Ziel / 40 s Max / 5 s Min)
- **transcribe.cpp** (ankündigt von Mozilla AI, Repo `handy-computer/transcribe.cpp`)
  mit Rust-Bindings `transcribe-cpp` — STT-Engine, GGUF-Modelle (Whisper, Parakeet,
  Canary, Voxtral, Qwen3-ASR, …); GGUF-Konvertierungen überwiegend vom HF-User
  `handy-computer`

## Konzept-Blaupausen (kein Code)

- **OpenWhispr** — https://github.com/OpenWhispr/openwhispr — Wörterbuch-Prompt-Budgets
  (~223-Token-Decoder-Fenster bei Whisper), Sidecar-Betriebsmuster, Auto-Lerner-Idee (v2),
  windows-key-listener-Muster (key-up-PTT)
- **Blitztext** — https://github.com/cmagnussen/blitztext-app — Prompt-Querschnittsregeln
  (Selbstkorrekturen, Injektionsschutz, geschützte Begriffe), Text-Fitting-Regelwerk,
  Fokus-Check-Konzept, „kein stiller Fallback", Meeting-/Zusammenfassungs-Struktur

## Recherche-Grundlage

- `Dictation-SW-Perplexity-Research_2026-10-24.md` (Perplexity-Recherche des Nutzers)
- SOTA-Deutsch-Modellrecherche vom 24.10.2026 (ASR-Leaderboard-DE:
  arxiv.org/html/2510.06961v4; NVIDIA Parakeet/Canary-Paper: arxiv.org/html/2509.14128v1)
