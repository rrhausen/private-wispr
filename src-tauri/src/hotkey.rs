//! Globaler Push-to-Talk-Hotkey über handy-keys (Low-Level-Hook, key-up-fähig).
//!
//! Vereinfachtes Muster aus Handys shortcut/handy_keys.rs: ein dedizierter
//! Thread besitzt den HotkeyManager und pollt Events; Pressed = Aufnahme
//! starten, Released = stoppen + transkribieren (Hold-Modus, M0).

use std::time::Duration;

use handy_keys::{Hotkey, HotkeyManager, HotkeyState};
use log::{error, info};
use tauri::AppHandle;

pub fn start(app: AppHandle, hotkey_string: &'static str) -> anyhow::Result<()> {
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
            let hotkey: Hotkey = match hotkey_string.parse() {
                Ok(h) => h,
                Err(e) => {
                    error!("Hotkey '{hotkey_string}' nicht parsebar: {e}");
                    return;
                }
            };
            match manager.register(hotkey) {
                Ok(_id) => info!("PTT-Hotkey aktiv: {hotkey_string} (halten zum Diktieren)"),
                Err(e) => {
                    error!("Hotkey '{hotkey_string}' nicht registrierbar: {e}");
                    return;
                }
            }

            loop {
                while let Some(event) = manager.try_recv() {
                    if event.state == HotkeyState::Pressed {
                        crate::on_ptt_down(&app);
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
