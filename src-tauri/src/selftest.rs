//! Headless-Selbsttest (`--selftest [audio.wav|verzeichnis]`): Keine GUI,
//! keine Aufnahme.
//!
//! Kein Argument: Engine-Init + Modell-Load + Geräte-Report (Build-Smoke-Test).
//! Mit Datei: vollständige Transkription mit Zeitmessung (Keimzelle des
//! Eval-Harness, specs Abschnitt 6).
//! Mit Verzeichnis: **Batch-Modus** — alle Audio-Clips transkribieren, gegen
//! die gleichnamigen `.txt`-Ground-Truths den normalisierten WER messen
//! (roh und nach Regel-Pipeline) und die Ersetzungs-Fehler sammeln, aus denen
//! Wörterbuch-Regeln abgeleitet werden. Audio-Formate: WAV 16 kHz direkt,
//! alles andere via ffmpeg (siehe `read_audio_16k_mono`).

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
    // Verzeichnis → Batch-Modus (eigene Tabelle auf stdout, JSON am Ende).
    if let Some(path) = wav {
        if path.is_dir() {
            return run_batch(path);
        }
    }
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

/// Audio-Erweiterungen, die der Batch-Modus einsammelt (WAV direkt, der Rest
/// über ffmpeg-Decode).
const AUDIO_EXTS: &[&str] = &["wav", "m4a", "mp3", "ogg", "opus", "flac", "aac", "wma"];

