//! Settings-Store: JSON im Benutzerprofil (`%LOCALAPPDATA%\PrivateWispr\settings.json`).
//!
//! Tiefe durch eine schmale Seam (ADR 0002): Der Zustand lebt einmal im
//! Prozess-Cache — `current()` liest ohne Platte — und es gibt genau EINEN
//! Schreibpfad: `update()` mutiert, schreibt durch und committet den Cache
//! erst nach erfolgreichem Schreiben (bei Fehler bleiben Cache und Platte
//! alt, der Aufrufer sieht den Fehler statt eines Phantomzustands). Vorher:
//! 18× `load()` über fünf Module, 7× load→mut→save mit Last-write-wins-
//! Rennen zwischen den Settern, 4–5 Platten-Lesungen pro Diktat.
//!
//! Korrupte settings.json wird LAUT behandelt (Hausregel: keine stillen
//! Fallbacks): Fehler ins Log, Original als `settings.json.corrupt`
//! gesichert (ein vorhandenes Backup wird überschrieben — der letzte
//! korrupte Zustand ist der relevante), im Speicher Defaults — die App
//! startet und funktioniert. Eine fehlende Datei ist Erststart-normal
//! und bleibt still.
//!
//! Bewusst schlank statt tauri-plugin-store: jedes Feld hat einen Default,
//! unbekannte Felder werden ignoriert, fehlende Felder fallen auf den
//! Default zurück — dadurch bleibt das Format ohne explizite Schema-
//! Migration erweiterbar (specs 4.10).

use anyhow::{Context, Result};
use log::{error, warn};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

fn default_true() -> bool {
    true
}

/// Belegung der beiden PTT-Modi (handy-keys-String, z. B. "ctrl+alt+space").
/// Frei konfigurierbar seit M1 (specs 4.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

/// Fester Ablageort, solange die Einstellung nicht gesetzt ist: der
/// Dokumente-Ordner des Nutzers (portabel — der frühere hart kodierte
/// Entwicklerpfad existierte auf anderen Rechnern nicht, z. B. bei der
/// Verteilung als portable Kopie; die alte Nutzervorgabe Repo-Ordner ist
/// auf dem Entwicklungsrechner einmalig in der settings.json persistiert).
pub fn default_handoff_dir() -> PathBuf {
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PrivateWispr-hand-off")
}

fn settings_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PrivateWispr")
        .join("settings.json")
}

// ─── Prozess-Cache (ADR 0002) ───────────────────────────────────────────

/// Cache plus zugehöriger Dateipfad. Der Pfad ist Teil des Stores, damit
/// Tests gegen Temp-Dateien laufen statt gegen die echte settings.json.
struct Store {
    settings: Settings,
    path: PathBuf,
}

static STORE: OnceLock<Mutex<Store>> = OnceLock::new();

/// Lazy-Zugriff auf den Prozess-Store: Der erste Griff lädt einmalig von der
/// Platte (inklusive Korrupt-Behandlung), danach bleibt der Cache im
/// Speicher. Selftest und Tests brauchen keinen Init-Aufruf.
fn store() -> &'static Mutex<Store> {
    STORE.get_or_init(|| Mutex::new(open_store_at(settings_path())))
}

/// Lade-Ergebnis einer Settings-Datei: fehlende Datei (Erststart, still),
/// parsebar, oder korrupt (Quell-Fehlermeldung fürs Log).
enum Loaded {
    Parsed(Settings),
    Missing,
    Corrupt { source: String },
}

fn load_from(path: &Path) -> Loaded {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded::Missing,
        Err(e) => Loaded::Corrupt { source: e.to_string() },
        Ok(text) => match serde_json::from_str::<Settings>(&text) {
            Ok(s) => Loaded::Parsed(s),
            Err(e) => Loaded::Corrupt { source: e.to_string() },
        },
    }
}

/// Das korrupte Original sichern: `settings.json.corrupt` (ein vorhandenes
/// Backup wird überschrieben). Schlägt selbst das fehl, bleibt das Original
/// liegen und der Pfad zeigt darauf zurück.
fn backup_corrupt(path: &Path) -> PathBuf {
    let mut backup = path.to_path_buf();
    backup.set_extension("json.corrupt");
    let _ = std::fs::remove_file(&backup);
    if std::fs::rename(path, &backup).is_err() {
        warn!("Korrupte Settings nicht sicherbar — Original bleibt liegen: {}", path.display());
        return path.to_path_buf();
    }
    backup
}

/// Store-Inhalt von der Platte laden. Korrupte Datei: Defaults + Original
/// gesichert + Fehler geloggt (laut, aber die App läuft weiter).
fn open_store_at(path: PathBuf) -> Store {
    let settings = match load_from(&path) {
        Loaded::Parsed(s) => s,
        Loaded::Missing => Settings::default(),
        Loaded::Corrupt { source } => {
            let backup = backup_corrupt(&path);
            error!(
                "Settings unlesbar: {source} — Original gesichert als {}, arbeite mit Defaults weiter",
                backup.display()
            );
            Settings::default()
        }
    };
    Store { settings, path }
}

/// Aktueller Settings-Stand (Clone aus dem Prozess-Cache — keine Platte).
pub fn current() -> Settings {
    store().lock().unwrap().settings.clone()
}

/// Der EINZIGE Schreibpfad: mutiert, schreibt durch (write-through) und
/// committet den Cache erst nach erfolgreichem Schreiben. Bei Fehler
/// bleiben Cache und Platte alt; der Aufrufer bekommt den Fehler.
pub fn update(f: impl FnOnce(&mut Settings)) -> Result<()> {
    let mut guard = store().lock().unwrap();
    let mut next = guard.settings.clone();
    f(&mut next);
    write_to(&guard.path, &next)?;
    guard.settings = next;
    Ok(())
}

