//! Globale Push-to-Talk-Hotkeys über handy-keys (Low-Level-Hook, key-up-fähig).
//!
//! M1: Hotkeys sind frei konfigurierbar (specs 4.3, Whispering-Stil). Die
//! Belegung kommt aus den Settings; `set_hotkeys` registriert zur Laufzeit neu
//! (mit Rollback auf die alte Belegung, wenn eine Kombination nicht
//! registrierbar ist) und der Key-Capture (`start_capture`) liefert über einen
//! eigenen nicht-blockenden Listener die gedrückte Kombination als String.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use handy_keys::{Hotkey, HotkeyId, HotkeyManager, HotkeyState, Key, KeyboardListener, Modifiers};
use log::{error, info, warn};
use tauri::{AppHandle, Emitter};

use crate::OutputMode;

/// Defaults für die erste Nutzung (M0-Belegung).
pub const DEFAULT_CURSOR_HOTKEY: &str = "ctrl+alt+space";
pub const DEFAULT_HANDOFF_HOTKEY: &str = "ctrl+alt+h";

/// Der Manager gehört dem Hotkey-Thread, ist aber über `&self` von
/// `set_hotkeys` aus neu konfigurierbar. mpsc-Receiver ist nicht Sync, daher
/// liegt der Manager im Mutex. Lock-Reihenfolge im ganzen Modul: erst
/// MANAGER, dann ACTIVE (verhindert Deadlocks).
static MANAGER: OnceLock<Mutex<HotkeyManager>> = OnceLock::new();
static ACTIVE: Mutex<Vec<(HotkeyId, OutputMode)>> = Mutex::new(Vec::new());

/// Key-Capture läuft höchstens einmal gleichzeitig; Abbruch über ein Flag.
static CAPTURE_RUNNING: AtomicBool = AtomicBool::new(false);
static CAPTURE_CANCEL: AtomicBool = AtomicBool::new(false);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);

