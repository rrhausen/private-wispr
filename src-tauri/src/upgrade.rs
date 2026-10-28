//! Upgrade-Mechanismus (nutzergesteuert — kein Hintergrund-Auto-Update,
//! das bleibt laut Spec bewusst draußen).
//!
//! Szenario: Auf dem Zielrechner läuft eine NSIS-Installation (setup.exe),
//! und der Nutzer startet später eine portable Kopie einer neueren Version
//! (exe aus dem Verteil-ZIP). Die App erkennt die Installation über den
//! Uninstall-Eintrag in der Registry und bietet an, sie **an Ort und Stelle
//! sauber zu aktualisieren**: Programmdateien (exe + Engine-DLLs) in den
//! Installationsordner kopieren und die Versionsnummer nachführen.
//! Einstellungen/Modell (`%LOCALAPPDATA%`) und Historie/Wörterbuch
//! (`%APPDATA%`) liegen außerhalb und bleiben unberührt — genau wie beim
//! erneuten Durchlauf einer setup.exe, nur ohne sie mitgeben zu müssen.
//!
//! Grenzen, klar gemeldet statt still: Läuft die installierte Instanz noch
//! (Tray), schlägt der exklusive Dateizugriff fehl → Hinweis, sie zuerst zu
//! beenden. Eine Installation „für alle Nutzer" (HKLM) braucht Adminrechte
//! → Hinweis auf die setup.exe. Downgrades gibt es nicht.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Produktname im Uninstall-Eintrag (identisch zum NSIS-Installer).
const DISPLAY_NAME: &str = "Private Wispr";

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct UpgradePlan {
    /// Version der gefundenen Installation (Registry `DisplayVersion`).
    pub installed_version: String,
    /// Version der gerade laufenden (portablen) exe.
    pub own_version: String,
    /// Installationsordner (Registry `InstallLocation`).
    pub install_dir: String,
}

#[derive(Clone, serde::Serialize)]
pub struct UpgradeReport {
    pub copied_files: usize,
    pub install_dir: String,
}

