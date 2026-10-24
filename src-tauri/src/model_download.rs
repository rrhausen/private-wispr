//! In-App-Modell-Download (M1): lädt das Default-Modell von HuggingFace
//! (gepinnte Revision) mit Fortschritts-Events (`pw://model-download`),
//! SHA256-Verifikation vor Nutzung, Abbruch über ein Flag und Resume über
//! Range-Requests (die `.part`-Datei bleibt beim Abbruch liegen).
//!
//! Bewusst blocking in einem eigenen Thread statt async: Der Download ist
//! I/O-gebunden, ureq ohne Runtime-Stack hält die Abhängigkeiten klein.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};

use crate::stt;

const CHUNK: usize = 256 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

static RUNNING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);

#[derive(Clone, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum Progress {
    Resuming { received: u64, total: u64 },
    Downloading { received: u64, total: u64 },
    Verifying,
    Done { path: String },
    Cancelled,
    Error { message: String },
}

impl Progress {
    fn emit(&self, app: &AppHandle) {
        let _ = app.emit("pw://model-download", self);
    }
}

/// Download starten, falls nicht schon einer läuft. Liefert sofort zurück;
/// der Verlauf kommt über `pw://model-download`. Nach erfolgreicher
/// Verifikation wird die Engine automatisch geladen.
pub fn start(app: AppHandle) -> Result<()> {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(()); // läuft schon — UI folgt den Events des laufenden Downloads
    }
    CANCEL.store(false, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("pw-model-download".into())
        .spawn(move || {
            let app = &app;
            match download(app) {
                Ok(()) => {}
                Err(e) => {
                    log::error!("Modell-Download fehlgeschlagen: {e:#}");
                    Progress::Error { message: format!("{e:#}") }.emit(app);
                }
            }
            RUNNING.store(false, Ordering::SeqCst);
        })
        .map_err(|e| {
            RUNNING.store(false, Ordering::SeqCst);
            anyhow::anyhow!("Download-Thread konnte nicht gestartet werden: {e}")
        })?;
    Ok(())
}

pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