/// Batch-Modus: alle Clips eines Verzeichnisses gegen ihre `.txt`-Ground-
/// Truths messen. Ausgabe: menschenlesbare Tabelle + Ersetzungs-Topliste auf
/// stdout, danach das komplette Ergebnis als JSON (weiterverarbeitbar).
fn run_batch(dir: &Path) -> Result<serde_json::Value> {
    let mut clips: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("Verzeichnis nicht lesbar: {}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| AUDIO_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
        })
        .collect();
    clips.sort();
    anyhow::ensure!(!clips.is_empty(), "Keine Audio-Dateien in {}", dir.display());

    let t_start = std::time::Instant::now();
    let mut engine = crate::stt::Engine::load_default()?;
    let engine_info = engine.info().clone();
    println!(
        "Modell: {} · {}{} · Load {} ms",
        engine_info.model_name,
        engine_info.backend,
        if engine_info.on_gpu { " · GPU" } else { "" },
        t_start.elapsed().as_millis()
    );

    // Wörterbuch einmal laden; die Pipeline läuft pro Clip wie in der App.
    let dict = crate::dictionary::Dictionary::load();
    let vocabulary = dict.vocabulary();

    struct Row {
        name: String,
        audio_secs: f64,
        total_ms: f64,
        has_gt: bool,
        wer_raw: Option<f64>,
        wer_final: Option<f64>,
        errors: usize,
    }
    let mut rows: Vec<Row> = Vec::new();
    // Aggregate über alle Clips: Ersetzungen (ref → hyp) und fehlende Wörter.
    let mut all_subs: std::collections::BTreeMap<(String, String), usize> =
        std::collections::BTreeMap::new();
    let mut all_dels: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut sum_err = 0usize;
    let mut sum_len = 0usize;
    let mut total_audio = 0.0f64;
    let mut total_ms = 0.0f64;
    let mut clip_json: Vec<serde_json::Value> = Vec::new();

    for clip in &clips {
        let name = clip.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let samples = read_audio_16k_mono(clip)?;
        let audio_secs = samples.len() as f64 / 16_000.0;
        let t0 = std::time::Instant::now();
        let result = engine
            .transcribe(&samples, vocabulary)
            .with_context(|| format!("Transkription fehlgeschlagen: {}", clip.display()))?;
        let mut clip_ms = t0.elapsed().as_millis() as f64;
        clip_ms = clip_ms.max(1.0);
        let outcome = dict.apply(&result.text);
        let fitted = crate::fitting::fit(&outcome.text, &crate::fitting::SeamContext::none());

        let gt = std::fs::read_to_string(clip.with_extension("txt")).ok();
        let (wer_raw, wer_final, errors) = if let Some(gt) = &gt {
            let reference = crate::eval::normalize(gt);
            let raw = crate::eval::wer(&reference, &crate::eval::normalize(&result.text));
            let (fin, ops) = {
                let (r, ops) = crate::eval::align(&reference, &crate::eval::normalize(&fitted));
                (r, ops)
            };
            let (subs, dels) = crate::eval::error_pairs(&ops);
            for (k, v) in subs {
                *all_subs.entry(k).or_insert(0) += v;
            }
            for (k, v) in dels {
                *all_dels.entry(k).or_insert(0) += v;
            }
            sum_err += fin.substitutions + fin.deletions + fin.insertions;
            sum_len += fin.ref_len;
            (Some(raw.wer()), Some(fin.wer()), fin.substitutions + fin.deletions)
        } else {
            (None, None, 0)
        };
        rows.push(Row {
            name: name.clone(),
            audio_secs,
            total_ms: clip_ms,
            has_gt: gt.is_some(),
            wer_raw,
            wer_final,
            errors,
        });
        total_audio += audio_secs;
        total_ms += clip_ms;
        clip_json.push(serde_json::json!({
            "clip": name,
            "has_ground_truth": gt.is_some(),
            "audio_secs": audio_secs,
            "transcription_ms": result.transcription_ms,
            "total_ms": clip_ms,
            "wer_raw": wer_raw,
            "wer_final": wer_final,
            "raw_text": result.text,
            "final_text": fitted,
        }));
        println!("  ✓ {} ({:.1} s)", name, audio_secs);
    }

    // Tabelle (Menschenlesbar).
    println!(
        "\n{:<26} {:>8} {:>8} {:>9} {:>10} {:>7}",
        "Clip", "Sek.", "RTF", "WER roh", "WER final", "Fehler"
    );
    println!("{}", "-".repeat(76));
    for r in &rows {
        let pct = |w: &Option<f64>| w.map(|v| format!("{:>8.1}%", v * 100.0)).unwrap_or_else(|| "        —".into());
        println!(
            "{:<26} {:>8.1} {:>8.3} {} {} {:>7}",
            r.name,
            r.audio_secs,
            r.total_ms / 1000.0 / r.audio_secs.max(0.001),
            pct(&r.wer_raw),
            pct(&r.wer_final),
            if r.has_gt { format!("{}", r.errors) } else { "—".into() }
        );
    }
    let overall = if sum_len > 0 { sum_err as f64 / sum_len as f64 } else { 0.0 };
    println!("{}", "-".repeat(76));
    println!(
        "Gesamt: {:.1} s Audio · RTF {:.3} · WER final (wortgewichtet) {:.1}% ({} Fehler auf {} Wörtern)",
        total_audio,
        total_ms / 1000.0 / total_audio.max(0.001),
        overall * 100.0,
        sum_err,
        sum_len
    );

    // Ersetzungen als Topliste — die Rohdaten für Wörterbuch-Regeln.
    let mut subs_vec: Vec<(&(String, String), &usize)> = all_subs.iter().collect();
    subs_vec.sort_by(|a, b| b.1.cmp(a.1));
    println!("\nErsetzungen (Referenz ← erkannt, Top 40):");
    for ((r, h), n) in subs_vec.iter().take(40) {
        println!("  {:>3}×  {:?} ← {:?}", n, r, h);
    };
    let mut dels_vec: Vec<(&String, &usize)> = all_dels.iter().collect();
    dels_vec.sort_by(|a, b| b.1.cmp(a.1));
    println!("\nFehlende Wörter (Referenz, Top 20):");
    for (r, n) in dels_vec.iter().take(20) {
        println!("  {:>3}×  {:?}", n, r);
    }

    let substitutions_json: Vec<serde_json::Value> = subs_vec
        .iter()
        .map(|((r, h), n)| serde_json::json!({"ref": r, "hyp": h, "count": n}))
        .collect();
    let missing_json: Vec<serde_json::Value> = dels_vec
        .iter()
        .map(|(r, n)| serde_json::json!({"word": r, "count": n}))
        .collect();
    Ok(serde_json::json!({
        "ok": true,
        "batch_dir": dir.display().to_string(),
        "model": engine_info.model_name,
        "backend": engine_info.backend,
        "on_gpu": engine_info.on_gpu,
        "dictionary_version": dict.version,
        "clips": clip_json,
        "summary": {
            "clips": rows.len(),
            "audio_secs": total_audio,
            "total_ms": total_ms,
            "wer_final_weighted": overall,
            "errors": sum_err,
            "ref_words": sum_len,
        },
        "substitutions": substitutions_json,
        "missing": missing_json,
    }))
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
