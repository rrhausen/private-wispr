//! STT-Engine: transcribe.cpp (GGUF, Vulkan via dynamic-backends).
//!
//! M0: in-process hinter `Engine` (der `SttEngine`-Trait für mehrere Backends
//! und die Worker-Prozess-Isolation folgen in M1 — siehe specs 4.1).

use std::path::PathBuf;
use std::sync::Once;
use std::time::Instant;

use anyhow::{Context, Result};
use transcribe_cpp::{Backend, Model, ModelOptions, RunOptions, Session, Task};

/// transcribe.cpp-Backends PRO Prozess genau einmal initialisieren (Logging
/// + Backends). Engine und VAD-Modell rufen beide hier auf; ein zweiter
/// `init_backends_default()`-Aufruf im selben Prozess veränderte messbar
/// die Transkription (WER 1,9 % → 5,1 %, 22.11.2026) — daher Once.
pub fn ensure_backends_once() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        transcribe_cpp::init_logging();
        if let Err(e) = transcribe_cpp::init_backends_default() {
            // Laut melden, aber nicht abbrechen: Das Modell-Load gleich danach
            // liefert den präzisen Fehler (z. B. DLL fehlt).
            log::error!("Backends-Init fehlgeschlagen: {e}");
        }
    });
}

/// M0-Default-Modell (specs 5.1): Parakeet TDT 0.6B v3, Q8_0.
/// SHA256/Größe sind Vorrat für den In-App-Download-Manager (M1) und den
/// Verifikations-Check.
#[allow(dead_code)]
pub const MODEL_REPO: &str = "handy-computer/parakeet-tdt-0.6b-v3-gguf";
pub const MODEL_FILE: &str = "parakeet-tdt-0.6b-v3-Q8_0.gguf";
/// Gepinnte Revision (identisch zu scripts/download-model.ps1) — reproduzierbare
/// Downloads unabhängig vom HF-Hauptbranch.
pub const MODEL_REV: &str = "85ac09ea12fc4b1112fa76810059364bc6adc9de";
#[allow(dead_code)]
pub const MODEL_SHA256: &str = "5859f77944efcd8eafa23a6350731960b2b55b2203df51f319665c807d802cc7";
#[allow(dead_code)]
pub const MODEL_SIZE: u64 = 739_508_576;

pub fn default_model_path() -> PathBuf {
    let candidates = model_candidates();
    candidates
        .iter()
        .find(|p| p.exists())
        .cloned()
        // Kein Treffer: den Standard-Pfad melden (Download-Ziel + UI-Anzeige).
        .unwrap_or_else(|| models_dir().join(MODEL_FILE))
}

/// Suchreihenfolge fürs Modell: neben der exe (`models\`-Unterordner und
/// direkt — portabler Modus, die gguf einfach ins entpackte Verzeichnis
/// legen), dann `%LOCALAPPDATA%\PrivateWispr\models\` (Standard, Ziel des
/// In-App-Downloads). Exe-nah gewinnt bei Doppelung: die bewusst mitgegebene
/// Datei schlägt die zentrale Ablage.
fn model_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.extend(model_candidates_in(dir));
        }
    }
    out.push(models_dir().join(MODEL_FILE));
    out
}

/// Reine Variante von `model_candidates` für Unit-Tests.
fn model_candidates_in(exe_dir: &std::path::Path) -> Vec<PathBuf> {
    vec![exe_dir.join("models").join(MODEL_FILE), exe_dir.join(MODEL_FILE)]
}

pub fn models_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PrivateWispr")
        .join("models")
}

#[derive(Clone, Default, Debug, serde::Serialize, serde::Deserialize)]
pub struct EngineInfo {
    pub loaded: bool,
    pub model_name: String,
    pub backend: String,
    pub on_gpu: bool,
    pub device: Option<String>,
    pub error: Option<String>,
}

