//! Globale Push-to-Talk-Hotkeys über handy-keys (Low-Level-Hook, key-up-fähig).
//!
//! Zwei Modi mit je eigenem Hotkey (M0.3): Cursor-Diktat und Hand-off-Diktat.
//! Vereinfachtes Muster aus Handys shortcut/handy_keys.rs: ein dedizierter
//! Thread besitzt den HotkeyManager und pollt Events; Pressed = Aufnahme
//! starten (im Modus dieses Hotkeys), Released = stoppen + verarbeiten.

use std::time::Duration;

use handy_keys::{Hotkey, HotkeyManager, HotkeyState};
use log::{error, info, warn};
use tauri::AppHandle;

use crate::OutputMode;

pub const CURSOR_HOTKEY: &str = "ctrl+alt+space";
pub const HANDOFF_HOTKEY: &str = "ctrl+alt+h";

pub fn start(app: AppHandle) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("pw-hotkey".into())
        .spawn(move || {
            let manager = match HotkeyManager::new_with_blocking() {
                Ok(m) => m,
                Err(e) => {
                    error!("HotkeyManager fehlgeschlagen: {e}");
                    return;
                }
            };

            let mut registered: Vec<(handy_keys::HotkeyId, OutputMode, &str)> = Vec::new();
            for (hotkey_string, mode) in [
                (CURSOR_HOTKEY, OutputMode::Cursor),
                (HANDOFF_HOTKEY, OutputMode::Handoff),
            ] {
                let hotkey: Hotkey = match hotkey_string.parse() {
                    Ok(h) => h,
                    Err(e) => {
                        error!("Hotkey '{hotkey_string}' nicht parsebar: {e}");
                        continue;
                    }
                };
                match manager.register(hotkey) {
                    Ok(id) => {
                        info!("PTT-Hotkey aktiv: {hotkey_string} → {mode:?} (halten zum Diktieren)");
                        registered.push((id, mode, hotkey_string));
                    }
                    Err(e) => error!("Hotkey '{hotkey_string}' nicht registrierbar: {e}"),
                }
            }
            if registered.is_empty() {
                error!("Kein Hotkey registriert — Diktat reagiert nicht");
                return;
            }

            loop {
                while let Some(event) = manager.try_recv() {
                    let Some((_, mode, _)) = registered.iter().find(|(id, _, _)| *id == event.id)
                    else {
                        continue;
                    };
                    if event.state == HotkeyState::Pressed {
                        crate::on_ptt_down(&app, *mode);
                    } else {
                        crate::on_ptt_up(&app);
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("Hotkey-Thread konnte nicht gestartet werden: {e}"))?;
    Ok(())
}

// Warn-Nutzung vermeiden (mode in Debug-Ausgabe oben genutzt); Platzhalter für
// spätere Konflikt-Erkennung beim freien Konfigurieren (M1).
#[allow(dead_code)]
fn _conflict_hint() {
    warn!("Hotkey-Konflikt-Erkennung folgt mit dem Modi-Editor in M1");
}
