//! STT-Engine: transcribe.cpp (GGUF, Vulkan via dynamic-backends).
//!
//! M0: in-process hinter `Engine` (der `SttEngine`-Trait für mehrere Backends
//! und die Worker-Prozess-Isolation folgen in M1 — siehe specs 4.1).

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use transcribe_cpp::{Backend, Model, ModelOptions, RunOptions, Session, Task};

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
    models_dir().join(MODEL_FILE)
}

pub fn models_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PrivateWispr")
        .join("models")
}

#[derive(Clone, Default, serde::Serialize)]
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

#[derive(Clone, serde::Serialize)]
pub struct TranscriptResult {
    pub text: String,
    pub language: Option<String>,
    pub transcription_ms: u128,
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
        transcribe_cpp::init_logging();
        transcribe_cpp::init_backends_default()
            .map_err(|e| anyhow::anyhow!("Backends-Init fehlgeschlagen: {e}"))?;

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

    /// 16-kHz-mono-f32-Samples transkribieren. Sprache ist für M0 auf Deutsch
    /// fixiert (Primärsprache laut specs); Auto-Detect folgt mit dem
    /// Wörterbuch/M1.
    pub fn transcribe(&mut self, samples: &[f32]) -> Result<TranscriptResult> {
        let t0 = Instant::now();
        let options = RunOptions {
            task: Task::Transcribe,
            language: Some("de".to_string()),
            target_language: None,
            vocabulary: Vec::new(),
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
