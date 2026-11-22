//! Globale Push-to-Talk-Hotkeys über handy-keys (Low-Level-Hook, key-up-fähig).
//!
//! M1: Hotkeys sind frei konfigurierbar (specs 4.3, Whispering-Stil). Die
//! Belegung kommt aus den Settings und wird zur Laufzeit umgehängt.
//!
//! Es gibt genau EINEN KeyboardListener, erstellt beim App-Start, dessen
//! Event-Loop die PTT-Kanten der registrierten Hotkeys verarbeitet. Der
//! Key-Capture für die Einstellungen lebt bewusst im Frontend (Webview-
//! Keydown, siehe settings.tsx): Der Hook ist dort nicht zuverlässig
//! erreichbar (Installation in manchen Prozess-Kontexten eine Lotterie,
//! Bug 25.10.; UIPI) — Entscheidung als ADR 0001 festgehalten.
//!
//! Die Kanten-Logik ist hier eigenständig implementiert (reine Funktionen,
//! unit-getestet); handy-keys liefert nur die Roh-Events und das Blockieren
//! registrierter Kombinationen gegenüber anderen Apps.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use handy_keys::{BlockingHotkeys, Hotkey, Key, KeyboardListener, KeyEvent};
use log::{error, info, warn};
use tauri::{AppHandle, Emitter};

use crate::OutputMode;

/// Defaults für die erste Nutzung (M0-Belegung).
pub const DEFAULT_CURSOR_HOTKEY: &str = "ctrl+alt+space";
pub const DEFAULT_HANDOFF_HOTKEY: &str = "ctrl+alt+h";

/// `set_hotkeys` aus abfragbar. mpsc-Receiver ist nicht Sync, daher im Mutex.
/// Blockier-Satz des Listeners: registrierte PTT-Kombinationen frisst der
/// Hook, damit sie in anderen Apps keine Shortcuts auslösen.
static BLOCKING: OnceLock<BlockingHotkeys> = OnceLock::new();

/// Aktive PTT-Belegung und gedrückt-Zustand je Modus. Lock-Reihenfolge:
/// erst PRESSED, dann ACTIVE (verhindert Deadlocks).
static ACTIVE: Mutex<Vec<(Hotkey, OutputMode)>> = Mutex::new(Vec::new());
static PRESSED: Mutex<Vec<OutputMode>> = Mutex::new(Vec::new());

/// Tastatur-Hook verbunden (Selbsttest bestanden)? UI-Status via
/// `pw://hook-status` + Command `hook_status`.
static HOOK_READY: AtomicBool = AtomicBool::new(false);

pub fn start(app: AppHandle) -> anyhow::Result<()> {
    let settings = crate::settings::current();
    let cursor = settings.hotkeys.cursor.clone();
    let handoff = settings.hotkeys.handoff.clone();

    let blocking: BlockingHotkeys = Arc::new(Mutex::new(HashSet::new()));
    if BLOCKING.set(blocking).is_err() {
        error!("Blockier-Satz bereits initialisiert");
        return Ok(());
    }

    if let Err(msg) = set_hotkeys(&cursor, &handoff) {
        error!("Hotkey-Belegung aus Settings fehlgeschlagen: {msg}");
        // Reset-Versuch auf die Defaults, damit die App bedienbar bleibt.
        if set_hotkeys(DEFAULT_CURSOR_HOTKEY, DEFAULT_HANDOFF_HOTKEY).is_ok() {
            warn!("Auf Default-Hotkeys zurückgefallen ({DEFAULT_CURSOR_HOTKEY}, {DEFAULT_HANDOFF_HOTKEY})");
        }
    }
    info!(
        "PTT-Hotkeys aktiv: {cursor} → Cursor · {handoff} → Hand-off (halten zum Diktieren)"
    );

    std::thread::Builder::new()
        .name("pw-hotkey-watchdog".into())
        .spawn(move || {
            // Die Loop kann sterben (Panic, z. B. vergiftetes Lock) — das war
            // am 25.10. unsichtbar, weil Panics nur auf die Konsole gingen.
            // Der Watchdog startet sie neu; die Panic selbst loggt der
            // Panic-Hook in lib.rs.
            let mut backoff = Duration::from_secs(1);
            loop {
                let app = app.clone();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_hotkey_loop(app)
                }));
                error!(
                    "Hotkey-Loop beendet ({:?}) — Neustart in {backoff:?}",
                    result.map_err(|_| "PANIK")
                );
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        })
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("Watchdog-Thread konnte nicht gestartet werden: {e}"))?;
    Ok(())
}

