//! Globale Push-to-Talk-Hotkeys über handy-keys (Low-Level-Hook, key-up-fähig).
//!
//! M1: Hotkeys sind frei konfigurierbar (specs 4.3, Whispering-Stil). Die
//! Belegung kommt aus den Settings und wird zur Laufzeit umgehängt.
//!
//! Es gibt genau EINEN KeyboardListener, erstellt beim App-Start, und dessen
//! Event-Loop bedient beides: die PTT-Kanten der registrierten Hotkeys und den
//! Key-Capture (als Zustandsautomat, nicht als zweiter Hook). Ein früherer
//! Entwurf erstellte pro Capture einen zweiten Listener — der war in manchen
//! Umgebungen stumm (Bug 25.10.), während der beim Start erstellte Hook in
//! allen Instanzen zuverlässig lieferte. Die Kanten-Logik ist hier
//! eigenständig implementiert (reine Funktionen, unit-getestet); handy-keys
//! liefert nur die Roh-Events und das Blockieren registrierter Kombinationen
//! gegenüber anderen Apps.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use handy_keys::{BlockingHotkeys, Hotkey, Key, KeyboardListener, KeyEvent, Modifiers};
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

/// Abbruch-Flag für den laufenden Capture (setzt der UI-Command).
static CAPTURE_CANCEL: AtomicBool = AtomicBool::new(false);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);

/// Tastatur-Hook verbunden (Selbsttest bestanden)? UI-Status via
/// `pw://hook-status` + Command `hook_status`.
static HOOK_READY: AtomicBool = AtomicBool::new(false);

/// Laufender Key-Capture mit dem Zeitpunkt, an dem er abläuft.
struct ActiveCapture {
    app: AppHandle,
    deadline: Instant,
    eval: CaptureEval,
}
static CAPTURE: Mutex<Option<ActiveCapture>> = Mutex::new(None);

