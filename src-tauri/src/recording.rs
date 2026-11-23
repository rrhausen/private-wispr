//! Aufnahme über handy-recorder (WASAPI → 16 kHz mono f32).
//!
//! Disk-Spooling ab dem ersten Chunk (specs 4.2): Das WAV wächst kontinuierlich
//! auf der Platte; eine Brain-Dump-Session von 10+ Minuten hat kein Timeout.
//! M0 ohne VAD (VAD-Policy folgt mit der Regel-Pipeline in M1).

use std::io::BufWriter;
use std::sync::{Arc, Mutex};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use handy_recorder::{AudioChunk, Channels, EndReason, Recorder, RecorderConfig, Sink};
use hound::{SampleFormat, WavSpec, WavWriter};
use log::{info, warn};
use tauri::{AppHandle, Emitter};

/// Wohin ein Diktat geht: Cursor-Ziel (Paste) oder Hand-off-Datei.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputMode {
    Cursor,
    Handoff,
}

/// Eine laufende Aufnahme. `recorder.stop()` liefert das Sink mit den Samples
/// zurück (Muster aus Handys managers/audio.rs).
pub struct ActiveRecording {
    recorder: Recorder<SpoolSink>,
    #[allow(dead_code)]
    pub started: Instant,
    pub mode: OutputMode,
}

/// Ergebnis einer beendeten Aufnahme. `wav_path` wird nach der Transkription
/// standardmäßig gelöscht (specs 4.8: Audio default löschen; bei Fehler
/// bleibt es für die Fehlersuche liegen). Ein optionales Archiv folgt später.
pub struct SpooledRecording {
    pub samples: Vec<f32>,
    pub wav_path: PathBuf,
    pub duration_secs: f64,
}

/// Beendete Aufnahme plus dem zurückgegebenen Recorder: handy-recorder
/// schließt das Gerät erst beim Drop — der Recorder kann also wiederverwendet
/// („warm gehalten") werden, damit z. B. Bluetooth-Headsets zwischen Diktaten
/// nicht trennen und beim nächsten Start die ersten Wörter verschlucken.
pub struct StoppedRecording {
    pub spooled: SpooledRecording,
    pub recorder: Recorder<SpoolSink>,
    /// EndReason::StopCalled (normales Ende) — nur dann wiederverwerten.
    pub clean: bool,
}

/// Der geöffnete Recorder-Typ für das Warmhalten in AppState.
pub type MicRecorder = Recorder<SpoolSink>;

/// Warmhaltem-Slot: `(Recorder, zuletzt benutzt)`. Der Failure-Handler des
/// Recorders leert den Slot, wenn das Gerät wegfällt.
pub type WarmSlot = Arc<Mutex<Option<(MicRecorder, Instant)>>>;

