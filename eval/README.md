# eval/ — Qualitäts-Gurt (Keimzelle, M0)

Vollständiger Plan: specs Abschnitt 6. Dieser Ordner ist das Zuhause für das
deutsche Testkorpus und den Harness. **Audio-Dateien liegen NICHT im Repo**
(Stimmdaten, `.gitignore` schließt `eval/audio/` aus) — nur Ground-Truth-Texte
und der Harness selbst werden versioniert.

## Testkorpus (M1 — Aufnahme läuft)

Aufnahmen mit zwei echten Geräten; das Dateipräfix nennt das Gerät
(`bt700-*` = Plantronics BT700 am Haupt-Rechner, `wl5024-*` = DELL Pro
Plus Wireless ANC WL5024 am Arbeitslaptop). Audio bleibt lokal
(`.gitignore` schließt `eval/audio/` aus), nur die gleichnamigen
`.txt`-Ground-Truths werden committet.

| #  | Name (ohne Präfix)   | Dauer    | Inhalt |
|----|----------------------|----------|--------|
| 01 | mail                 | ~1 min   | typische Arbeits-E-Mail (Anrede … Gruß) |
| 02 | chat                 | ~30 s    | kurze Chat-Nachrichten, lockerer Ton |
| 03 | bericht              | ~2 min   | formeller Berichtsabschnitt, Fachregister |
| 04 | notiz                | ~30 s    | Stichpunkte, Telegrammstil |
| 05 | zahlen               | ~1 min   | Beträge, Prozent, Daten, Uhrzeiten, Nummern |
| 06 | namen                | ~1 min   | Personen-, Firmen-, Straßen-, Produktnamen |
| 07 | mix                  | ~1–2 min | Namen + Zahlen + Begriffe im Fließtext |
| 08 | natuerlich           | ~1–2 min | spontan, mit Füllwörtern & Selbstkorrekturen |
| 09 | fachbegriffe1        | ~1 min   | 15–20 eigene Fachbegriffe in Sätzen |
| 10 | fachbegriffe2        | ~1 min   | 15–20 weitere, vermutlich „harte" Begriffe |
| 11 | braindump            | ≥10 min  | Monolog am Stück — Regression gegen das „Abbruch nach ~30 s"-Fehlerbild |

Dazu vom Arbeitslaptop (WL5024) dieselben Kategorien kompakt, bevorzugt
01/02/05/06/09 — plus echte Alltagsdiktate aus dem Betrieb.

### Aufnahme (App ab 0.1.4, Schalter „Diktat-Audio aufbewahren")

1. Einstellungen → „Diktat-Audio aufbewahren" aktivieren, Headset als
   Mikrofon wählen. Kurzen Test (10 s) fahren und prüfen, dass die WAV
   unter `%APPDATA%\PrivateWispr\audio\JJJJ-MM-TT\` liegt.
2. Je Clip: Diktat-Hotkey wie im Alltag drücken und sprechen (Cursor-Modus
   genügt). Freies Sprechen statt Ablesen — nur zahlen/namen/fachbegriffe
   dürfen von einer Liste gelesen werden.
3. WAV in `<gerät>-<nr>-<name>.wav` umbenennen (z. B. `bt700-05-zahlen.wav`)
   und in `eval/audio/` kopieren; daneben gleichnamige `.txt` mit dem
   Soll-Text: Wortlaut wie gesprochen, aber ohne Füllwörter; Namen und
   Begriffe in der Schreibweise, die am Ende stehen soll; Zahlen in der
   Zielform (Ziffern oder Wörter — der Runner normalisiert das).
4. Bei `08-natuerlich`: Selbstkorrekturen im Wortlaut mitdokumentieren
   (Ist-Zustand messen; das Aufräumen ist ein späterer Pipeline-Schritt).

Alternativ (v. a. für den 10-Minuten-Braindump, 10 min Taste halten ist
unpraktisch): Direktaufnahme mit beliebigem Recorder — der Selftest
dekodiert via ffmpeg jedes Format intern auf 16 kHz mono.

## Harness

M0 liefert den Einstiegspunkt über den Headless-Selftest der App:

```bash
# Engine-Init + Modell-Load + Backend/GPU-Report:
src-tauri/target/debug/private-wispr.exe --selftest

# Transkription mit Zeitmessung und Echtzeitfaktor:
src-tauri/target/debug/private-wispr.exe --selftest eval/audio/01-alltagsdiktat.wav
```

Ausgabe ist JSON (`transcription_ms`, `total_ms`, `realtime_factor`, `text`) —
direkt weiterverarbeitbar für Vergleiche (`--model X` gegen `--model Y` folgt
mit dem Katalog in M1/M2).

## Metriken (Roadmap)

| Metrik | Stand |
|---|---|
| Latenz Modell (transcription_ms, RTF) | ✅ M0 via `--selftest` |
| Latenz E2E (Loslassen → eingefügt) | M1 (Hooks im PTT-Flow) |
| WER gesamt (normalisiert, roh + final) | ✅ `--selftest <verzeichnis/>` (22.11.2026: 4,8% → **2,2%** nach Regel-Ableitung) |
| Schädliche Fehlersetzungen | ✅ Ersetzungs-Topliste im Batch-Report |
| Fachbegriff-Trefferquote | offen (aus Substitutionsliste ableitbar) |
| Parakeet-v3-Deutsch-EOS-Check | ✅ 10,5-Minuten-Braindump ohne Abbruch, WER 1,2%, RTF 0,008 |

Messreports liegen in `eval/reports/` (baseline/nach-regeln/final).