pub fn start(app: AppHandle) -> anyhow::Result<()> {
    let settings = crate::settings::load();
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
        .name("pw-hotkey".into())
        .spawn(move || {
            // Die Low-Level-Hook-Installation kann pro Start fehlgehen, ohne
            // dass der Listener-Thread stirbt (Bug 25.10.: taube Instanzen,
            // v. a. aus dem IDE-Terminal; Ursache jenseits unserer Kontrolle —
            // Token/Job/Drosselung unauffällig). Gegenmaßnahmen: Selbsttest
            // per F24-Injektion, Neuinstallation bis der Test besteht (in der
            // Startphase schnell: ~1 s/Takt, sonst 2 s), Status-Event für die
            // UI und lautes Loggen statt stiller Taubheit.
            let blocking = BLOCKING.get().expect("BLOCKING ist gesetzt").clone();
            let mut listener: Option<KeyboardListener> = None;
            let mut listener_since = Instant::now();
            let mut reconnect_wait_until = Instant::now();
            let mut connected = false; // erster Selbsttest/Echt-Ereignis war erfolgreich
            let mut last_event_at = Instant::now();
            let mut probe_sent_at: Option<Instant> = None;
            let mut probe_baseline = Instant::now();
            let mut failed_attempts: u32 = 0;

            let set_ready = |ready: bool| {
                if HOOK_READY.swap(ready, Ordering::SeqCst) != ready {
                    let _ = app.emit("pw://hook-status", serde_json::json!({ "ready": ready }));
                }
            };
            set_ready(false);

            loop {
                capture_tick();

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
                // bemerken. Während des Captures auslassen, damit die F24
                // nicht als neue Kombination landet.
                let probe_delay = if connected {
                    Duration::from_secs(120)
                } else {
                    Duration::from_millis(300)
                };
                let silence = last_event_at.elapsed();
                let want_probe = listener.is_some()
                    && probe_sent_at.is_none()
                    && listener_since.elapsed() >= Duration::from_millis(300)
                    && ((!connected && listener_since.elapsed() >= probe_delay)
                        || (connected && silence >= Duration::from_secs(120)));
                if want_probe && CAPTURE.lock().unwrap().is_none() {
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
                            failed_attempts += 1;
                            if failed_attempts == 1 {
                                warn!(
                                    "Hook-Selbsttest FEHLGESCHLAGEN: das eigene F24-Ereignis wurde \
                                     nicht gesehen. Der Hook wird neu installiert, bis es klappt — \
                                     Diktat/Hotkey-Änderung sind erst danach nutzbar (in der UI als \
                                     „wird verbunden“ sichtbar). Hinweis: ein elevated Fenster im \
                                     Vordergrund kann NUR den Test blocken (UIPI), echte Tasten \
                                     funktionieren dann trotzdem."
                                );
                            } else if failed_attempts % 10 == 0 {
                                info!("Hook weiterhin taub (Versuch {failed_attempts}) — Neuinstallation läuft weiter");
                            }
                            listener = None; // Neuinstallation erzwingen
                            set_ready(false);
                            reconnect_wait_until =
                                Instant::now() + if connected { Duration::from_secs(2) } else { Duration::from_millis(250) };
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
        })
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("Hotkey-Thread konnte nicht gestartet werden: {e}"))?;
    Ok(())
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
    let mut set = blocking.lock().unwrap();
    for (old, _) in active.iter() {
        set.remove(old);
    }
    *active = vec![
        (cursor_hotkey, OutputMode::Cursor),
        (handoff_hotkey, OutputMode::Handoff),
    ];
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

/// Ein Key-Event verarbeiten: Diagnose-Log während des Captures, Capture-
/// Bewertung, PTT-Kanten (Down/Up je Modus).
fn handle_event(app: &AppHandle, ev: &KeyEvent) {
    if CAPTURE.lock().unwrap().is_some() {
        info!(
            "[Diagnose] {} key={:?} mods={:?}",
            if ev.is_key_down { "key_down" } else { "key_up" },
            ev.key,
            ev.modifiers
        );
    }

    let mut done: Option<Option<String>> = None;
    {
        let mut cap = CAPTURE.lock().unwrap();
        if let Some(run) = cap.as_mut() {
            if let Some(result) = capture_eval(&mut run.eval, ev) {
                done = Some(result);
            }
        }
    }
    if let Some(result) = done {
        finish_capture(result);
    }

    let edges = {
        let mut pressed = PRESSED.lock().unwrap();
        ptt_edges(&ACTIVE.lock().unwrap(), &mut pressed, ev)
    };
    for (mode, down) in edges {
        if down {
            crate::on_ptt_down(app, mode);
        } else {
            crate::on_ptt_up(app);
        }
    }
}

/// Capture-Ablauf/Abbruch unabhängig von Events prüfen (Timeout).
fn capture_tick() {
    let expired = {
        let cap = CAPTURE.lock().unwrap();
        match cap.as_ref() {
            Some(run) => CAPTURE_CANCEL.load(Ordering::SeqCst) || Instant::now() >= run.deadline,
            None => false,
        }
    };
    if expired {
        if CAPTURE_CANCEL.load(Ordering::SeqCst) {
            info!("Key-Capture: Abbruch-Befehl (UI)");
        } else {
            info!("Key-Capture: 15 s Timeout ohne Taste");
        }
        finish_capture(None);
    }
}

/// Laufenden Capture beenden und das Ergebnis an die UI emittieren (null bei
/// Abbruch/Timeout). Tut nichts, wenn kein Capture läuft.
fn finish_capture(result: Option<String>) {
    let run = CAPTURE.lock().unwrap().take();
    let Some(run) = run else { return };
    CAPTURE_CANCEL.store(false, Ordering::SeqCst);
    if let Some(hotkey) = &result {
        info!("Key-Capture: {hotkey}");
    } else {
        info!("Key-Capture abgebrochen (Escape/Timeout/Abbruch-Befehl)");
    }
    let _ = run
        .app
        .emit("pw://hotkey-captured", serde_json::json!({ "hotkey": result }));
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

/// Zustand eines laufenden Captures.
struct CaptureEval {
    seen_modifiers: Modifiers,
    saw_key: bool,
}

impl CaptureEval {
    fn new() -> Self {
        Self {
            seen_modifiers: Modifiers::empty(),
            saw_key: false,
        }
    }
}

/// Ein Event im laufenden Capture bewerten; `Some(ergebnis)` beendet ihn
/// (Ergebnis null = Abbruch). Zwei Fälle: Ein Nicht-Modifier-Key beendet mit
/// der Kombination aus gehaltenen Modifiern + Taste; nur Modifier gedrückt und
/// wieder losgelassen ergeben eine Modifier-only-Kombination (dafür werden
/// die Modifier über alle key_down-Events akkumuliert, weil beim Loslassen
/// der erste schon fehlt).
fn capture_eval(st: &mut CaptureEval, ev: &KeyEvent) -> Option<Option<String>> {
    if !ev.is_key_down {
        if !st.saw_key && !st.seen_modifiers.is_empty() {
            return Some(
                Hotkey::new(generalize_modifiers(st.seen_modifiers), None)
                    .ok()
                    .map(|h| h.to_handy_string()),
            );
        }
        return None;
    }
    if let Some(key) = ev.key {
        if key == Key::Escape {
            return Some(None);
        }
        st.saw_key = true;
        if let Ok(hotkey) = Hotkey::new(generalize_modifiers(ev.modifiers), key) {
            return Some(Some(hotkey.to_handy_string()));
        }
        return None;
    }
    st.seen_modifiers |= ev.modifiers;
    None
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
        (
            Modifiers::SHIFT_LEFT,
            Modifiers::SHIFT_RIGHT,
            Modifiers::SHIFT,
        ),
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

/// Key-Capture starten (asynchron): Ergebnis via Event `pw://hotkey-captured`,
/// Payload `{ "hotkey": "<kombination>" }` oder `null` bei Abbruch/Timeout.
/// Escape bricht ab (specs 4.3). Läuft schon ein Capture, wird der Aufruf
/// ignoriert.
pub fn start_capture(app: AppHandle) {
    let mut cap = CAPTURE.lock().unwrap();
    if cap.is_some() {
        return;
    }
    CAPTURE_CANCEL.store(false, Ordering::SeqCst);
    *cap = Some(ActiveCapture {
        app,
        deadline: Instant::now() + CAPTURE_TIMEOUT,
        eval: CaptureEval::new(),
    });
    drop(cap);
    info!("Key-Capture gestartet — warte auf Taste (Esc bricht ab, 15 s Timeout)");
}

/// Laufenden Capture abbrechen.
pub fn cancel_capture() {
    CAPTURE_CANCEL.store(true, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Repro für den Capture-Bug vom 25.10.: Der Key-Capture sieht von
    /// handy-keys stets seitenspezifische Modifier (CtrlLeft/OptLeft …). Der
    /// zurückgegebene String muss daraus Compound-Modifier machen — sonst
    /// matchte ein gespeichertes „ctrl_left+alt_left+m" nur auf die linke
    /// Seite und das Label zeigte den Rohstring.
    #[test]
    fn capture_normalisiert_seiten_modifikatoren() {
        let mut st = CaptureEval::new();
        // Modifer-Events (key=None) akkumulieren, dann Taste:
        assert_eq!(capture_eval(&mut st, &ev_down(Modifiers::CTRL_LEFT, None)), None);
        assert_eq!(
            capture_eval(&mut st, &ev_down(Modifiers::CTRL_LEFT | Modifiers::OPT_LEFT, None)),
            None
        );
        let result = capture_eval(&mut st, &ev_down(Modifiers::CTRL_LEFT | Modifiers::OPT_LEFT, Some(Key::M)));
        assert_eq!(result, Some(Some("ctrl+alt+m".into())));
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
        let mut st = CaptureEval::new();
        assert_eq!(capture_eval(&mut st, &ev_down(Modifiers::CTRL_LEFT, None)), None);
        assert_eq!(capture_eval(&mut st, &ev_down(Modifiers::CTRL_LEFT | Modifiers::OPT_RIGHT, None)), None);
        let result = capture_eval(&mut st, &ev_up(Modifiers::CTRL_LEFT, None));
        assert_eq!(result, Some(Some("ctrl+alt".into())));

        // Als Muster „ctrl+alt" muss das gemischte Seiten-Event genauso
        // matchen wie die Reinform rechts+rechts.
        let round: Hotkey = "ctrl+alt".parse().unwrap();
        assert!(round.modifiers.matches(Modifiers::CTRL_LEFT | Modifiers::OPT_RIGHT));
        assert!(round.modifiers.matches(Modifiers::CTRL_RIGHT | Modifiers::OPT_RIGHT));
    }

    /// Ein key_up ohne vorherige Modifier (z. B. Rest einer Taste, die vor dem
    /// Capture-Start gedrückt wurde) darf den Capture nicht beenden.
    #[test]
    fn capture_ignoriert_fremdes_key_up() {
        let mut st = CaptureEval::new();
        assert_eq!(capture_eval(&mut st, &ev_up(Modifiers::empty(), Some(Key::F12))), None);
        // Und danach geht es normal weiter:
        assert_eq!(
            capture_eval(&mut st, &ev_down(Modifiers::empty(), Some(Key::F9))),
            Some(Some("f9".into()))
        );
    }

    #[test]
    fn capture_escape_bricht_ab() {
        let mut st = CaptureEval::new();
        assert_eq!(capture_eval(&mut st, &ev_down(Modifiers::empty(), Some(Key::Escape))), Some(None));
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
