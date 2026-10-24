# Private Wispr

**Private, deutsch-first Diktat- und Transkriptions-App für Windows 11.**

Taste halten, sprechen, loslassen — der Text erscheint am Cursor. Spracherkennung läuft lokal
auf der GPU (transcribe.cpp, GGUF-Modelle, Vulkan). Cloud-LLMs optional (OpenAI-kompatibel,
eigene Keys). Kein Account, kein Backend, keine Telemetrie.

**Status: Spezifikationsphase** — die Spezifikation liegt unter
[`specs/2026-10-24-private-wispr-spezifikation.md`](specs/2026-10-24-private-wispr-spezifikation.md)
(entstanden aus einem Grilling-Interview am 24.10.2026).

## Eckpunkte

- **Stack:** Tauri 2 (Rust + React/TypeScript), Windows 11, native (kein WSL2 erforderlich)
- **Engine:** transcribe.cpp im isolierten Worker-Prozess; `SttEngine`-Trait hält CUDA-Tür offen
- **Modelle:** Parakeet-TDT-0.6B-v3 (Schnell/Diktat) · Voxtral Small 24B (Präzise/Batch) ·
  Whisper large-v3-turbo (Fallback)
- **Wörterbuch:** deterministische deutsche Fachwort-Regeln (Unicode-sicher, Profile) +
  Fachwörter direkt im Modell-Decode
- **Modi:** Exakt & Bereinigt als editierbare Prompts, Hotkey pro Modus
- **Meilensteine:** M0 Walking Skeleton → M1 Produktkern → M2 Modi & Cloud → M3 Lange Formate
  (Details in der Spezifikation, Abschnitt 8)

## Vorbilder / verwendete Open-Source-Arbeiten

Siehe [`NOTICE.md`](NOTICE.md) — wesentliche Bausteine und Blaupausen stammen aus
[Handy](https://github.com/cjpais/Handy), [OpenWhispr](https://github.com/OpenWhispr/openwhispr)
und [Blitztext](https://github.com/cmagnussen/blitztext-app) (alle MIT).