fn run_hotkey_loop(app: AppHandle) {
    // Loslassen-Events können verloren gehen (z. B. Hook-Neuinstallation) —
    // gedrückte Modi zurücksetzen, sonst schluckt der nächste Druck keine
    // Down-Kante mehr und der Hotkey wirkt tot.
    PRESSED.lock().unwrap().clear();
    // Der Hook-Thread kann in manchen Prozess-Kontexten bei der
    // Installation sterben (Bug 25.10.: taube Instanzen, v. a. aus dem
    // IDE-Terminal; Ursache jenseits unserer Kontrolle — Token/Job/
    // Drosselung unauffällig). Gegenmaßnahmen: Selbsttest per
    // F24-Injektion, Neuinstallation bis der Test besteht (in der Startphase
    // schnell: ~1 s/Takt, sonst 2 s), Status-Event für die UI und lautes
    // Loggen statt stiller Taubheit.
    let blocking = BLOCKING.get().expect("BLOCKING ist gesetzt").clone();
    let mut listener: Option<KeyboardListener> = None;
    let mut listener_since = Instant::now();
    let mut reconnect_wait_until = Instant::now();
    let mut connected = false; // erster Selbsttest/Echt-Ereignis war erfolgreich
    let mut last_event_at = Instant::now();
    let mut probe_sent_at: Option<Instant> = None;
    let mut probe_baseline = Instant::now();
    let mut failed_attempts: u32 = 0;
    let mut blocked_reports: u32 = 0; // durch elevated Vordergrund blockte Tests
    let mut next_probe_after = Instant::now();

    let set_ready = |ready: bool| {
        if HOOK_READY.swap(ready, Ordering::SeqCst) != ready {
            let _ = app.emit("pw://hook-status", serde_json::json!({ "ready": ready }));
        }
    };
    set_ready(false);

    loop {
            // Empfangen (10-ms-Takt) und Kanten verarbeiten.
            if let Some(l) = listener.as_ref() {
                let mut died = false;
                loop {
                    match l.recv_timeout(Duration::from_millis(10)) {
                        Ok(ev) => {
                            last_event_at = Instant::now();
                            if !connected {
                                connected = true;
                                failed_attempts = 0;
                                set_ready(true);
                                info!("Tastatur-Hook verbunden (Selbsttest bestanden)");
                            }
                            handle_event(&app, &ev);
                        }
                        Err(handy_keys::Error::Timeout) => break,
                        Err(_) => {
                            // Kanal getrennt: Hook-Thread wurde beendet.
                            died = true;
                            break;
                        }
                    }
                }
                if died {
                    if failed_attempts == 0 {
                        error!(
                            "Tastatur-Hook-Thread wurde beendet — Hook-Installation ist in \
                             diesem Prozess-Kontext fehlgeschlagen. Neuinstallation, bis der \
                             Selbsttest besteht."
                        );
                    }
                    failed_attempts += 1;
                    listener = None;
                    set_ready(false);
                    reconnect_wait_until =
                        Instant::now() + if connected { Duration::from_secs(2) } else { Duration::from_millis(250) };
                }
            }

            // Listener (erneut) installieren.
            if listener.is_none() && Instant::now() >= reconnect_wait_until {
                match KeyboardListener::new_with_blocking(blocking.clone()) {
                    Ok(l) => {
                        listener = Some(l);
                        listener_since = Instant::now();
                        last_event_at = Instant::now();
                        probe_sent_at = None;
                    }
                    Err(e) => {
                        if failed_attempts == 0 {
                            error!("KeyboardListener fehlgeschlagen: {e} — Diktat reagiert nicht");
                            failed_attempts += 1;
                        }
                        reconnect_wait_until =
                            Instant::now() + if connected { Duration::from_secs(2) } else { Duration::from_millis(250) };
                    }
                }
            }

            // Selbsttest: eine harmlose F24 injizieren (praktisch nirgends
            // belegt) und prüfen, ob sie den eigenen Hook erreicht. In der
            // Startphase nach 300 ms; nach dem ersten Erfolg nur noch bei
            // langer Stille (120 s), um auch späteres Stillwerden zu
            // bemerken.
            let probe_delay = if connected {
                Duration::from_secs(120)
            } else {
                Duration::from_millis(300)
            };
            let silence = last_event_at.elapsed();
            let want_probe = listener.is_some()
                && probe_sent_at.is_none()
                && Instant::now() >= next_probe_after
                && listener_since.elapsed() >= Duration::from_millis(300)
                && ((!connected && listener_since.elapsed() >= probe_delay)
                    || (connected && silence >= Duration::from_secs(120)));
            if want_probe {
                send_self_probe();
                probe_baseline = last_event_at;
                probe_sent_at = Some(Instant::now());
            }
            if let Some(at) = probe_sent_at {
                if at.elapsed() >= Duration::from_millis(500) {
                    if last_event_at > probe_baseline {
                        // Ein Ereignis (F24 oder echte Taste) ist angekommen.
                        if !connected {
                            connected = true;
                            failed_attempts = 0;
                            set_ready(true);
                            info!("Tastatur-Hook verbunden (Selbsttest bestanden)");
                        }
                    } else {
                        let (fg_exe, fg_elevated) = foreground_exe_and_elevated();
                        if fg_elevated {
                            // Der Test ist durch UIPI blockt (elevated Fenster im
                            // Vordergrund blockt NUR injizierte Tasten, nicht
                            // echte). Der Hook kann gesund sein — nicht neu
                            // installieren, in 5 s erneut prüfen.
                            if blocked_reports % 10 == 0 {
                                info!(
                                    "Selbsttest durch elevated Vordergrund ({fg_exe}) blockiert — \
                                     Hook bleibt installiert, Prüfung wiederholt sich \
                                     (Versuch {})",
                                    blocked_reports + 1
                                );
                            }
                            blocked_reports += 1;
                            next_probe_after = Instant::now() + Duration::from_secs(5);
                        } else {
                            failed_attempts += 1;
                            blocked_reports = 0;
                            if failed_attempts == 1 {
                                warn!(
                                    "Hook-Selbsttest FEHLGESCHLAGEN (Vordergrund: {fg_exe}): das \
                                     eigene F24-Ereignis wurde nicht gesehen. Der Hook wird neu \
                                     installiert, bis es klappt — Diktat ist erst danach nutzbar \
                                     (in der UI als „wird verbunden“ sichtbar)."
                                );
                            } else if failed_attempts % 10 == 0 {
                                info!("Hook weiterhin taub (Versuch {failed_attempts}) — Neuinstallation läuft weiter");
                            }
                            listener = None; // Neuinstallation erzwingen
                            connected = false; // zügige Prüfung des neuen Hooks
                            set_ready(false);
                            reconnect_wait_until = Instant::now() + Duration::from_millis(250);
                        }
                    }
                    probe_sent_at = None;
                }
            }

            if listener.is_none() {
                std::thread::sleep(Duration::from_millis(50));
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
}

/// Ist der Tastatur-Hook aktuell verbunden (Selbsttest bestanden)?
pub fn hook_ready() -> bool {
    HOOK_READY.load(Ordering::SeqCst)
}

/// Harmloses F24 (down+up) injizieren — der Hook-Selbsttest muss es sehen.
fn send_self_probe() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY, VK_F24,
    };
    fn ev(vk: VIRTUAL_KEY, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }
    let seq = [ev(VK_F24, false), ev(VK_F24, true)];
    unsafe {
        SendInput(&seq, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Aktuell aktive Belegung (aus der Registrierung, nicht aus den Settings).
pub fn current() -> (String, String) {
    let active = ACTIVE.lock().unwrap();
    let mut cursor = None;
    let mut handoff = None;
    for (hotkey, mode) in active.iter() {
        match mode {
            OutputMode::Cursor => cursor = Some(hotkey.to_handy_string()),
            OutputMode::Handoff => handoff = Some(hotkey.to_handy_string()),
        }
    }
    (
        cursor.unwrap_or_else(|| DEFAULT_CURSOR_HOTKEY.into()),
        handoff.unwrap_or_else(|| DEFAULT_HANDOFF_HOTKEY.into()),
    )
}

/// Beide PTT-Hotkeys validieren und atomar umhängen. Die Registrierung ist
/// reine Datenpflege (Kanten-Logik sieht die neue Belegung im nächsten Event)
/// und kann nicht mehr teilweise scheitern — ein Rollback wie beim alten
/// Manager entfällt.
pub fn set_hotkeys(cursor: &str, handoff: &str) -> Result<(), String> {
    let cursor_hotkey = validate(cursor)?;
    let handoff_hotkey = validate(handoff)?;
    if cursor_hotkey.to_handy_string() == handoff_hotkey.to_handy_string() {
        return Err(format!(
            "„{}“ ist für beide Modi belegt — bitte verschiedene Kombinationen wählen.",
            cursor_hotkey.to_handy_string()
        ));
    }

    let blocking = BLOCKING
        .get()
        .ok_or_else(|| "Hotkey-System noch nicht bereit".to_string())?;

    let mut active = ACTIVE.lock().unwrap();
    let mut pressed = PRESSED.lock().unwrap();
    let mut set = blocking.lock().unwrap();
    for (old, _) in active.iter() {
        set.remove(old);
    }
    *active = vec![
        (cursor_hotkey, OutputMode::Cursor),
        (handoff_hotkey, OutputMode::Handoff),
    ];
    // Bei Wechsel der Belegung gedrückte Modi zurücksetzen: das Loslassen
    // der alten Kombination darf die neue nicht klemmen.
    pressed.clear();
    for (new, _) in active.iter() {
        set.insert(*new);
    }
    info!("Hotkeys umgestellt: {cursor} → Cursor · {handoff} → Hand-off");
    Ok(())
}

/// Eine Kombination prüfen: parsebar, kein Escape (Reserve für Abbruch), und
/// ohne Modifier nur mit F-Taste — nackte Buchstaben/Zahlen würden systemweit
/// blockiert und machen damit andere Apps unbedienbar.
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

/// Ein Key-Event verarbeiten: PTT-Kanten (Down/Up je Modus).
fn handle_event(app: &AppHandle, ev: &KeyEvent) {
    let edges = {
        let mut pressed = PRESSED.lock().unwrap();
        ptt_edges(&ACTIVE.lock().unwrap(), &mut pressed, ev)
    };
    for (mode, down) in edges {
        match (mode, down) {
            // Hand-off ist ein Toggle (specs 4.3): Antippen startet bzw.
            // beendet die Aufnahme — die Release-Kante (Loslassen nach dem
            // Antippen) gehört bewusst NICHT zur Steuerung, sonst wäre die
            // Aufnahme nach dem Start sofort wieder vorbei.
            (OutputMode::Handoff, true) => crate::on_handoff_toggle(app),
            (OutputMode::Handoff, false) => {}
            // Cursor behält klassisches Push-to-Talk: halten = aufnehmen.
            (OutputMode::Cursor, true) => crate::on_ptt_down(app, mode),
            (OutputMode::Cursor, false) => crate::on_ptt_up(app),
        }
    }
}

/// PTT-Kanten aus einem Roh-Event ableiten (Eigenimplementierung nach dem
/// Manager-Muster): Down bei passendem key_down, wenn noch nicht gedrückt;
/// Up beim key_up derselben Taste oder wenn ein Modifier-Event die
/// Kombination nicht mehr abbildet (z. B. Strg losgelassen). Liefert
/// (Modus, gedrückt?)-Wechsel in Belegungsreihenfolge.
fn ptt_edges(
    active: &[(Hotkey, OutputMode)],
    pressed: &mut Vec<OutputMode>,
    ev: &KeyEvent,
) -> Vec<(OutputMode, bool)> {
    let mut edges = Vec::new();
    for (hotkey, mode) in active {
        let matches = hotkey.modifiers.matches(ev.modifiers) && hotkey.key == ev.key;
        let is_down = pressed.contains(mode);
        if ev.is_key_down {
            if matches && !is_down {
                pressed.push(*mode);
                edges.push((*mode, true));
            }
        } else {
            let releases = (ev.key.is_some() && hotkey.key == ev.key)
                || (ev.key.is_none() && !hotkey.modifiers.matches(ev.modifiers));
            if is_down && releases {
                pressed.retain(|m| m != mode);
                edges.push((*mode, false));
            }
        }
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use handy_keys::Modifiers;

    fn ev_down(mods: Modifiers, key: Option<Key>) -> KeyEvent {
        KeyEvent {
            modifiers: mods,
            key,
            is_key_down: true,
            changed_modifier: None,
        }
    }

    fn ev_up(mods: Modifiers, key: Option<Key>) -> KeyEvent {
        KeyEvent {
            modifiers: mods,
            key,
            is_key_down: false,
            changed_modifier: None,
        }
    }

    fn active_single(hotkey: &str, mode: OutputMode) -> Vec<(Hotkey, OutputMode)> {
        vec![(hotkey.parse().unwrap(), mode)]
    }

    /// Seitengebunden vs. Compound: „ctrl_right+space" matcht nur die rechte
    /// Strg, „ctrl+alt" (Defaults-Stil) weiterhin beide Seiten.
    #[test]
    fn seitige_und_compound_muster_matchen_korrekt() {
        let right: Hotkey = "ctrl_right+space".parse().unwrap();
        assert!(right.modifiers.matches(Modifiers::CTRL_RIGHT));
        assert!(!right.modifiers.matches(Modifiers::CTRL_LEFT));

        // PTT-Kanten mit rechts: Down nur bei echter rechter Strg.
        let active = active_single("ctrl_right+space", OutputMode::Cursor);
        let mut pressed = Vec::new();
        assert!(
            ptt_edges(&active, &mut pressed, &ev_down(Modifiers::CTRL_LEFT, Some(Key::Space))).is_empty(),
            "linke Strg darf nicht feuern"
        );
        assert_eq!(
            ptt_edges(&active, &mut pressed, &ev_down(Modifiers::CTRL_RIGHT, Some(Key::Space))),
            vec![(OutputMode::Cursor, true)]
        );
        assert_eq!(
            ptt_edges(&active, &mut pressed, &ev_up(Modifiers::CTRL_RIGHT, Some(Key::Space))),
            vec![(OutputMode::Cursor, false)]
        );

        // Modifier-only „nur rechte Strg" als PTT: Down beim Runterdrücken,
        // Up beim Loslassen (nutzt die im Alltag unbenutzte Taste als
        // Ein-Tasten-Diktat). Nach dem key_up ist kein Modifier mehr gesetzt.
        let active = active_single("ctrl_right", OutputMode::Cursor);
        let mut pressed = Vec::new();
        assert_eq!(
            ptt_edges(&active, &mut pressed, &ev_down(Modifiers::CTRL_RIGHT, None)),
            vec![(OutputMode::Cursor, true)]
        );
        assert_eq!(
            ptt_edges(&active, &mut pressed, &ev_up(Modifiers::empty(), None)),
            vec![(OutputMode::Cursor, false)]
        );
        // Linke Strg löst nichts aus — auch kein versehentliches Loslassen.
        let mut pressed = Vec::new();
        assert!(ptt_edges(&active, &mut pressed, &ev_down(Modifiers::CTRL_LEFT, None)).is_empty());
        assert!(pressed.is_empty());
        // Wird während der Aufnahme die linke statt der rechten Strg
        // gehalten, zerfällt die Kombination → Up-Kante.
        let mut pressed = vec![OutputMode::Cursor];
        assert_eq!(
            ptt_edges(&active, &mut pressed, &ev_up(Modifiers::CTRL_LEFT, None)),
            vec![(OutputMode::Cursor, false)]
        );
    }

    #[test]
    fn rctrl_ist_alias_fuer_ctrl_right() {
        let short: Hotkey = "rctrl".parse().unwrap();
        let long: Hotkey = "ctrl_right".parse().unwrap();
        assert_eq!(short.to_handy_string(), long.to_handy_string());
        assert!(validate("rctrl").is_ok());
        assert!(validate("rctrl+space").is_ok());
        assert!(validate("lalt+shift").is_ok());
    }

    /// Compound-Belegungen (Form der alten Defaults) matchen weiterhin
    /// gemischte Seiten — alte Settings bleiben funktional gleich.
    #[test]
    fn compound_muster_matchen_gemischte_seiten() {
        let round: Hotkey = "ctrl+alt".parse().unwrap();
        assert!(round.modifiers.matches(Modifiers::CTRL_LEFT | Modifiers::OPT_RIGHT));
        assert!(round.modifiers.matches(Modifiers::CTRL_RIGHT | Modifiers::OPT_RIGHT));
    }

    /// PTT: Down bei passendem key_down, Up beim key_up derselben Taste.
    #[test]
    fn ptt_kanten_down_up() {
        let active = active_single("ctrl+alt+space", OutputMode::Cursor);
        let mut pressed = Vec::new();

        // Nicht passend: nur Alt gedrückt.
        assert!(ptt_edges(&active, &mut pressed, &ev_down(Modifiers::OPT_LEFT, None)).is_empty());
        // Strg dazu — immer noch nicht passend (Leertaste fehlt).
        assert!(
            ptt_edges(&active, &mut pressed, &ev_down(Modifiers::CTRL_LEFT | Modifiers::OPT_LEFT, None))
                .is_empty()
        );
        // Leertaste: Down-Kante.
        assert_eq!(
            ptt_edges(&active, &mut pressed, &ev_down(Modifiers::CTRL_LEFT | Modifiers::OPT_LEFT, Some(Key::Space))),
            vec![(OutputMode::Cursor, true)]
        );
        // Auto-Repeat (weiteres down): keine zweite Kante.
        assert!(ptt_edges(&active, &mut pressed, &ev_down(Modifiers::CTRL_LEFT | Modifiers::OPT_LEFT, Some(Key::Space))).is_empty());
        // Leertaste loslassen: Up-Kante.
        assert_eq!(
            ptt_edges(&active, &mut pressed, &ev_up(Modifiers::CTRL_LEFT | Modifiers::OPT_LEFT, Some(Key::Space))),
            vec![(OutputMode::Cursor, false)]
        );
    }

    /// PTT: Modifier-only-Hotkey (z. B. nur Strg+Alt) feuert beim vollständigen
    /// Halten und wird freigegeben, sobald ein Modifier losgelassen wird.
    #[test]
    fn ptt_modifier_only_hotkey() {
        let active = active_single("ctrl+alt", OutputMode::Cursor);
        let mut pressed = Vec::new();

        assert!(ptt_edges(&active, &mut pressed, &ev_down(Modifiers::CTRL_RIGHT, None)).is_empty());
        assert_eq!(
            ptt_edges(&active, &mut pressed, &ev_down(Modifiers::CTRL_RIGHT | Modifiers::OPT_RIGHT, None)),
            vec![(OutputMode::Cursor, true)]
        );
        // Ein regulärer Tastendruck während des Haltens ändert nichts.
        assert!(ptt_edges(&active, &mut pressed, &ev_down(Modifiers::CTRL_RIGHT | Modifiers::OPT_RIGHT, Some(Key::M))).is_empty());
        // Alt loslassen: Kombination nicht mehr komplett → Up-Kante.
        assert_eq!(
            ptt_edges(&active, &mut pressed, &ev_up(Modifiers::CTRL_RIGHT, None)),
            vec![(OutputMode::Cursor, false)]
        );
    }

    /// PTT: Wird Strg während einer gehaltenen Kombination losgelassen, muss
    /// die Up-Kante kommen (sonst „klemmt" die Aufnahme).
    #[test]
    fn ptt_release_bei_verlorenem_modifier() {
        let active = active_single("ctrl+alt+space", OutputMode::Cursor);
        let mut pressed = vec![OutputMode::Cursor];

        let edges = ptt_edges(
            &active,
            &mut pressed,
            &ev_up(Modifiers::OPT_LEFT, None), // Strg-up: Kombination zerfällt
        );
        assert_eq!(edges, vec![(OutputMode::Cursor, false)]);
        assert!(pressed.is_empty());
    }

    /// Zwei Modi gleichzeitig gedrückt: jede Kante separat.
    #[test]
    fn ptt_zwei_hotkeys_unabhängig() {
        let active = vec![
            ("ctrl+alt+space".parse().unwrap(), OutputMode::Cursor),
            ("ctrl+alt+h".parse().unwrap(), OutputMode::Handoff),
        ];
        let mut pressed = Vec::new();

        assert!(ptt_edges(&active, &mut pressed, &ev_down(Modifiers::CTRL_LEFT | Modifiers::OPT_LEFT, None)).is_empty());
        assert_eq!(
            ptt_edges(&active, &mut pressed, &ev_down(Modifiers::CTRL_LEFT | Modifiers::OPT_LEFT, Some(Key::H))),
            vec![(OutputMode::Handoff, true)]
        );
        assert!(pressed == vec![OutputMode::Handoff]);
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

/// Vordergrund-Fenster als (Prozessname, elevated?) — der Selbsttest muss
/// wissen, ob ein elevated Fenster NUR den Test blockt (UIPI blockt
/// SendInput, aber keine echten Tasten).
#[cfg(target_os = "windows")]
fn foreground_exe_and_elevated() -> (String, bool) {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{
        OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
        PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return ("<kein Fenster>".into(), false);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return (format!("<pid {pid}>"), false);
        };
        let mut name = [0u16; 260];
        let mut len = name.len() as u32;
        let exe = if QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(name.as_mut_ptr()), &mut len).is_ok() {
            String::from_utf16_lossy(&name[..len as usize])
        } else {
            format!("<pid {pid}>")
        };
        let mut elevated = false;
        let mut token = Default::default();
        if OpenProcessToken(handle, TOKEN_QUERY, &mut token).is_ok() {
            let mut elev = TOKEN_ELEVATION::default();
            let mut rl = 0u32;
            if GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut elev as *mut _ as *mut core::ffi::c_void),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut rl,
            )
            .is_ok()
            {
                elevated = elev.TokenIsElevated != 0;
            }
            let _ = CloseHandle(token);
        }
        let _ = CloseHandle(handle);
        (exe, elevated)
    }
}
