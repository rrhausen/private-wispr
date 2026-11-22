//! Setup-Assistent (specs 4.10): Erststart-Erkennung. Das Frontend
//! (setup.tsx) entscheidet rein über `settings.setup_done` — die ganze
//! Heuristik steckt hier, unit-testbar und ohne Fenster.
//!
//! Veteranen-Migration: Ein `setup_done`-Feld gab es erst mit dem
//! Assistenten — serde-Default `false` würde ihn daher bei jedem Update
//! einer Bestandsinstallation aufploppen lassen, obwohl dort Mikrofon und
//! Modell längst eingerichtet sind. Definition „Bestandsinstallation":
//! Beim Laden lag eine settings.json UND das Modell ist auffindbar
//! (stt-Auflösung: neben der exe oder in %LOCALAPPDATA%). Solche
//! Installationen werden beim App-Start einmalig als eingerichtet
//! markiert (Info-Log, kein stiller Vorgang). Echte Erststarts (keine
//! settings.json — auch portabel mit mitgegebener GGUF) sehen den
//! Assistenten; der Modell-Schritt zeigt dann „bereits da".

use log::{info, warn};

/// Migrations-Bedarf: Bestandsinstallation, die den Assistenten nicht
/// mehr sehen soll (einmalig beim Start geprüft). Die Anzeige-Entscheidung
/// danach ist trivial: Der Assistent erscheint genau dann, wenn
/// `settings.setup_done` false ist (Frontend liest es via get_settings).
pub fn veteran_migration_needed(
    setup_done: bool,
    settings_existed: bool,
    model_present: bool,
) -> bool {
    !setup_done && settings_existed && model_present
}

/// Beim App-Start ausführen: Bestandsinstallation einmalig markieren.
pub fn run_startup_migration() {
    let done = crate::settings::current().setup_done;
    let existed = crate::settings::settings_existed();
    let present = crate::stt::default_model_path().exists();
    if !veteran_migration_needed(done, existed, present) {
        return;
    }
    match crate::settings::update(|s| s.setup_done = true) {
        Ok(()) => info!(
            "Bestandsinstallation erkannt (Settings vorhanden, Modell liegt vor) — \
             Setup-Assistent gilt als abgeschlossen"
        ),
        Err(e) => warn!("Veteranen-Migration fehlgeschlagen ({e:#}) — Assistent erscheint erneut"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bestandsinstallation (Settings + Modell, aber noch kein setup_done):
    /// einmalig als eingerichtet markieren.
    #[test]
    fn veteran_wird_migriert() {
        assert!(veteran_migration_needed(false, true, true));
    }

    /// Echter Erststart: keine settings.json — kein Migrieren, der
    /// Assistent erscheint (setup_done bleibt false).
    #[test]
    fn erststart_ohne_settings_zeigt_assistenten() {
        assert!(!veteran_migration_needed(false, false, true));
        assert!(!veteran_migration_needed(false, false, false));
    }

    /// Portabler Erststart mit mitgegebener GGUF: kein Veteran (keine
    /// Settings) — Assistent erscheint, Modell-Schritt meldet „bereits da".
    #[test]
    fn portabler_erststart_mit_gguf_ist_kein_veteran() {
        assert!(!veteran_migration_needed(false, false, true));
    }

    /// Bestand OHNE Modell (Modell nachträglich gelöscht): kein Veteran —
    /// der Assistent erscheint und führt zum Download.
    #[test]
    fn bestand_ohne_modell_zeigt_assistenten() {
        assert!(!veteran_migration_needed(false, true, false));
    }

    /// Nach Abschluss (oder Migration) erscheint nichts mehr.
    #[test]
    fn abgeschlossene_einrichtung_zeigt_nichts() {
        assert!(!veteran_migration_needed(true, true, true));
    }
}
