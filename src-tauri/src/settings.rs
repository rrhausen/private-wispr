//! Settings-Store: JSON im Benutzerprofil (`%LOCALAPPDATA%\PrivateWispr\settings.json`).
//!
//! Bewusst schlank statt tauri-plugin-store: jedes Feld hat einen Default,
//! unbekannte Felder werden ignoriert, fehlende Felder fallen auf den Default
//! zurück — dadurch bleibt das Format ohne explizite Schema-Migration
//! erweiterbar (specs 4.10). Der Umstieg folgt, wenn Migrationen nötig werden.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn default_true() -> bool {
    true
}

/// Belegung der beiden PTT-Modi (handy-keys-String, z. B. "ctrl+alt+space").
/// Frei konfigurierbar seit M1 (specs 4.3).
#[derive(Clone, Serialize, Deserialize)]
pub struct HotkeySettings {
    #[serde(default = "default_cursor_hotkey")]
    pub cursor: String,
    #[serde(default = "default_handoff_hotkey")]
    pub handoff: String,
}

fn default_cursor_hotkey() -> String {
    "ctrl+alt+space".into()
}

fn default_handoff_hotkey() -> String {
    "ctrl+alt+h".into()
}

impl Default for HotkeySettings {
    fn default() -> Self {
        HotkeySettings {
            cursor: default_cursor_hotkey(),
            handoff: default_handoff_hotkey(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Name des gewählten Eingabegeräts; `None` = System-Standard.
    #[serde(default)]
    pub mic_device: Option<String>,
    /// Ablageort der Hand-off-Tagesdateien; `None` = eingebauter Default.
    #[serde(default)]
    pub handoff_dir: Option<String>,
    /// Overlay-Pille anzeigen; default an.
    #[serde(default = "default_true")]
    pub overlay_enabled: bool,
    /// Dezenter Klick beim Aufnahme-Start; default an.
    #[serde(default = "default_true")]
    pub feedback_sound: bool,
    /// Historien-Aufbewahrung in Tagen (specs 4.8); 0 = unbegrenzt.
    #[serde(default = "default_history_retention_days")]
    pub history_retention_days: u64,
    /// Diktat-Audio aufbewahren statt nach der Transkription zu löschen
    /// (specs 4.8: optionales Archiv; default aus). Landet unter
    /// %APPDATA%\PrivateWispr\audio\JJJJ-MM-TT\ — u. a. zum Einsammeln der
    /// Eval-Clips direkt aus dem echten Diktatbetrieb.
    #[serde(default)]
    pub archive_audio: bool,
    /// Farbschema der App: "dark" | "light" | "system" (Default dark —
    /// das war bis M1 die einzige Welt).
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub hotkeys: HotkeySettings,
}

fn default_history_retention_days() -> u64 {
    90
}

fn default_theme() -> String {
    "dark".into()
}

pub const THEMES: &[&str] = &["dark", "light", "system"];

impl Default for Settings {
    fn default() -> Self {
        Settings {
            mic_device: None,
            handoff_dir: None,
            overlay_enabled: true,
            feedback_sound: true,
            history_retention_days: default_history_retention_days(),
            archive_audio: false,
            theme: default_theme(),
            hotkeys: HotkeySettings::default(),
        }
    }
}

/// Fester Ablageort, solange die Einstellung nicht gesetzt ist (Nutzervorgabe
/// aus M0.3 — das hand-off-Verzeichnis dieses Projekts).
pub fn default_handoff_dir() -> PathBuf {
    PathBuf::from(r"C:\Users\rrhau\projects\private-wispr\hand-off")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fehlende_felder_fallen_auf_default() {
        // Alte M0-Settings ohne die neuen Felder müssen weiter parsebar sein.
        let s: Settings = serde_json::from_str(r#"{"mic_device":"BT700"}"#).unwrap();
        assert_eq!(s.mic_device.as_deref(), Some("BT700"));
        assert_eq!(s.handoff_dir, None);
        assert!(s.overlay_enabled);
        assert_eq!(s.history_retention_days, 90);
        assert_eq!(s.theme, "dark");
        assert!(!s.archive_audio);
    }

    #[test]
    fn unbekannte_felder_werden_ignoriert() {
        let s: Settings =
            serde_json::from_str(r#"{"zukuenftiges_feld": 42, "overlay_enabled": true}"#).unwrap();
        assert!(s.overlay_enabled);
    }
}
