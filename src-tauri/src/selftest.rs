//! Headless-Selbsttest (`--selftest [audio.wav]`): Keine GUI, keine Aufnahme.
//!
//! Kein Argument: Engine-Init + Modell-Load + Geräte-Report (Build-Smoke-Test).
//! Mit WAV: vollständige Transkription mit Zeitmessung — Keimzelle des
//! Eval-Harness (specs Abschnitt 6). Das WAV muss 16 kHz mono sein
//! (Float oder Int16; wird nach f32 normalisiert).

use std::path::Path;

use anyhow::{Context, Result};

pub fn run(wav: Option<std::path::PathBuf>) -> i32 {
    match run_inner(wav.as_deref()) {
        Ok(output) => {
            println!("{}", serde_json::to_string_pretty(&output).unwrap_or_default());
            0
        }
        Err(e) => {
            eprintln!("selftest fehlgeschlagen: {e:#}");
            1
        }
    }
}

fn run_inner(wav: Option<&Path>) -> Result<serde_json::Value> {
    let t_start = std::time::Instant::now();
    let mut engine = crate::stt::Engine::load_default()?;
    let load_ms = t_start.elapsed().as_millis();
    let info = engine.info().clone();

    let base = serde_json::json!({
        "ok": true,
        "model": info.model_name,
        "backend": info.backend,
        "on_gpu": info.on_gpu,
        "load_ms": load_ms,
    });

    let Some(path) = wav else { return Ok(base) };

    let samples = read_wav_16k_mono(path)?;
    let audio_secs = samples.len() as f64 / 16_000.0;
    let t0 = std::time::Instant::now();
    let result = engine
        .transcribe(&samples)
        .with_context(|| format!("Transkription fehlgeschlagen: {}", path.display()))?;
    let total_ms = t0.elapsed().as_millis();

    let rt = if audio_secs > 0.0 {
        Some(total_ms as f64 / 1000.0 / audio_secs)
    } else {
        None
    };
    let mut out = serde_json::json!({
        "wav": path.display().to_string(),
        "audio_secs": audio_secs,
        "transcription_ms": result.transcription_ms,
        "total_ms": total_ms,
        "realtime_factor": rt,
        "language": result.language,
        "text": result.text,
    });
    // Base-Felder (Modell, Backend, Load-Zeit) dazumischen.
    if let (Some(dst), Some(src)) = (out.as_object_mut(), base.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    Ok(out)
}

/// Liest ein WAV und normalisiert auf 16-kHz-mono-f32. Andere Sampleraten
/// werden abgelehnt (klarer Fehler statt stiller Verzerrung).
fn read_wav_16k_mono(path: &Path) -> Result<Vec<f32>> {
    let reader = hound::WavReader::open(path)
        .with_context(|| format!("WAV öffnen fehlgeschlagen: {}", path.display()))?;
    let spec = reader.spec();
    anyhow::ensure!(
        spec.sample_rate == 16_000,
        "WAV muss 16 kHz sein (ist {} Hz) — specs 4.2",
        spec.sample_rate
    );
    anyhow::ensure!(spec.channels == 1, "WAV muss mono sein (ist {} Kanäle)", spec.channels);

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<std::result::Result<Vec<f32>, _>>()
            .context("Float-Samples lesen fehlgeschlagen")?,
        hound::SampleFormat::Int => reader
            .into_samples::<i16>()
            .collect::<std::result::Result<Vec<i16>, _>>()
            .context("Int16-Samples lesen fehlgeschlagen")?
            .into_iter()
            .map(|s| s as f32 / i16::MAX as f32)
            .collect(),
    };
    Ok(samples)
}
