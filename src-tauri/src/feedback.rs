//! Feedback-Klick beim Aufnahme-Start (M1): ein dezenter, synthetisierter
//! Doppelklick, abgespielt über Windows' eigenes `PlaySoundW` (asynchron,
//! kein zusätzlicher Audio-Stack im Prozess).
//!
//! Hintergrund: rodio war ursprünglich geplant, kollidiert aber mit
//! handy-recorder — rodio zieht cpal 0.15, handy-recorder den Fork handy-cpal
//! 0.18, und Cargo verbietet zwei `alsa-sys`-Versionen im Graph (links-
//! Konflikt). PlaySound mit einer einmalig erzeugten WAV liefert dieselbe UX
//! ohne neuen Dependency-Baum. Abschaltbar über die Einstellungen.

use anyhow::{Context, Result};
use std::path::PathBuf;

use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME, SND_NODEFAULT};

/// Klick abspielen, wenn die Einstellung an ist. PlaySoundW mit SND_ASYNC
/// kehrt sofort zurück; schlägt es fehl (z. B. kein Audiogerät), ist das nur
/// eine Warnung im Log — das Diktat selbst ist davon unberührt.
pub fn play_click_if_enabled() {
    if !crate::settings::load().feedback_sound {
        return;
    }
    if let Err(e) = play_click() {
        log::warn!("Feedback-Klick fehlgeschlagen: {e:#}");
    }
}

fn play_click() -> Result<()> {
    let path = ensure_click_wav()?;
    let wide = to_wide(&path);
    // SND_NODEFAULT: bleibt still, wenn die Datei fehlt — kein System-Piep.
    let ok = unsafe {
        PlaySoundW(PCWSTR(wide.as_ptr()), None, SND_ASYNC | SND_FILENAME | SND_NODEFAULT)
    };
    if !ok.as_bool() {
        anyhow::bail!("PlaySoundW fehlgeschlagen (kein Audiogerät?)");
    }
    Ok(())
}

/// WAV einmalig unter `%LOCALAPPDATA%\PrivateWispr\feedback-click.wav`
/// ablegen (Ordner gehört ohnehin zur App); bei jedem Start frisch erzeugen,
/// damit Code-Änderungen an der Klick-Form übernommen werden.
fn ensure_click_wav() -> Result<PathBuf> {
    let path = dirs::data_local_dir()
        .context("Kein LOCALAPPDATA-Verzeichnis")?
        .join("PrivateWispr")
        .join("feedback-click.wav");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(&path, spec)
        .with_context(|| format!("Klick-WAV anlegen fehlgeschlagen: {}", path.display()))?;
    for s in click_samples() {
        writer.write_sample(s)?;
    }
    writer.finalize()?;
    Ok(path)
}

fn to_wide(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Zwei kurze Sinus-Bursts mit steilem Attack und exponentiellem Decay —
/// ein leiser „Doppeltick". Peaks bleiben unter 0,2 (leise, aber hörbar).
/// Die Sample-Indizes sind ganzzahlig (Basis + i): die naheliegende Formel
/// `(start + t) * SR` rundet bei f32 gelegentlich auf i-1 und würde zwei
/// Beiträge auf einem Index aufsummieren.
fn click_samples() -> Vec<f32> {
    const SR: f32 = 48_000.0;
    // (Startzeit s, Frequenz Hz, Länge s)
    let bursts = [(0.0f32, 1_400.0, 0.028), (0.052, 950.0, 0.036)];
    let mut out: Vec<f32> = Vec::new();
    for (start, freq, len) in bursts {
        let n = (len * SR) as usize;
        let base = (start * SR).round() as usize;
        for i in 0..n {
            let t = i as f32 / SR;
            let attack = (t / 0.001).min(1.0);
            let decay = (-(t / (len / 4.0)).max(0.0)).exp();
            let s = (2.0 * std::f32::consts::PI * freq * t).sin();
            let idx = base + i;
            if out.len() <= idx {
                out.resize(idx + 1, 0.0);
            }
            out[idx] += 0.16 * attack * decay * s;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn klick_ist_leise_und_endlich() {
        let samples = click_samples();
        assert!(!samples.is_empty());
        assert!(samples.len() < 48_000 / 5, "kürzer als 200 ms");
        for s in &samples {
            assert!(s.is_finite());
            assert!(s.abs() <= 0.2, "Amplitude {s} über Pegelelimit");
        }
    }

    #[test]
    fn klick_wav_laesst_schreiben() {
        let samples = click_samples();
        let mut buf: Vec<u8> = Vec::new();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let cursor = std::io::Cursor::new(&mut buf);
        let mut writer = hound::WavWriter::new(cursor, spec).unwrap();
        for s in &samples {
            writer.write_sample(*s).unwrap();
        }
        writer.finalize().unwrap();
        assert!(buf.len() < 100_000, "PlaySound mag kleine WAVs (≤100 KB)");
    }
}