/// Atomar schreiben: erst die Temp-Datei, dann ersetzen — ein Absturz
/// mittendrin hinterlässt keine halbe Datei (eine solche landete beim
/// nächsten Start sonst im Korrupt-Pfad).
fn write_to(path: &Path, settings: &Settings) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(settings)?)
        .with_context(|| format!("Settings schreiben fehlgeschlagen: {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("Settings ersetzen fehlgeschlagen: {}", path.display()))?;
    Ok(())
}

// ─── Alt-Brücke (flüchtig, fliegt mit den Aufrufern im nächsten Commit) ──

pub fn load() -> Settings {
    current()
}

pub fn save(settings: &Settings) -> Result<()> {
    update(|s| *s = settings.clone())
}

#[cfg(test)]
pub(crate) mod testing {
    //! Test-Zugang zum Prozess-Store: umschalten gegen Settings + Temp-Pfad,
    //! restaurieren per Drop (auch bei assert-Panics). Prozess-weiter Lock,
    //! weil cargo test parallel läuft und der Store Prozess-Zustand ist.

    use super::*;
    use std::sync::{MutexGuard as TestMutexGuard, OnceLock as TestOnceLock};

    static SEQUENCE: TestOnceLock<std::sync::Mutex<()>> = TestOnceLock::new();

    /// Seriell halten, solange ein Test den Store umgeschaltet hat.
    pub fn sequence() -> TestMutexGuard<'static, ()> {
        SEQUENCE.get_or_init(|| std::sync::Mutex::new(())).lock().unwrap()
    }

    /// Voriger Stand, den Drop wiederherstellt.
    pub struct StoreSwap {
        prev: (Settings, PathBuf),
    }

    pub fn swap(settings: Settings, path: PathBuf) -> StoreSwap {
        let mut guard = store().lock().unwrap();
        let prev = (guard.settings.clone(), guard.path.clone());
        guard.settings = settings;
        guard.path = path;
        StoreSwap { prev }
    }

    impl Drop for StoreSwap {
        fn drop(&mut self) {
            let mut guard = store().lock().unwrap();
            guard.settings = self.prev.0.clone();
            guard.path = self.prev.1.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "pw-settings-test-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

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

    /// Pflicht-Test (1): Korrupte Datei ⇒ Defaults im Cache, Original als
    /// Backup gesichert (Inhalt erhalten), kein Panic.
    #[test]
    fn korrupte_datei_sichert_original_und_nimmt_defaults() {
        let _seq = testing::sequence();
        let path = temp_path();
        std::fs::write(&path, "{ keine json").unwrap();
        let opened = open_store_at(path.clone());
        assert_eq!(opened.settings, Settings::default(), "Cache muss Defaults enthalten");
        let backup = path.with_extension("json.corrupt");
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap_or_default(),
            "{ keine json",
            "Original muss unversehrt gesichert sein"
        );
        std::fs::remove_file(&backup).ok();
        std::fs::remove_file(&path).ok();
    }

    /// Pflicht-Test (2): Fehlende Datei ist Erststart-normal — stille
    /// Defaults, KEIN Backup, kein Korrupt-Pfad.
    #[test]
    fn fehlende_datei_ist_stiller_erststart() {
        let _seq = testing::sequence();
        let path = temp_path(); // bewusst nie geschrieben
        let opened = open_store_at(path.clone());
        assert_eq!(opened.settings, Settings::default());
        assert!(
            !path.with_extension("json.corrupt").exists(),
            "kein Backup bei fehlender Datei"
        );
    }

    /// Pflicht-Test (3): update() schreibt durch (atomar) und current()
    /// liefert danach den neuen Stand.
    #[test]
    fn update_schreibt_durch_und_current_liefert_neu() {
        let _seq = testing::sequence();
        let path = temp_path();
        let _swap = testing::swap(Settings::default(), path.clone());
        update(|s| s.theme = "light".into()).unwrap();
        assert_eq!(current().theme, "light");
        let on_disk: Settings =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk.theme, "light", "Platte muss den neuen Stand haben");
        assert!(!path.with_extension("json.tmp").exists(), "keine Temp-Reste");
        std::fs::remove_file(&path).ok();
    }

    /// Pflicht-Test (4): Zwei sequentielle update() auf verschiedene Felder
    /// überleben beide — das frühere load→mut→save-Rennen zwischen den
    /// Settern (Last write wins, erster Write still verloren) ist per
    /// Konstruktion weg.
    #[test]
    fn sequentielle_updates_verlieren_nichts_mehr() {
        let _seq = testing::sequence();
        let path = temp_path();
        let _swap = testing::swap(Settings::default(), path.clone());
        update(|s| s.mic_device = Some("BT700".into())).unwrap();
        update(|s| s.archive_audio = true).unwrap();
        let s = current();
        assert_eq!(s.mic_device.as_deref(), Some("BT700"));
        assert!(s.archive_audio);
        std::fs::remove_file(&path).ok();
    }

    /// Grillen-Entscheidung Rollback: Schlägt das Schreiben fehl (Pfad ist
    /// ein Verzeichnis), bleibt der Cache auf dem alten Stand — kein
    /// Phantomzustand „im Speicher schon geändert".
    #[test]
    fn schreibfehler_laesst_cache_und_platte_alt() {
        let _seq = testing::sequence();
        let dir = std::env::temp_dir().join(format!(
            "pw-settings-dir-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let _swap = testing::swap(Settings::default(), dir.clone());
        assert!(update(|s| s.theme = "light".into()).is_err());
        assert_eq!(current().theme, Settings::default().theme);
        std::fs::remove_file(dir.with_extension("json.tmp")).ok();
        std::fs::remove_dir_all(&dir).ok();
    }
}