impl EngineInfo {
    pub fn device_or_default(&self) -> &str {
        self.device.as_deref().unwrap_or("auto")
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct TranscriptResult {
    pub text: String,
    pub language: Option<String>,
    pub transcription_ms: u128,
}

/// Interne Engine-Schnittstelle (specs 4.1): Die App spricht nur mit diesem
/// Trait. Implementierungen: `Engine` (in-process, Selftest/Eval) und
/// `engine_worker::WorkerEngine` (isolation, App-Betrieb). Ein späteres
/// CUDA-Sidecar dockt am selben Trait an, ohne Umbau.
pub trait SttEngine {
    /// 16-kHz-mono-f32-Samples transkribieren; `vocabulary` = Fachwörter aus
    /// dem Wörterbuch (Biasing im Decode).
    fn transcribe(&mut self, samples: &[f32], vocabulary: &[String])
        -> anyhow::Result<TranscriptResult>;
    fn info(&self) -> &EngineInfo;
}

pub struct Engine {
    session: Session,
    info: EngineInfo,
}

impl Engine {
    pub fn load_default() -> Result<Engine> {
        let path = default_model_path();
        anyhow::ensure!(
            path.exists(),
            "Modell fehlt unter {} — erst scripts/download-model.ps1 ausführen",
            path.display()
        );
        Self::load(&path)
    }

    pub fn load(path: &std::path::Path) -> Result<Engine> {
        crate::stt::ensure_backends_once();

        let model = Model::load_with(
            path,
            &ModelOptions { backend: Backend::Auto, device: None, no_mmap: false },
        )
        .with_context(|| format!("Modell-Load fehlgeschlagen ({})", path.display()))?;
        let session = model.session().context("Session-Erstellung fehlgeschlagen")?;

        let mut info = EngineInfo {
            loaded: true,
            model_name: path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            ..EngineInfo::default()
        };
        if let Ok(device) = model.device() {
            info.on_gpu = true; // Device-Binding gilt als GPU-Erfolg; CPU-Fallback loggt transcribe.cpp
            info.device = Some(format!("{}", device.index.unwrap_or(0)));
        }
        info.backend = model.backend().to_string();
        Ok(Engine { session, info })
    }

    pub fn info(&self) -> &EngineInfo {
        &self.info
    }

    /// 16-kHz-mono-f32-Samples transkribieren. Sprache ist auf Deutsch
    /// fixiert (Primärsprache laut specs); Auto-Detect folgt später.
    /// `vocabulary`: Fachwörter-Kanones aus dem Wörterbuch (specs 4.5 —
    /// Biasing im Decode, soweit das Modell es unterstützt).
    pub fn transcribe(&mut self, samples: &[f32], vocabulary: &[String]) -> Result<TranscriptResult> {
        let t0 = Instant::now();
        let options = RunOptions {
            task: Task::Transcribe,
            language: Some("de".to_string()),
            target_language: None,
            vocabulary: vocabulary.to_vec(),
            ..Default::default()
        };
        let transcript = self
            .session
            .run(samples, &options)
            .map_err(|e| anyhow::anyhow!("Transkription fehlgeschlagen: {e}"))?;
        Ok(TranscriptResult {
            text: transcript.text,
            language: transcript.language,
            transcription_ms: t0.elapsed().as_millis(),
        })
    }
}

impl SttEngine for Engine {
    fn transcribe(
        &mut self,
        samples: &[f32],
        vocabulary: &[String],
    ) -> Result<TranscriptResult> {
        // Ruft die gleichnamige inherent-Methode (inherent gewinnt).
        Engine::transcribe(self, samples, vocabulary)
    }

    fn info(&self) -> &EngineInfo {
        &self.info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modell_kandidaten_neben_der_exe_und_zentral() {
        let exe_dir = std::path::Path::new(r"C:\Tools\PrivateWispr");
        assert_eq!(
            model_candidates_in(exe_dir),
            vec![
                exe_dir.join("models").join(MODEL_FILE),
                exe_dir.join(MODEL_FILE),
            ]
        );
        // Die echte Kandidatenliste endet immer mit dem LOCALAPPDATA-Standard.
        let candidates = model_candidates();
        assert_eq!(candidates.last(), Some(&models_dir().join(MODEL_FILE)));
    }
}
