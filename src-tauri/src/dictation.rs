//! Diktat-Verarbeitung (GLOSSARY.md): der stille Vorgang von der beendeten
//! Aufnahme bis zum fertigen Text — Wörterbuch in den Decode, transkribieren,
//! Regeln anwenden, Text fügen, Audio verwerten.
//!
//! Tiefe durch eine kleine Schnittstelle (specs-Architektur, „Deep Module"):
//! App (`on_ptt_up`) und Selftest (Einzel-WAV wie Batch) rufen exakt diese
//! Funktion — der Selftest misst damit per Konstruktion die Pipeline, die die
//! App nutzt, nicht eine hand-gewebte Kopie. Alle Abhängigkeiten (Engine,
//! Wörterbuch, Naht-Kontext, Archiv-Entscheidung) werden injiziert; das Modul
//! hat keine Nebenwirkungen nach außen (kein Overlay, kein Event, keine
//! Historie) — die liegen beim Aufrufer.
//!
//! Audio-Politik (specs 4.8) an genau einem Ort, auf JEDEM Pfad:
//! * Transkribiert → archivieren (wenn aktiviert) oder löschen
//! * Zu kurz       → löschen (kein Archiv-Wert in <0,3-s-Blips; früher leakte
//!                   dieser Pfad das Spool-WAV still nach %TEMP%)
//! * Engine-Fehler → WAV bleibt für die Fehlersuche liegen, laut geloggt

use std::path::{Path, PathBuf};

use crate::dictionary::{AppliedRule, Dictionary};
use crate::fitting::SeamContext;
use crate::recording::{OutputMode, SpooledRecording};
use crate::stt::SttEngine;

/// Aufnahmen unter dieser Schwelle gelten als versehentlich (Blitztext-Gate).
pub const MIN_RECORDING_SECS: f64 = 0.3;

/// Ergebnis der Diktat-Verarbeitung. „Zu kurz" ist kein Fehler, sondern ein
/// normaler Fall mit eigener Anzeige; bei „Fehlgeschlagen" bleibt das Audio
/// für die Fehlersuche liegen (Pfad steht im Log).
#[derive(Clone, Debug)]
pub enum DictationOutcome {
    Transcribed {
        /// Rohtranskript vor den Regeln (Nachvollziehbarkeit, specs 4.8).
        raw: String,
        /// Endtext nach Regel-Pipeline und Text-Fitting.
        final_text: String,
        /// Angewandte Wörterbuch-Regeln.
        applied: Vec<AppliedRule>,
        /// Version der Wörterbuch-Datei beim Diktat.
        dictionary_version: String,
        audio_secs: f64,
        transcription_ms: u128,
        /// Gesamtzeit der Pipeline inkl. Regeln/Fitting (Eval-Kennzahl).
        pipeline_ms: u128,
    },
    TooShort {
        duration_secs: f64,
    },
    Failed(String),
}

/// Ein beendetes Diktat verarbeiten. Siehe Modul-Doku für die Pfade und die
/// Audio-Politik; Nebenwirkungen nach außen hat diese Funktion keine.
pub fn process(
    spool: &SpooledRecording,
    mode: OutputMode,
    engine: &mut dyn SttEngine,
    dict: &Dictionary,
    seam: SeamContext,
    archive_audio: bool,
) -> DictationOutcome {
    if spool.duration_secs < MIN_RECORDING_SECS {
        dispose_wav(&spool.wav_path, mode, false);
        return DictationOutcome::TooShort { duration_secs: spool.duration_secs };
    }

    let vocabulary = dict.vocabulary();
    if !vocabulary.is_empty() {
        log::info!("Decode mit {} Fachwörtern aus dem Wörterbuch", vocabulary.len());
    }
    let t0 = std::time::Instant::now();
    let result = match engine.transcribe(&spool.samples, vocabulary) {
        Ok(r) => r,
        Err(e) => {
            // specs 4.8: Bei Transkriptions-Fehler bleibt das WAV für die
            // Fehlersuche liegen — bewusst und sichtbar geloggt, kein Leak.
            log::warn!("Audio aufbewahrt für Fehlersuche: {}", spool.wav_path.display());
            return DictationOutcome::Failed(e.to_string());
        }
    };
    let outcome = dict.apply(&result.text);
    if !outcome.applied.is_empty() {
        log::info!("Wörterbuch angewandt: {:?}", outcome.applied);
    }
    let final_text = crate::fitting::fit(&outcome.text, &seam);
    let pipeline_ms = t0.elapsed().as_millis();
    log::info!(
        "Transkript ({} s Audio, {} ms): {}",
        spool.duration_secs,
        result.transcription_ms,
        result.text
    );

    // Audio verwerten erst NACH erfolgreicher Transkription (specs 4.8).
    dispose_wav(&spool.wav_path, mode, archive_audio);

    DictationOutcome::Transcribed {
        raw: result.text,
        final_text,
        applied: outcome.applied,
        dictionary_version: dict.version.clone(),
        audio_secs: spool.duration_secs,
        transcription_ms: result.transcription_ms,
        pipeline_ms,
    }
}

