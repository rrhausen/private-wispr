//! Aufnahme über handy-recorder (WASAPI → 16 kHz mono f32).
//!
//! Disk-Spooling ab dem ersten Chunk (specs 4.2): Das WAV wächst kontinuierlich
//! auf der Platte; eine Brain-Dump-Session von 10+ Minuten hat kein Timeout.
//! M0 ohne VAD (VAD-Policy folgt mit der Regel-Pipeline in M1).

use std::io::BufWriter;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use handy_recorder::{AudioChunk, Channels, EndReason, Recorder, RecorderConfig, Sink};
use hound::{SampleFormat, WavSpec, WavWriter};
use log::warn;
use tauri::{AppHandle, Emitter};

/// Eine laufende Aufnahme. `recorder.stop()` liefert das Sink mit den Samples
/// zurück (Muster aus Handys managers/audio.rs).
pub struct ActiveRecording {
    recorder: Recorder<SpoolSink>,
    #[allow(dead_code)]
    pub started: Instant,
}

/// Ergebnis einer beendeten Aufnahme. `wav_path` wird ab M1 in die Historie
/// geschrieben (Audio-Archivierung optional).
pub struct SpooledRecording {
    pub samples: Vec<f32>,
    #[allow(dead_code)]
    pub wav_path: PathBuf,
    pub duration_secs: f64,
}

struct SpoolSink {
    writer: WavWriter<BufWriter<std::fs::File>>,
    wav_path: PathBuf,
    samples: Vec<f32>,
    app: Option<AppHandle>,
    chunk_counter: u32,
}

const SAMPLE_RATE: u32 = 16_000;

impl SpoolSink {
    fn finish(self) -> Result<SpooledRecording> {
        self.writer
            .finalize()
            .with_context(|| format!("WAV finalisieren fehlgeschlagen: {}", self.wav_path.display()))?;
        let duration_secs = self.samples.len() as f64 / SAMPLE_RATE as f64;
        Ok(SpooledRecording {
            samples: self.samples,
            wav_path: self.wav_path,
            duration_secs,
        })
    }
}

impl Sink for SpoolSink {
    fn process_chunk(&mut self, chunk: AudioChunk<'_>) {
        let real = &chunk.samples[..chunk.valid_frames];
        for s in real {
            let _ = self.writer.write_sample(*s);
        }
        self.samples.extend_from_slice(real);

        // Pegel fürs Overlay, gedrosselt (~alle 100 ms bei ~30-ms-Chunks).
        self.chunk_counter += 1;
        if let Some(app) = &self.app {
            if self.chunk_counter % 3 == 0 {
                let rms = rms_level(real);
                let _ = app.emit("pw://level", rms);
            }
        }
    }
}

fn rms_level(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: f32 = frame.iter().map(|s| s * s).sum();
    (sum / frame.len() as f32).sqrt().min(1.0)
}

fn open_recorder(device: Option<String>) -> Result<Recorder<SpoolSink>> {
    let config = RecorderConfig {
        device,
        channels: Channels::MixToMono,
        frames_per_chunk: None,
        ..RecorderConfig::speech()
    };
    Recorder::open_with_failure_handler(config, |err| {
        warn!("Recorder-Fehler: {err}");
    })
    .context("Mikrofon konnte nicht geöffnet werden")
}

fn make_sink(app: Option<AppHandle>) -> Result<SpoolSink> {
    let wav_path = std::env::temp_dir().join(format!(
        "pw-dictation-{}.wav",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    let spec = WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let writer = WavWriter::create(&wav_path, spec)
        .with_context(|| format!("Spool-WAV anlegen fehlgeschlagen: {}", wav_path.display()))?;
    Ok(SpoolSink {
        writer,
        wav_path,
        samples: Vec::new(),
        app,
        chunk_counter: 0,
    })
}

/// Mikrofon öffnen und Aufnahme starten (on-demand wie Handy; Warm-State M1).
/// `device`: gewählter Geräte-NAME (aus der UI); hier wird er auf die interne
/// Geräte-ID aufgelöst, weil `RecorderConfig.device` die ID will (Handy-
/// Verhalten: UI zeigt Namen, Recorder bekommt die ID). Ist das gewählte
/// Gerät nicht (mehr) vorhanden, fällt die Aufnahme auf den System-Standard
/// zurück statt still zu scheitern (Meldung im Log).
pub fn start(app: &AppHandle, device: Option<String>) -> Result<ActiveRecording> {
    let device_id = device.as_deref().and_then(|name| {
        handy_recorder::list_input_devices()
            .ok()
            .and_then(|devices| {
                devices
                    .into_iter()
                    .find(|d| !d.is_monitor && d.name == name)
                    .map(|d| d.id)
            })
    });
    if device.is_some() && device_id.is_none() {
        warn!(
            "Gerät {:?} nicht in der Geräteliste gefunden; nutze System-Standard",
            device
        );
    }
    let (recorder, used_fallback) = match open_recorder(device_id) {
        Ok(recorder) => (recorder, false),
        Err(e) if device.is_some() => {
            warn!(
                "Gerät '{:?}' nicht öffnenbar ({e:#}); falle auf System-Standard zurück",
                device
            );
            (open_recorder(None)?, true)
        }
        Err(e) => return Err(e),
    };
    if used_fallback {
        let _ = app.emit("pw://mic-fallback", ());
    }
    let sink = make_sink(Some(app.clone()))?;
    let started = Instant::now();
    recorder
        .start(sink)
        .map_err(|e| anyhow::anyhow!("Aufnahme-Start fehlgeschlagen: {}", e.error))?;
    Ok(ActiveRecording { recorder, started })
}

/// Aufnahme beenden und das gespoolte Audio zurückgeben.
pub fn stop(active: ActiveRecording) -> Result<SpooledRecording> {
    let stopped = active
        .recorder
        .stop()
        .context("Aufnahme konnte nicht gestoppt werden")?;
    match &stopped.end_reason {
        EndReason::StopCalled => {}
        reason => warn!("Aufnahme endete unerwartet: {reason:?}"),
    }
    stopped.sink.finish()
}