fn download(app: &AppHandle) -> Result<()> {
    let final_path = stt::default_model_path();
    if final_path.exists() {
        // Schon da (z. B. via Skript geladen, während die UI offen war).
        Progress::Done { path: final_path.display().to_string() }.emit(app);
        crate::load_engine(app);
        return Ok(());
    }
    std::fs::create_dir_all(stt::models_dir()).context("Modellverzeichnis anlegen fehlgeschlagen")?;

    let part_path = part_path(&final_path);

    // Resume: vorhandene .part in Offset + Hash-Zustand übernehmen. Der Hash
    // muss auch die schon auf Platte liegenden Bytes erfassen, sonst stimmt
    // die Endsumme nach dem Fortsetzen nicht.
    let mut hasher = Sha256::new();
    let mut offset: u64 = 0;
    if part_path.exists() {
        let len = part_path.metadata().context(".part-Größe lesbar")?.len();
        if len > stt::MODEL_SIZE {
            std::fs::remove_file(&part_path).context("unvollständige .part löschen")?;
        } else if len > 0 {
            hasher = hash_existing(&part_path)?;
            offset = len;
            Progress::Resuming { received: offset, total: stt::MODEL_SIZE }.emit(app);
        }
    }

    let url = format!(
        "https://huggingface.co/{}/resolve/{}/{}",
        stt::MODEL_REPO,
        stt::MODEL_REV,
        stt::MODEL_FILE
    );
    let mut request = ureq::get(&url);
    if offset > 0 {
        request = request.set("Range", &format!("bytes={offset}-"));
    }
    let response = request
        .call()
        .map_err(|e| anyhow::anyhow!("Download-Request fehlgeschlagen: {e}"))?;

    // 206 = Resume funktioniert; alles andere (auch 200 nach Range-Request)
    // bedeutet: Server liefert die komplette Datei → von vorn beginnen.
    let resuming = response.status() == 206 && offset > 0;
    if !resuming {
        hasher = Sha256::new();
        offset = 0;
    }

    let mut reader = response.into_reader();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(resuming)
        .truncate(!resuming)
        .open(&part_path)
        .with_context(|| format!("{}.part öffnen fehlgeschlagen", final_path.display()))?;

    let mut received = offset;
    let mut buf = vec![0u8; CHUNK];
    let mut last_emit = Instant::now();
    loop {
        if CANCEL.load(Ordering::SeqCst) {
            file.flush().ok();
            Progress::Cancelled.emit(app); // .part bleibt fürs Resume liegen
            return Ok(());
        }
        let n = reader
            .read(&mut buf)
            .map_err(|e| anyhow::anyhow!("Download abgebrochen (Netzwerk): {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .context("Schreiben der Modelldaten fehlgeschlagen (Platte voll?)")?;
        hasher.update(&buf[..n]);
        received += n as u64;
        if last_emit.elapsed() >= PROGRESS_INTERVAL {
            Progress::Downloading { received, total: stt::MODEL_SIZE }.emit(app);
            last_emit = Instant::now();
        }
    }
    file.flush().context("Modelldaten flushen fehlgeschlagen")?;
    drop(file);
    Progress::Downloading { received, total: stt::MODEL_SIZE }.emit(app);

    // Verifikation VOR Nutzung (specs 4.1): Größe und SHA256.
    Progress::Verifying.emit(app);
    let hash = format!("{:x}", hasher.finalize());
    if received != stt::MODEL_SIZE {
        std::fs::remove_file(&part_path).ok();
        bail!(
            "Größe stimmt nicht: erwartet {} Bytes, bekommen {} — Download wiederholen",
            stt::MODEL_SIZE,
            received
        );
    }
    if hash != stt::MODEL_SHA256 {
        std::fs::remove_file(&part_path).ok();
        bail!("SHA256-Mismatch — Datei war beschädigt/verändert, erneut herunterladen");
    }

    std::fs::rename(&part_path, &final_path).context("Modell finalisieren fehlgeschlagen")?;
    log::info!(
        "Modell herunterladen und verifiziert: {} ({} MB)",
        final_path.display(),
        stt::MODEL_SIZE / (1024 * 1024)
    );
    Progress::Done { path: final_path.display().to_string() }.emit(app);

    crate::load_engine(app);
    Ok(())
}

fn part_path(final_path: &Path) -> std::path::PathBuf {
    let mut name = final_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.push_str(".part");
    final_path.with_file_name(name)
}

/// SHA256 über eine bestehende Datei streamen (Resume-Vorlauf).
fn hash_existing(path: &Path) -> Result<Sha256> {
    let mut f = std::fs::File::open(path).with_context(|| format!("{} öffnen", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = f.read(&mut buf).context("Datei lesen")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    #[test]
    fn resume_hash_entspricht_ganzdatei_hash() {
        let dir = std::env::temp_dir().join("pw-dl-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("resume-test.bin");
        // Deterministische „Datei“ größer als ein Chunk, damit mehrfach gelesen wird.
        let data: Vec<u8> = (0..CHUNK * 2 + 123)
            .map(|i| (i % 251) as u8)
            .collect();
        std::fs::write(&path, &data).unwrap();

        let resumed = hash_existing(&path).unwrap().finalize();
        let expected = Sha256::digest(&data);
        assert_eq!(resumed[..], expected[..]);

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    #[test]
    fn part_pfad_haengt_suffix_an() {
        let p = part_path(Path::new(r"C:\models\x.gguf"));
        assert_eq!(p, Path::new(r"C:\models\x.gguf.part"));
    }

    #[test]
    fn progress_events_serialisieren_state_tag() {
        let json = serde_json::to_value(Progress::Downloading { received: 1, total: 2 }).unwrap();
        assert_eq!(json["state"], "downloading");
        assert_eq!(json["received"], 1);
        let json = serde_json::to_value(Progress::Cancelled).unwrap();
        assert_eq!(json, serde_json::json!({ "state": "cancelled" }));
    }
}