pub struct SpoolSink {
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

fn open_recorder(
    device: Option<String>,
    invalidated: Option<WarmSlot>,
) -> Result<Recorder<SpoolSink>> {
    let config = RecorderConfig {
        device,
        channels: Channels::MixToMono,
        frames_per_chunk: None,
        ..RecorderConfig::speech()
    };
    Recorder::open_with_failure_handler(config, move |err| {
        warn!("Recorder-Fehler: {err}");
        // Gerät ist weg (z. B. Headset aus): ein warm gehaltener Recorder
        // darf nicht wieder verwendet werden — Slot leeren. Während einer
        // Aufnahme ist der Recorder gar nicht im Slot (No-op).
        if let Some(slot) = &invalidated {
            slot.lock().unwrap().take();
        }
    })
    .context("Mikrofon konnte nicht geöffnet werden")
}

/// Mikrofon über die Fallback-Kette öffnen (ohne Aufnahme zu starten).
/// `device`: gewählter Geräte-NAME (aus der UI); hier wird er auf die interne
/// Geräte-ID aufgelöst, weil `RecorderConfig.device` die ID will.
///
/// Fallback-Kette (Blaupause Epicenter/Whispering-Verhalten, eigenständig
/// implementiert): gewähltes Gerät → einmaliger Retry nach kurzer Pause
/// (Bluetooth-HFP/SCO-Aufbau braucht Sekunden) → System-Standard → erstes
/// aktives Eingabegerät. Jeder Fallback wird sichtbar geloggt.
pub fn open_for_mic(
    device: Option<String>,
    invalidated: Option<WarmSlot>,
) -> Result<MicRecorder> {
    let device_id = resolve_device_id(device.as_deref());
    if device.is_some() && device_id.is_none() {
        warn!(
            "Gerät {:?} nicht in der Geräteliste gefunden; nutze Fallback-Kette",
            device
        );
    }

    // Versuch 1: gewähltes (oder aufgelöstes) Gerät.
    if let Some(id) = device_id.clone() {
        match open_recorder(Some(id), invalidated.clone()) {
            Ok(recorder) => return Ok(recorder),
            Err(e) => {
                warn!("Gerät '{device:?}' schlug fehl ({e:#}); Retry in 600 ms (Bluetooth-Aufbauzeit)");
            }
        }
        // Versuch 2: kurze Pause, dann nochmals (HFP/SCO-Link-Aufbau).
        std::thread::sleep(std::time::Duration::from_millis(600));
        match open_recorder(device_id, invalidated.clone()) {
            Ok(recorder) => return Ok(recorder),
            Err(e) => warn!("Retry für '{device:?}' schlug fehl ({e:#}); Fallback auf System-Standard"),
        }
    }

    // Versuch 3: System-Standard.
    match open_recorder(None, invalidated.clone()) {
        Ok(recorder) => {
            warn!("Aufnahme über System-Standard (gewähltes Gerät nicht verfügbar)");
            return Ok(recorder);
        }
        Err(e) => warn!("System-Standard schlug fehl ({e:#}); letzter Fallback: erstes aktives Gerät"),
    }

    // Versuch 4: erstes aktives Eingabegerät aus der Liste.
    if let Some(first) = first_active_device_id() {
        let recorder = open_recorder(Some(first), invalidated)
            .context("Auch das erste aktive Eingabegerät konnte nicht geöffnet werden")?;
        return Ok(recorder);
    }
    Err(anyhow::anyhow!("Kein Eingabegerät verfügbar"))
}

/// Recorder für eine neue Aufnahme besorgen — der einzige Code-Pfad für
/// Diktat und Mikrofon-Test: den warm gehaltenen aus dem Slot übernehmen
/// (erst `start_with` prüft seine Brauchbarkeit, das Gerät kann seit dem
/// Parken weggefallen sein), und wenn der Warm-Start fehlschlägt, EINMAL
/// frisch über die Fallback-Kette öffnen. Ohne Warm-Start geht es direkt
/// frisch los; schlägt das fehl, ist Schluss (Fehler beim Aufrufer).
pub fn acquire(
    app: &AppHandle,
    device: Option<String>,
    warm: &WarmSlot,
    mode: OutputMode,
) -> Result<ActiveRecording> {
    let parked = warm.lock().unwrap().take();
    if let Some((recorder, _)) = parked {
        info!("Warmes Mikrofon aus dem Slot — sofort startbereit");
        match start_with(app, recorder, mode) {
            Ok(active) => return Ok(active),
            Err(e) => warn!("Warmes Mikrofon startet nicht ({e:#}) — öffne frisch"),
        }
    } else {
        info!("Kein warmes Mikrofon — öffne über die Fallback-Kette ({device:?})");
    }
    let recorder = open_for_mic(device, Some(warm.clone()))?;
    start_with(app, recorder, mode)
}

/// Aufnahme mit einem (neuen oder warm gehaltenen) Recorder starten.
/// `app` fließt nur für den Spool-Sink rein (Pegel-Event `pw://level`).
pub fn start_with(
    app: &AppHandle,
    recorder: MicRecorder,
    mode: OutputMode,
) -> Result<ActiveRecording> {
    let sink = make_sink(Some(app.clone()))?;
    let started = Instant::now();
    recorder
        .start(sink)
        .map_err(|e| anyhow::anyhow!("Aufnahme-Start fehlgeschlagen: {}", e.error))?;
    Ok(ActiveRecording { recorder, started, mode })
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

/// Namen aus der UI auf die interne Geräte-ID mappen.
fn resolve_device_id(name: Option<&str>) -> Option<String> {
    name.and_then(|n| {
        handy_recorder::list_input_devices()
            .ok()
            .and_then(|devices| {
                devices
                    .into_iter()
                    .find(|d| !d.is_monitor && d.name == n)
                    .map(|d| d.id)
            })
    })
}

fn first_active_device_id() -> Option<String> {
    handy_recorder::list_input_devices()
        .ok()
        .and_then(|devices| devices.into_iter().find(|d| !d.is_monitor).map(|d| d.id))
}

/// Aufnahme beenden und das gespoolte Audio zurückgeben; der Recorder bleibt
/// geöffnet und kann warm gehalten werden (`clean` = normales Ende).
pub fn stop(active: ActiveRecording) -> Result<StoppedRecording> {
    let stopped = active
        .recorder
        .stop()
        .context("Aufnahme konnte nicht gestoppt werden")?;
    let clean = matches!(&stopped.end_reason, EndReason::StopCalled);
    if !clean {
        warn!("Aufnahme endete unerwartet: {:?} — Recorder wird verworfen", stopped.end_reason);
    }
    let spooled = stopped.sink.finish()?;
    Ok(StoppedRecording {
        spooled,
        recorder: active.recorder,
        clean,
    })
}