pub fn start(app: AppHandle) -> anyhow::Result<()> {
    let settings = crate::settings::load();
    let cursor = settings.hotkeys.cursor.clone();
    let handoff = settings.hotkeys.handoff.clone();

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
            if MANAGER.set(Mutex::new(manager)).is_err() {
                error!("HotkeyManager bereits initialisiert");
                return;
            }

            if let Err(msg) = set_hotkeys(&cursor, &handoff) {
                error!("Hotkey-Registrierung fehlgeschlagen: {msg} — Diktat reagiert nicht");
                // Reset-Versuch auf die Defaults, damit die App bedienbar bleibt.
                if set_hotkeys(DEFAULT_CURSOR_HOTKEY, DEFAULT_HANDOFF_HOTKEY).is_ok() {
                    warn!("Auf Default-Hotkeys zurückgefallen ({DEFAULT_CURSOR_HOTKEY}, {DEFAULT_HANDOFF_HOTKEY})");
                }
                return;
            }
            info!(
                "PTT-Hotkeys aktiv: {cursor} → Cursor · {handoff} → Hand-off (halten zum Diktieren)"
            );

            loop {
                // Events unter dem Lock einsammeln, PTT-Handling danach außerhalb:
                // on_ptt_down kann beim Bluetooth-Retry ~600 ms dauern und würde
                // sonst set_hotkeys blockieren.
                let mut events: Vec<(OutputMode, HotkeyState)> = Vec::new();
                if let Some(m) = MANAGER.get() {
                    let manager = m.lock().unwrap();
                    while let Some(event) = manager.try_recv() {
                        let mode = ACTIVE
                            .lock()
                            .unwrap()
                            .iter()
                            .find(|(id, _)| *id == event.id)
                            .map(|(_, mode)| *mode);
                        if let Some(mode) = mode {
                            events.push((mode, event.state));
                        }
                    }
                }
                for (mode, state) in events {
                    if state == HotkeyState::Pressed {
                        crate::on_ptt_down(&app, mode);
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

/// Aktuell aktive Belegung (aus der Registrierung, nicht aus den Settings —
/// bei Registrierungsfehlern bleibt die alte Belegung aktiv).
pub fn current() -> (String, String) {
    let Some(m) = MANAGER.get() else {
        return (DEFAULT_CURSOR_HOTKEY.into(), DEFAULT_HANDOFF_HOTKEY.into());
    };
    let manager = m.lock().unwrap();
    let mut cursor = None;
    let mut handoff = None;
    for (id, mode) in ACTIVE.lock().unwrap().iter() {
        if let Some(hotkey) = manager.get_hotkey(*id) {
            match mode {
                OutputMode::Cursor => cursor = Some(hotkey.to_handy_string()),
                OutputMode::Handoff => handoff = Some(hotkey.to_handy_string()),
            }
        }
    }
    (
        cursor.unwrap_or_else(|| DEFAULT_CURSOR_HOTKEY.into()),
        handoff.unwrap_or_else(|| DEFAULT_HANDOFF_HOTKEY.into()),
    )
}

/// Beide PTT-Hotkeys validieren und atomar umregistrieren. Bei Fehler bleibt
/// die bisherige Belegung aktiv und die Meldung erklärt den Grund.
pub fn set_hotkeys(cursor: &str, handoff: &str) -> Result<(), String> {
    let cursor_hotkey = validate(cursor)?;
    let handoff_hotkey = validate(handoff)?;
    if cursor_hotkey.to_handy_string() == handoff_hotkey.to_handy_string() {
        return Err(format!(
            "„{}“ ist für beide Modi belegt — bitte verschiedene Kombinationen wählen.",
            cursor_hotkey.to_handy_string()
        ));
    }

    let manager = MANAGER
        .get()
        .ok_or_else(|| "Hotkey-System noch nicht bereit".to_string())?
        .lock()
        .unwrap();

    let mut active = ACTIVE.lock().unwrap();
    let old: Vec<(HotkeyId, OutputMode)> = active.clone();
    for (id, _) in &old {
        let _ = manager.unregister(*id);
    }

    let new = [(cursor_hotkey, OutputMode::Cursor), (handoff_hotkey, OutputMode::Handoff)];
    let mut registered: Vec<(HotkeyId, OutputMode)> = Vec::new();
    for (hotkey, mode) in new {
        match manager.register(hotkey) {
            Ok(id) => registered.push((id, mode)),
            Err(e) => {
                // Rollback: teilweise registrierte wieder entfernen, alte Belegung zurück.
                for (id, _) in &registered {
                    let _ = manager.unregister(*id);
                }
                for (id, mode) in &old {
                    let hotkey = manager
                        .get_hotkey(*id)
                        .expect("alte Hotkey-ID muss auflösbar sein");
                    match manager.register(hotkey) {
                        Ok(rid) => registered.push((rid, *mode)),
                        Err(e2) => error!("Rollback-Registrierung fehlgeschlagen: {e2}"),
                    }
                }
                *active = registered;
                return Err(format!(
                    "„{}“ konnte nicht registriert werden: {e}",
                    hotkey.to_handy_string()
                ));
            }
        }
    }
    *active = registered;
    info!("Hotkeys umgestellt: {cursor} → Cursor · {handoff} → Hand-off");
    Ok(())
}

/// Eine Kombination prüfen: parsebar, kein Escape (Reserve für Abbruch), und
/// ohne Modifier nur mit F-Taste — nackte Buchstaben/Zahlen würden systemweit
/// blockt und machen damit andere Apps unbedienbar.
fn validate(hotkey_str: &str) -> Result<Hotkey, String> {
    let hotkey: Hotkey = hotkey_str
        .parse()
        .map_err(|e| format!("„{hotkey_str}“ ist keine gültige Kombination: {e}"))?;
    let Some(key) = hotkey.key else {
        return Ok(hotkey); // Modifier-only ist erlaubt (z. B. nur Strg+Alt)
    };
    if key == Key::Escape {
        return Err("Escape ist für „Aufnahme abbrechen“ reserviert.".to_string());
    }
    if hotkey.modifiers.is_empty() && !is_function_key(&format!("{key}")) {
        return Err(
            "Ohne Modifier (Strg/Alt/Win/Shift) würde die Taste in allen anderen Apps \
             blockiert — bitte einen Modifier mit drücken oder eine F-Taste wählen."
                .to_string(),
        );
    }
    Ok(hotkey)
}

fn is_function_key(display: &str) -> bool {
    display.len() >= 2
        && display.starts_with('F')
        && display[1..].chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Repro für den Capture-Bug vom 25.10.: Der Key-Capture sieht von
    /// handy-keys stets seitenspezifische Modifier (CtrlLeft/OptLeft …). Der
    /// zurückgegebene String muss daraus Compound-Modifier machen — sonst
    /// matchte ein gespeichertes „ctrl_left+alt_left+m" nur auf die linke
    /// Seite und das Label zeigte den Rohstring.
    #[test]
    fn capture_normalisiert_seiten_modifikatoren() {
        let hotkey = Hotkey::new(
            generalize_modifiers(Modifiers::CTRL_LEFT | Modifiers::OPT_LEFT),
            Some(Key::M),
        )
        .unwrap();
        assert_eq!(hotkey.to_handy_string(), "ctrl+alt+m");

        // Roundtrip: Der String muss wieder parsebar sein und danach BEIDE
        // Seiten matchen (wie die Defaults „ctrl+alt+space").
        let round: Hotkey = hotkey.to_handy_string().parse().unwrap();
        assert!(round.modifiers.matches(Modifiers::CTRL_LEFT | Modifiers::OPT_LEFT));
        assert!(round.modifiers.matches(Modifiers::CTRL_RIGHT | Modifiers::OPT_RIGHT));
    }

    #[test]
    fn generalize_alle_gruppen() {
        assert_eq!(generalize_modifiers(Modifiers::empty()), Modifiers::empty());
        assert_eq!(generalize_modifiers(Modifiers::SHIFT_LEFT), Modifiers::SHIFT);
        assert_eq!(generalize_modifiers(Modifiers::CMD_RIGHT), Modifiers::CMD);
        assert_eq!(
            generalize_modifiers(Modifiers::CTRL_LEFT | Modifiers::OPT_RIGHT | Modifiers::FN),
            Modifiers::CTRL | Modifiers::OPT | Modifiers::FN,
        );
        // Bereits compound Modifier bleiben unverändert.
        assert_eq!(generalize_modifiers(Modifiers::CTRL), Modifiers::CTRL);
    }

    /// Modifier-only-Capture (z. B. nur Strg+Alt gedrückt und losgelassen)
    /// muss ebenfalls compound normalisiert werden.
    #[test]
    fn capture_modifier_only_wird_compound() {
        let hotkey = Hotkey::new(
            generalize_modifiers(Modifiers::CTRL_LEFT | Modifiers::OPT_RIGHT),
            None,
        )
        .unwrap();
        assert_eq!(hotkey.to_handy_string(), "ctrl+alt");
        // Als Muster „ctrl+alt" muss das gemischte Seiten-Event (links Strg,
        // rechts Alt) genauso matchen wie die Reinform rechts+rechts.
        let round: Hotkey = hotkey.to_handy_string().parse().unwrap();
        assert!(round.modifiers.matches(Modifiers::CTRL_LEFT | Modifiers::OPT_RIGHT));
        assert!(round.modifiers.matches(Modifiers::CTRL_RIGHT | Modifiers::OPT_RIGHT));
    }

    #[test]
    fn gueltige_kombinationen() {
        assert!(validate("ctrl+alt+space").is_ok());
        assert!(validate("ctrl+alt+h").is_ok());
        assert!(validate("shift+f9").is_ok());
        assert!(validate("f9").is_ok()); // F-Taste ohne Modifier ist erlaubt
        assert!(validate("ctrl+alt").is_ok()); // Modifier-only
    }

    #[test]
    fn nackte_taste_wird_abgelehnt() {
        let err = validate("a").unwrap_err();
        assert!(err.contains("Modifier"));
        assert!(validate("9").is_err());
    }

    #[test]
    fn escape_ist_reserviert() {
        assert!(validate("escape").is_err());
        assert!(validate("ctrl+esc").is_err());
    }

    #[test]
    fn unparsebares_wird_abgelehnt() {
        assert!(validate("cmd+q+>").is_err());
        assert!(validate("").is_err());
    }

    #[test]
    fn f_tasten_erkennen() {
        assert!(is_function_key("F1"));
        assert!(is_function_key("F24"));
        assert!(!is_function_key("F"));
        assert!(!is_function_key("Foo"));
        assert!(!is_function_key("A"));
    }

    #[test]
    fn capture_roundtrip_liefert_parsebaren_string() {
        // Der Capture liefert to_handy_string(); der muss durch from_str wieder
        // in dasselbe Hotkey-Objekt parsebar sein (Persistenz + Registrierung).
        for s in ["ctrl+alt+space", "shift+f9", "ctrl+alt", "super+shift+m"] {
            let hotkey: Hotkey = s.parse().unwrap();
            let round: Hotkey = hotkey.to_handy_string().parse().unwrap();
            assert_eq!(hotkey.to_handy_string(), round.to_handy_string());
        }
    }
}

/// Key-Capture starten (asynchron): Ergebnis via Event `pw://hotkey-captured`,
/// Payload `{ "hotkey": "<kombination>" }` oder `null` bei Abbruch/Timeout.
/// Escape bricht ab (specs 4.3). Läuft schon ein Capture, wird der Aufruf
/// ignoriert.
pub fn start_capture(app: AppHandle) {
    if CAPTURE_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    CAPTURE_CANCEL.store(false, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("pw-hotkey-capture".into())
        .spawn(move || {
            let result = capture_once();
            CAPTURE_RUNNING.store(false, Ordering::SeqCst);
            if let Some(hotkey) = &result {
                info!("Key-Capture: {hotkey}");
            } else {
                info!("Key-Capture abgebrochen (Escape/Timeout/Abbruch-Befehl)");
            }
            let _ = app.emit(
                "pw://hotkey-captured",
                serde_json::json!({ "hotkey": result }),
            );
        })
        .map_err(|e| {
            error!("Capture-Thread konnte nicht gestartet werden: {e}");
            CAPTURE_RUNNING.store(false, Ordering::SeqCst);
            e
        })
        .ok();
}

/// Laufenden Capture abbrechen.
pub fn cancel_capture() {
    CAPTURE_CANCEL.store(true, Ordering::SeqCst);
}

/// Auf eine sinnvolle Kombination warten. Zwei Fälle:
/// 1. Ein Nicht-Modifier-Key wird gedrückt → Kombination aus den aktuell
///    gehaltenen Modifiern + Taste.
/// 2. Nur Modifier werden gedrückt und wieder losgelassen → Modifier-only-
///    Kombination (z. B. nur Strg+Alt). Dafür akkumulieren wir die Modifier
///    über alle key_down-Events, weil beim Loslassen der erste schon fehlt.
fn capture_once() -> Option<String> {
    let listener = match KeyboardListener::new() {
        Ok(l) => l,
        Err(e) => {
            error!("Capture-Listener fehlgeschlagen: {e}");
            return None;
        }
    };

    let deadline = Instant::now() + CAPTURE_TIMEOUT;
    let mut seen_modifiers = Modifiers::empty();
    let mut saw_key = false;

    loop {
        if CAPTURE_CANCEL.load(Ordering::SeqCst) || Instant::now() >= deadline {
            return None;
        }
        match listener.recv_timeout(Duration::from_millis(100)) {
            Ok(event) if !event.is_key_down => {
                // Modifier-only: Modifier losgelassen, ohne dass eine echte
                // Taste dazwischen lag → die akkumulierten Modifier sind der
                // Hotkey.
                if !saw_key && !seen_modifiers.is_empty() {
                    return Hotkey::new(generalize_modifiers(seen_modifiers), None)
                        .ok()
                        .map(|h| h.to_handy_string());
                }
            }
            Ok(event) => {
                if let Some(key) = event.key {
                    if key == Key::Escape {
                        return None;
                    }
                    saw_key = true;
                    if let Ok(hotkey) = Hotkey::new(generalize_modifiers(event.modifiers), key) {
                        return Some(hotkey.to_handy_string());
                    }
                } else {
                    seen_modifiers |= event.modifiers;
                }
            }
            Err(_) => continue, // kurz vor Deadline — Schleifenkopf beendet rechtzeitig
        }
    }
}

/// Seitenspezifische Modifier (Strg links, Alt rechts, …) auf die Compound-
/// Aliase zusammenziehen. Der Capture sieht je nach Tastatur immer nur die
/// konkrete Seite; gespeichert werden soll aber „ctrl" (matched beide Seiten,
/// wie die Defaults) — sonst würde z. B. rechte Strg beim Diktieren ins Leere
/// laufen, und das Label zeigte „ctrl_left+alt_left+m".
fn generalize_modifiers(m: Modifiers) -> Modifiers {
    let mut out = Modifiers::empty();
    for (left, right, compound) in [
        (Modifiers::CMD_LEFT, Modifiers::CMD_RIGHT, Modifiers::CMD),
        (Modifiers::SHIFT_LEFT, Modifiers::SHIFT_RIGHT, Modifiers::SHIFT),
        (Modifiers::CTRL_LEFT, Modifiers::CTRL_RIGHT, Modifiers::CTRL),
        (Modifiers::OPT_LEFT, Modifiers::OPT_RIGHT, Modifiers::OPT),
    ] {
        if m.intersects(left | right) {
            out |= compound;
        }
    }
    if m.contains(Modifiers::FN) {
        out |= Modifiers::FN;
    }
    out
}
