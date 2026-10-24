//! Diagnose-Werkzeug: prüft handy-keys isoliert von der App.
//! 1) Roher KeyboardListener: kommen ÜBERHAUPT Tastenereignisse an?
//! 2) HotkeyManager + ctrl+alt+space: matcht die Hotkey-Erkennung?
//! Ausführen: cargo run --example hotkey_probe

use std::time::{Duration, Instant};

fn main() {
    println!("[probe] Phase 1: roher KeyboardListener, 120 s lang (bitte JETZT Tasten drücken)");
    match handy_keys::KeyboardListener::new() {
        Ok(listener) => {
            let t0 = Instant::now();
            let mut seen = 0usize;
            while t0.elapsed() < Duration::from_secs(120) {
                match listener.recv_timeout(Duration::from_millis(200)) {
                    Ok(ev) => {
                        seen += 1;
                        println!("[probe] raw event #{seen}: {ev:?}");
                    }
                    Err(_) => {}
                }
            }
            println!("[probe] Phase 1 Ende: {seen} rohe Events gesehen");
        }
        Err(e) => println!("[probe] FEHLER KeyboardListener: {e}"),
    }

    println!("[probe] Phase 2: HotkeyManager mit ctrl+alt+space, 15 s");
    match HotkeyManagerProbe::run() {
        Ok(count) => println!("[probe] Phase 2 Ende: {count} Hotkey-Events"),
        Err(e) => println!("[probe] FEHLER HotkeyManager: {e}"),
    }
}

struct HotkeyManagerProbe;

impl HotkeyManagerProbe {
    fn run() -> Result<usize, String> {
        let manager = handy_keys::HotkeyManager::new_with_blocking()
            .map_err(|e| format!("new_with_blocking: {e}"))?;
        let hotkey: handy_keys::Hotkey = "ctrl+alt+space"
            .parse()
            .map_err(|e: handy_keys::Error| format!("parse: {e}"))?;
        let id = manager.register(hotkey).map_err(|e| format!("register: {e}"))?;
        println!("[probe] registriert, id={id:?}");
        let t0 = Instant::now();
        let mut count = 0usize;
        while t0.elapsed() < Duration::from_secs(15) {
            while let Some(ev) = manager.try_recv() {
                count += 1;
                println!("[probe] HOTKEY EVENT #{count}: id={:?} state={:?}", ev.id, ev.state);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(count)
    }
}
