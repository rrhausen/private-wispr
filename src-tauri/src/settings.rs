//! Minimaler Settings-Store (M0): JSON im Benutzerprofil.
//! Der Umstieg auf tauri-plugin-store mit Schema-Migration folgt in M1
//! (specs 4.10) — das Dateiformat bleibt dabei gleich strukturiert.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// Name des gewählten Eingabegeräts; `None` = System-Standard.
    pub mic_device: Option<String>,
}

fn settings_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PrivateWispr")
        .join("settings.json")
}

pub fn load() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(settings: &Settings) -> Result<()> {
    let path = settings_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(settings)?)?;
    Ok(())
}