/// Das Spool-WAV eines Diktats archivieren oder löschen. Archiv (wenn
/// aktiviert): `%APPDATA%\PrivateWispr\audio\JJJJ-MM-TT\HH-MM-SS <mode>.wav`
/// — verschieben statt kopieren, %TEMP% räumt sich sonst nie auf.
fn dispose_wav(wav_path: &Path, mode: OutputMode, archive: bool) {
    if archive {
        match archive_dir() {
            Ok(dir) => {
                let dest = dir.join(format!(
                    "{} {}.wav",
                    chrono::Local::now().format("%H-%M-%S"),
                    match mode {
                        OutputMode::Cursor => "cursor",
                        OutputMode::Handoff => "handoff",
                    }
                ));
                if std::fs::rename(wav_path, &dest).is_ok() {
                    log::info!("Audio archiviert: {}", dest.display());
                } else if let Err(e) = std::fs::copy(wav_path, &dest)
                    .and_then(|_| std::fs::remove_file(wav_path))
                {
                    // Andere Laufwerke/Partitionen: rename geht nicht — kopieren.
                    log::warn!("Audio nicht archivierbar ({e}): {}", wav_path.display());
                } else {
                    log::info!("Audio archiviert (kopiert): {}", dest.display());
                }
            }
            Err(e) => log::warn!(
                "Archiv-Ordner nicht anlegbar ({e}) — Audio bleibt: {}",
                wav_path.display()
            ),
        }
        return;
    }
    if let Err(e) = std::fs::remove_file(wav_path) {
        log::warn!("Spool-WAV nicht löschbar ({e}): {}", wav_path.display());
    }
}

