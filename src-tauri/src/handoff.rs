//! Hand-off-Modus: Diktat wird statt am Cursor in die Tagesdatei unter
//! `hand-off/` geschrieben — der Weg, Diktate an elevated Fenster/Apps
//! (z. B. Agenten-IDEs) zu übergeben, für die UIPI injizierte Tasten blockt.
//!
//! Format pro Eintrag: `## HH:MM` + Text + Leerzeile, UTF-8, append-safe.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::PathBuf;

/// M0.3: fester Pfad laut Nutzervorgabe; wandert in M1 in die Einstellungen.
const HANDOFF_DIR: &str = r"C:\Users\rrhau\projects\private-wispr\hand-off";

pub fn append(text: &str) -> Result<PathBuf> {
    let dir = PathBuf::from(HANDOFF_DIR);
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
