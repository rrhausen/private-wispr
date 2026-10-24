# eval/ — Qualitäts-Gurt (Keimzelle, M0)

Vollständiger Plan: specs Abschnitt 6. Dieser Ordner ist das Zuhause für das
deutsche Testkorpus und den Harness. **Audio-Dateien liegen NICHT im Repo**
(Stimmdaten, `.gitignore` schließt `eval/audio/` aus) — nur Ground-Truth-Texte
und der Harness selbst werden versioniert.

## Testkorpus (ab M1 vollständig)

```
eval/audio/           # lokal erzeugt, nie committen
  01-alltagsdiktat.wav     + 01-alltagsdiktat.txt   (Ground-Truth)
  02-fachbegriffe-it.wav   + 02-fachbegriffe-it.txt  (Wörterbuch-Begriffe)
  03-zahlen-daten.wav      + 03-zahlen-daten.txt     (Zahlen, Einheiten, Datumsangaben)
  04-umlaut-komposita.wav  + 04-umlaut-komposita.txt (ß/Umlaute, Wortgrenzen)
  05-braindump-10min.wav   + 05-braindump-10min.txt  (≥10 Minuten — Regressionstest
                                                       gegen das Whispering-Fehlerbild
                                                       „Abbruch nach ~30 s")
```

Clips: 16 kHz mono WAV (die App-Aufnahme erzeugt genau das Format), mit der
eigenen Stimme aufgenommen, Ground-Truth als wortgenaues Transkript.

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
| WER gesamt | M1 (Ground-Truth-Diff) |
| Fachbegriff-Trefferquote | M1 (mit Wörterbuch-Modul) |
| Schädliche Fehlersetzungen | M1 (Regel-Diff gegen Ground-Truth) |
| Parakeet-v3-Deutsch-EOS-Check | M1 (Clip 05 + Einzelsatz-Clips) |