fn archive_dir() -> std::io::Result<PathBuf> {
    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PrivateWispr")
        .join("audio")
        .join(chrono::Local::now().format("%Y-%m-%d").to_string());
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministische Engine für Pipeline-Tests (dritter Adapter neben
    /// In-Process-Engine und Worker — „die Schnittstelle ist die Test-Fläche").
    struct FakeEngine {
        text: &'static str,
        fail: bool,
        info: crate::stt::EngineInfo,
    }

    impl FakeEngine {
        fn ok(text: &'static str) -> Self {
            Self { text, fail: false, info: fake_info() }
        }
        fn failing() -> Self {
            Self { text: "", fail: true, info: fake_info() }
        }
    }

    fn fake_info() -> crate::stt::EngineInfo {
        crate::stt::EngineInfo {
            loaded: true,
            model_name: "fake".into(),
            ..Default::default()
        }
    }

    impl SttEngine for FakeEngine {
        fn transcribe(
            &mut self,
            samples: &[f32],
            _vocabulary: &[String],
        ) -> anyhow::Result<crate::stt::TranscriptResult> {
            if self.fail {
                anyhow::bail!("Engine-Fehler (Test)");
            }
            Ok(crate::stt::TranscriptResult {
                text: self.text.to_string(),
                language: Some("de".into()),
                transcription_ms: samples.len() as u128 / 16,
            })
        }
        fn info(&self) -> &crate::stt::EngineInfo {
            &self.info
        }
    }

    fn dict_mit_regel() -> Dictionary {
        // Reine compile-Pipeline ohne Dateisystem (wie die dictionary-Tests).
        Dictionary::compile(serde_yaml::from_str::<crate::dictionary::DictionaryFile>(
            "locale: de-DE\nterms:\n  - canonical: SharePoint\n    aliases: [Scherpoint]",
        )
        .unwrap())
    }

    /// Echtes Temp-WAV anlegen (16 kHz mono f32), damit die Entsorgung über
    /// echte Dateien getestet wird.
    fn temp_wav(samples: usize) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pw-dictation-test-{}.wav",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..samples {
            writer.write_sample(0.0f32).unwrap();
        }
        writer.finalize().unwrap();
        path
    }

    fn spool(samples: usize, secs: f64) -> SpooledRecording {
        SpooledRecording {
            samples: vec![0.0; samples],
            wav_path: temp_wav(samples),
            duration_secs: secs,
        }
    }

    /// Pflicht-Test (a): Pipeline-Ordnung — roh bleibt roh, Filler raus,
    /// Wörterbuch-Regel greift, Fitting läuft, Zeiten werden gemessen.
    #[test]
    fn pipeline_ordnung_roh_regeln_fitting() {
        let s = spool(16_000, 1.0);
        let mut engine = FakeEngine::ok("äh hallo Scherpoint welt");
        let dict = dict_mit_regel();
        let out = process(&s, OutputMode::Cursor, &mut engine, &dict, SeamContext::none(), false);
        let DictationOutcome::Transcribed {
            raw, final_text, applied, audio_secs, pipeline_ms, ..
        } = out else {
            panic!("Expected Transcribed, got {out:?}");
        };
        assert_eq!(raw, "äh hallo Scherpoint welt");
        assert!(final_text.contains("SharePoint"), "Regel muss greifen: {final_text}");
        assert!(!final_text.contains("Scherpoint"));
        assert!(!final_text.contains("äh"), "Filler muss raus: {final_text}");
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].canonical, "SharePoint");
        assert_eq!(applied[0].matched, "Scherpoint");
        assert_eq!(audio_secs, 1.0);
        assert!(pipeline_ms > 0);
        assert!(!s.wav_path.exists(), "Audio muss entsorgt sein (Archiv aus)");
    }

    /// Pflicht-Test (b): Zu kurz → TooShort-ERGEBNIS und das WAV wird
    /// trotzdem entsorgt (früher leakte genau dieser Pfad nach %TEMP%).
    #[test]
    fn zu_kurz_entsorgt_trotzdem() {
        let s = spool(1600, 0.1);
        let mut engine = FakeEngine::ok("egal");
        let out = process(&s, OutputMode::Handoff, &mut engine, &dict_mit_regel(), SeamContext::none(), true);
        assert!(matches!(out, DictationOutcome::TooShort { duration_secs } if (duration_secs - 0.1).abs() < 1e-9));
        assert!(!s.wav_path.exists(), "Zu-kurz-WAV muss entsorgt werden — kein Leak");
    }

    /// Pflicht-Test (c): Engine-Fehler → Failed statt stiller Abbruch; das
    /// WAV bleibt bewusst für die Fehlersuche liegen (specs 4.8).
    #[test]
    fn engine_fehler_wav_bleibt_fuer_fehlersuche() {
        let s = spool(16_000, 1.0);
        let mut engine = FakeEngine::failing();
        let out = process(&s, OutputMode::Cursor, &mut engine, &dict_mit_regel(), SeamContext::none(), true);
        let DictationOutcome::Failed(msg) = out else {
            panic!("Expected Failed, got {out:?}");
        };
        assert!(msg.contains("Engine-Fehler"));
        assert!(s.wav_path.exists(), "WAV muss für die Fehlersuche bleiben");
        std::fs::remove_file(&s.wav_path).ok();
    }

    /// Archiv-Politik: Mit aktiviertem Archiv landet das WAV unter dem
    /// Tagesordner, benannt nach Uhrzeit + Modus (Eval-Clips aus dem Betrieb).
    #[test]
    fn archivierung_benennt_zeit_und_modus() {
        let s = spool(16_000, 1.0);
        let mut engine = FakeEngine::ok("test");
        let out = process(&s, OutputMode::Handoff, &mut engine, &dict_mit_regel(), SeamContext::none(), true);
        assert!(matches!(out, DictationOutcome::Transcribed { .. }));
        assert!(!s.wav_path.exists(), "Quell-WAV ist weg (verschoben)");
        // Die archivierte Datei heißt "... handoff.wav" und wird wieder entfernt.
        let dir = archive_dir().unwrap();
        let treffer: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.ends_with("handoff.wav") && n.starts_with(|c: char| c.is_ascii_digit())
            })
            .collect();
        assert!(!treffer.is_empty(), "Archiv-Datei muss existieren");
        for f in treffer {
            std::fs::remove_file(f.path()).ok();
        }
    }
}