pub fn own_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Numerischer Versionsvergleich (a > b). Suffixe wie „-beta" werden
/// ignoriert; fehlende Teile zählen als 0 (0.1 == 0.1.0).
pub fn is_newer(a: &str, b: &str) -> bool {
    let pa = parse_version(a);
    let pb = parse_version(b);
    let n = pa.len().max(pb.len());
    for i in 0..n {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

fn parse_version(v: &str) -> Vec<u64> {
    v.split(['-', '+'])
        .next()
        .unwrap_or_default()
        .split('.')
        .map(|p| p.trim().parse().unwrap_or(0))
        .collect()
}

/// Gefundene Installation (aus dem Uninstall-Eintrag).
struct Installed {
    version: String,
    install_dir: String,
    per_machine: bool,
}

/// Uninstall-Eintrag der App suchen: HKCU (per-user-Installation, Tauri-
/// Default) vor HKLM (per-machine). Matching über `DisplayName`, weil der
/// Key-Name je Installer-Variante variiert.
fn find_installed() -> Option<Installed> {
    use windows_registry::{CURRENT_USER, LOCAL_MACHINE};

    const UNINSTALL: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
    for (root, per_machine) in [(LOCAL_MACHINE, true), (CURRENT_USER, false)] {
        let Ok(uninstall) = root.open(UNINSTALL) else { continue };
        let Ok(keys) = uninstall.keys() else { continue };
        for key in keys {
            let Ok(sub) = uninstall.open(&key) else { continue };
            let name = sub.get_string("DisplayName").unwrap_or_default();
            if name != DISPLAY_NAME {
                continue;
            }
            let version = sub.get_string("DisplayVersion").unwrap_or_default();
            let dir = sub.get_string("InstallLocation").unwrap_or_default();
            if dir.is_empty() {
                continue;
            }
            return Some(Installed { version, install_dir: dir, per_machine });
        }
    }
    None
}

/// Upgrade anbieten? Nur wenn: Installation vorhanden, wir laufen nicht
/// selbst aus ihrem Ordner, und wir sind neuer. Kein Downgrade.
pub fn detect() -> Option<UpgradePlan> {
    let installed = find_installed()?;
    let own_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    if same_dir(&own_dir, Path::new(&installed.install_dir)) {
        return None; // wir SIND die Installation
    }
    if !is_newer(&own_version(), &installed.version) {
        return None;
    }
    Some(UpgradePlan {
        installed_version: installed.version,
        install_dir: installed.install_dir,
        own_version: own_version(),
    })
}

/// Laufende Instanz erkennen, ohne Prozessliste: Windows sperrt eine
/// ausgeführte exe gegen Schreibzugriff — der exklusive Öffnungsversuch
/// ist der Test.
fn app_in_dir_is_running(dir: &Path) -> bool {
    let exe = dir.join("private-wispr.exe");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&exe)
        .is_err()
}

/// Das Upgrade durchführen: exe + alle DLLs aus dem eigenen Ordner in den
/// Installationsordner kopieren, `DisplayVersion` nachziehen.
pub fn apply(plan: &UpgradePlan) -> Result<UpgradeReport> {
    // Per-machine-Installationen liegen unter HKLM/Program Files — dort
    // braucht das Überschreiben Adminrechte; klar melden statt scheitern.
    // (detect() liefert den Plan nur für die portable Kopie, die nie aus
    // dem Installationsordner läuft.)
    if find_installed().is_some_and(|i| i.per_machine) {
        bail!(
            "Die Installation liegt für alle Nutzer vor (braucht Adminrechte). \
             Bitte das Upgrade über die setup.exe ausführen."
        );
    }

    let own_dir = std::env::current_exe()
        .ok()
        .context("Eigener Programmordner nicht ermittelbar")?
        .parent()
        .context("Kein Elternordner")?
        .to_path_buf();
    let target_dir = PathBuf::from(&plan.install_dir);
    anyhow::ensure!(target_dir.is_dir(), "Installationsordner fehlt: {}", target_dir.display());

    if app_in_dir_is_running(&target_dir) {
        bail!(
            "Die installierte Private-Wispr-Instanz läuft noch (vermutlich im Tray). \
             Bitte sie zuerst über das Tray-Menü beenden und dann erneut aktualisieren."
        );
    }

    // Eigene Programmdateien einsammeln: exe + alle DLLs (Engine-Backends).
    let mut sources: Vec<PathBuf> = vec![own_dir.join("private-wispr.exe")];
    for entry in std::fs::read_dir(&own_dir).context("Eigener Ordner nicht lesbar")?.flatten() {
        if entry.path().extension().is_some_and(|e| e.eq_ignore_ascii_case("dll")) {
            sources.push(entry.path());
        }
    }

    let mut copied = 0usize;
    for src in &sources {
        let name = src.file_name().context("Dateiname fehlt")?;
        let dst = target_dir.join(name);
        std::fs::copy(src, &dst).with_context(|| {
            format!("Kopieren fehlgeschlagen: {} → {}", src.display(), dst.display())
        })?;
        copied += 1;
    }

    // Registry-Version nachführen, damit „Apps & Features“ die neue Version
    // zeigt (per-user-Installation).
    update_display_version(&plan.own_version);

    log::info!(
        "Upgrade ausgeführt: Installation {} → {} ({} Dateien nach {})",
        plan.installed_version,
        plan.own_version,
        copied,
        target_dir.display()
    );
    Ok(UpgradeReport { copied_files: copied, install_dir: plan.install_dir.clone() })
}

/// `DisplayVersion` im zugehörigen Uninstall-Eintrag setzen (per-user).
/// Schlägt das Lesen/Schreiben fehl, ist das nicht fatal — die App läuft
/// trotzdem in der neuen Version; nur „Apps & Features“ zeigte die alte.
fn update_display_version(version: &str) {
    use windows_registry::CURRENT_USER;
    const UNINSTALL: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
    let Ok(uninstall) = CURRENT_USER.open(UNINSTALL) else { return };
    let Ok(keys) = uninstall.keys() else { return };
    for key in keys {
        let Ok(sub) = uninstall.open(&key) else { continue };
        if sub.get_string("DisplayName").unwrap_or_default() == DISPLAY_NAME {
            let _ = sub.set_string("DisplayVersion", version);
            return;
        }
    }
    log::warn!("DisplayVersion konnte nicht nachgezogen werden (Uninstall-Eintrag nicht schreibbar)");
}

/// Pfadvergleich ohne Groß-/Kleinschreibung und ohne trailing slash.
fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let s = p.to_string_lossy().trim_end_matches('\\').to_lowercase();
        s
    };
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versionsvergleich_zahlenweise() {
        assert!(is_newer("0.1.2", "0.1.1"));
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1", "0.0.9"));
        assert!(!is_newer("0.1.1", "0.1.2"));
        assert!(!is_newer("0.1.2", "0.1.2"));
        assert!(is_newer("0.1.2", "0.1")); // 0.1 == 0.1.0
    }

    #[test]
    fn suffixe_und_muell_robust() {
        assert!(is_newer("0.2.0-beta", "0.1.9"));
        assert!(!is_newer("0.1.0-x", "0.1.0"));
        assert!(!is_newer("übung", "0.1.0")); // unparsebar == 0
    }

    #[test]
    fn gleicher_ordner_unterschiedliche_schreibweise() {
        assert!(same_dir(
            Path::new(r"C:\Tools\PrivateWispr"),
            Path::new(r"c:\tools\privatewispr\")
        ));
        assert!(!same_dir(Path::new(r"C:\Tools\A"), Path::new(r"C:\Tools\B")));
    }
}
