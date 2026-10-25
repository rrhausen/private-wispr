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

    let samples = read_audio_16k_mono(path)?;
    let audio_secs = samples.len() as f64 / 16_000.0;
    // Wörterbuch mitmessen (specs 4.5 + 6): Vocabulary in den Decode, danach
    // Regel-Pipeline auf das Rohtranskript — Keimzelle für WER-/Fachbegriff-
    // Metriken auf den Eval-Clips.
    let dict = crate::dictionary::Dictionary::load();
    let t0 = std::time::Instant::now();
    let result = engine
        .transcribe(&samples, dict.vocabulary())
        .with_context(|| format!("Transkription fehlgeschlagen: {}", path.display()))?;
    let total_ms = t0.elapsed().as_millis();
    let outcome = dict.apply(&result.text);
    let fitted = crate::fitting::fit(&outcome.text, &crate::fitting::SeamContext::none());

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
        "text": fitted,
        "raw_text": result.text,
        "applied_rules": outcome.applied,
        "dictionary_version": dict.version,
    });
    // Base-Felder (Modell, Backend, Load-Zeit) dazumischen.
    if let (Some(dst), Some(src)) = (out.as_object_mut(), base.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    Ok(out)
}

/// Audio auf 16-kHz-mono-f32 normalisieren. WAV in 16 kHz mono wird direkt
/// gelesen; alles andere (M4A/AAC — z. B. iPhone-Sprachmemos —, MP3, OPUS,
/// WAV mit anderer Rate/Kanalzahl) decodiert ffmpeg nach 16-kHz-mono-WAV
/// (Specs 4.9 macht das später mit gebundeltem ffmpeg für die UI; hier ist
/// es der Eval-/Selftest-Pfad). ffmpeg kommt aus dem PATH oder aus der
/// Umgebungsvariable `PW_FFMPEG`. Fehlt es, gibt es einen klaren Fehler
/// mit Beschaffungshinweis — kein stiller Ausfall.
fn read_audio_16k_mono(path: &Path) -> Result<Vec<f32>> {
    if let Some(samples) = try_read_wav_16k_mono(path)? {
        return Ok(samples);
    }
    decode_with_ffmpeg(path)
}

/// Liest das WAV, wenn es bereits 16 kHz mono ist (Float oder Int16), und
/// normalisiert auf f32. `Ok(None)` = ffmpeg-Pfad (kein WAV, oder andere
/// Rate/Kanalzahl — die Normalisierung macht ffmpeg explizit, statt still
/// verzerrt zu lesen).
fn try_read_wav_16k_mono(path: &Path) -> Result<Option<Vec<f32>>> {
    let Ok(reader) = hound::WavReader::open(path) else {
        return Ok(None); // kein WAV (oder nicht lesbar) → ffmpeg
    };
    let spec = reader.spec();
    if spec.sample_rate != 16_000 || spec.channels != 1 {
        return Ok(None);
    }
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
    Ok(Some(samples))
}

/// ffmpeg-Aufruf: Eingabe → 16-kHz-mono-f32-WAV nach %TEMP%, einlesen,
/// Temp-Datei entfernen.
fn decode_with_ffmpeg(path: &Path) -> Result<Vec<f32>> {
    let ffmpeg = std::env::var_os("PW_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
    let out = std::env::temp_dir().join(format!(
        "pw-decode-{}.wav",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    let output = std::process::Command::new(&ffmpeg)
        .arg("-y")
        .arg("-i").arg(path)
        .args(["-ar", "16000", "-ac", "1", "-c:a", "pcm_f32le", "-f", "wav"])
        .arg(&out)
        .output();
    match output {
        Err(_) => anyhow::bail!(
            "ffmpeg nicht gefunden ({}) — für M4A/MP3/OPUS oder andere Sampleraten nötig: \
             `winget install Gyan.FFmpeg` oder Umgebungsvariable PW_FFMPEG auf ffmpeg.exe setzen. \
             WAV 16 kHz mono funktioniert ohne ffmpeg.",
            ffmpeg.to_string_lossy()
        ),
        Ok(res) if !res.status.success() => {
            let stderr = String::from_utf8_lossy(&res.stderr);
            let tail: Vec<&str> = stderr.lines().rev().take(4).collect();
            anyhow::bail!(
                "ffmpeg-Decode fehlgeschlagen ({}): {}",
                path.display(),
                tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
            );
        }
        Ok(_) => {}
    }
    // ffmpeg schreibt garantiert 16 kHz mono f32 — hound liest es direkt.
    let reader = hound::WavReader::open(&out)
        .with_context(|| format!("ffmpeg-Ausgabe nicht lesbar: {}", out.display()))?;
    let samples: Vec<f32> = reader
        .into_samples::<f32>()
        .collect::<std::result::Result<Vec<f32>, _>>()
        .context("ffmpeg-Ausgabe-Samples lesen fehlgeschlagen")?;
    let _ = std::fs::remove_file(&out);
    Ok(samples)
}
