//! Hand-off-Modus: Diktat wird statt am Cursor in die Tagesdatei unter dem
//! konfigurierbaren Hand-off-Ablageort geschrieben — der Weg, Diktate an
//! elevated Fenster/Apps (z. B. Agenten-IDEs) zu übergeben, für die UIPI
//! injizierte Tasten blockt.
//!
//! Format pro Eintrag: `## HH:MM` + Text + Leerzeile, UTF-8, append-safe.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::PathBuf;

use crate::settings;

/// Aktives Hand-off-Verzeichnis: Einstellung, sonst der eingebaute Default
/// (M0.3-Nutzervorgabe: das hand-off-Verzeichnis dieses Projekts).
pub fn dir() -> PathBuf {
    settings::current()
        .handoff_dir
        .map(PathBuf::from)
        .unwrap_or_else(settings::default_handoff_dir)
}

pub fn append(text: &str) -> Result<PathBuf> {
    let dir = dir();
    std::fs::create_dir_all(&dir).context("hand-off-Verzeichnis anlegen fehlgeschlagen")?;

    let now = chrono::Local::now();
    let file = dir.join(format!("{}.md", now.format("%Y-%m-%d")));
    let entry = format!("## {}\n{}\n\n", now.format("%H:%M"), text.trim());

    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .with_context(|| format!("hand-off-Datei öffnen fehlgeschlagen: {}", file.display()))?;
    out.write_all(entry.as_bytes())
        .context("In hand-off-Datei schreiben fehlgeschlagen")?;
    Ok(file)
}
