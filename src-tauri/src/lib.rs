//! Private Wispr — M0 Walking Skeleton.
//!
//! Datenfluss: PTT-Hotkey (hold) → Aufnahme 16 kHz mono mit Disk-Spooling →
//! transcribe.cpp (Parakeet, Vulkan) → einfaches Paste (Clipboard+SendInput).
//! Overlay-Pille zeigt die Phase; das Hauptfenster Status und letztes Diktat.
//!
//! M0 bewusst offene Punkte (siehe specs, Abschnitt 8): Engine läuft noch
//! in-process (Worker-Prozess-Isolation kommt in M1), Paste noch ohne
//! Clipboard-Restore/Read-Receipt (paste_tx-Portierung M1), Wörterbuch erst M1.

mod deliver;
mod dictionary;
pub mod dictation;
pub mod engine_worker;
pub mod eval;
mod feedback;
mod fitting;
mod focus;
mod handoff;
mod history;
mod hotkey;
mod model_download;
mod paste;
mod paste_tx;
mod recording;
pub mod selftest;
mod seam;
mod settings;
pub mod stt;
mod upgrade;

pub use recording::OutputMode;
use stt::SttEngine;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use log::{error, info, warn};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

pub struct AppState {
    /// Engine hinter dem SttEngine-Trait: im App-Betrieb der isolierte
    /// Worker-Prozess (specs 4.1) — GPU-Faults killen nur den Worker.
    pub engine: Arc<Mutex<Option<Box<dyn stt::SttEngine + Send>>>>,
    pub engine_info: Arc<Mutex<stt::EngineInfo>>,
    pub recording: Arc<Mutex<Option<recording::ActiveRecording>>>,
    /// Gewähltes Eingabegerät (Name); `None` = System-Standard.
    pub mic: Arc<Mutex<Option<String>>>,
    /// Warm gehaltener Recorder: Das Gerät (v. a. Bluetooth-Headsets) bleibt
    /// zwischen Diktaten geöffnet, damit der Anfang nicht verschluckt wird.
    /// Nach WARM_IDLE ohne Aufnahme wird er geschlossen.
    pub mic_warm: recording::WarmSlot,
    /// Vordergrund-Fenster bei Aufnahmebeginn (Fokus-Check, specs 4.6, und
    /// target_app für die Historie).
    pub focus_at_start: Mutex<Option<focus::Snapshot>>,
}

/// Wie lange das Mikrofon nach dem letzten Diktat warm gehalten wird.
const WARM_IDLE: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Serialize)]
struct OverlayPayload {
    phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<f32>,
}

#[derive(Clone, Serialize)]
struct LastPayload {
    /// Endtext nach der Regel-Pipeline (das, was eingefügt wurde).
    text: String,
    /// Rohtranskript vor den Regeln (Nachvollziehbarkeit, specs 4.8).
    raw_text: String,
    /// Angewandte Wörterbuch-Regeln.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    applied_rules: Vec<dictionary::AppliedRule>,
    audio_secs: f64,
    transcription_ms: u128,
    /// Wohin der Text ging: „Cursor", Zwischenablage-Fälle oder der
    /// Hand-off-Dateipfad — ehrlich, siehe deliver::execute.
    target: String,
}

#[tauri::command]
fn status(state: tauri::State<AppState>) -> serde_json::Value {
    let info = state.engine_info.lock().unwrap();
    let (cursor_hotkey, handoff_hotkey) = hotkey::current();
    serde_json::json!({
        "hotkeys": {
            "cursor": cursor_hotkey,
            "handoff": handoff_hotkey,
        },
        "mic": state.mic.lock().unwrap().clone(),
        "handoff_dir": handoff::dir().display().to_string(),
        "engine": {
            "ready": info.loaded,
            "model": info.model_name,
            "backend": info.backend,
            "on_gpu": info.on_gpu,
            "device": info.device,
            "error": info.error,
            "model_path": stt::default_model_path().display().to_string(),
            "model_present": stt::default_model_path().exists(),
            "model_downloading": model_download::is_running(),
        },
    })
}

#[tauri::command]
fn list_devices() -> Result<Vec<String>, String> {
    // Monitor-Quellen sind Lautsprecher-Ausgaben, keine Mikrofone.
    let devices = handy_recorder::list_input_devices().map_err(|e| e.to_string())?;
    Ok(devices
        .into_iter()
        .filter(|d| !d.is_monitor)
        .map(|d| d.name)
        .collect())
}

#[tauri::command]
fn set_mic(app: AppHandle, state: tauri::State<AppState>, name: Option<String>) -> Result<(), String> {
    // Gerätewechsel: ein warm gehaltener Recorder für das alte Gerät ist
    // wertlos — schließen (Drop), damit z. B. das BT-Headset trennen kann.
    drop(state.mic_warm.lock().unwrap().take());
    *state.mic.lock().unwrap() = name.clone();
    settings::update(|s| s.mic_device = name).map_err(|e| e.to_string())?;
    info!("Eingabegerät gesetzt: {:?}", state.mic.lock().unwrap());
    emit_status(&app);
    Ok(())
}

#[tauri::command]
fn get_settings() -> settings::Settings {
    settings::current()
}

/// Hand-off-Ablageort setzen. Der Ordner wird sofort angelegt (klarer Fehler
/// statt späterem Fallback beim Diktat); `None` stellt den Default wieder her.
#[tauri::command]
fn set_handoff_dir(path: Option<String>) -> Result<String, String> {
    if let Some(p) = &path {
        std::fs::create_dir_all(p)
            .map_err(|e| format!("Verzeichnis nicht anlegbar: {p} ({e})"))?;
    }
    settings::update(|s| s.handoff_dir = path).map_err(|e| e.to_string())?;
    let effective = handoff::dir().display().to_string();
    info!("Hand-off-Ablageort: {effective}");
    Ok(effective)
}

#[tauri::command]
fn set_overlay_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update(|s| s.overlay_enabled = enabled).map_err(|e| e.to_string())?;
    if !enabled {
        if let Some(w) = app.get_webview_window("overlay") {
            let _ = w.hide();
        }
    }
    Ok(())
}

#[tauri::command]
fn set_feedback_enabled(enabled: bool) -> Result<(), String> {
    settings::update(|s| s.feedback_sound = enabled).map_err(|e| e.to_string())
}

/// Farbschema setzen ("dark" | "light" | "system").
#[tauri::command]
fn set_theme(theme: String) -> Result<(), String> {
    if !settings::THEMES.contains(&theme.as_str()) {
        return Err(format!("Unbekanntes Farbschema: {theme}"));
    }
    settings::update(|s| s.theme = theme).map_err(|e| e.to_string())
}

/// Paste-Chord setzen ("auto" | "ctrl_v" | "ctrl_shift_v" | "shift_insert",
/// specs 4.6). Wirkt ab dem nächsten Diktat.
#[tauri::command]
fn set_paste_chord(chord: String) -> Result<(), String> {
    if !settings::PASTE_CHORDS.contains(&chord.as_str()) {
        return Err(format!("Unbekanntes Einfüge-Verhalten: {chord}"));
    }
    settings::update(|s| s.paste_chord = chord).map_err(|e| e.to_string())
}

/// Diktat-Audio aufbewahren statt nach der Transkription zu löschen
/// (specs 4.8). Zum Einsammeln der Eval-Clips aus dem echten Betrieb.
#[tauri::command]
fn set_archive_audio(enabled: bool) -> Result<(), String> {
    settings::update(|s| s.archive_audio = enabled).map_err(|e| e.to_string())
}

/// Beide PTT-Hotkeys setzen: validieren + konfliktfrei umregistrieren, erst
/// danach persistieren (bei Fehler bleibt die alte Belegung aktiv).
#[tauri::command]
fn set_hotkeys(cursor: String, handoff: String) -> Result<(), String> {
    if let Err(msg) = hotkey::set_hotkeys(&cursor, &handoff) {
        // Keine stillen Ablehnungen: Grund + gewünschte Belegung ins Log,
        // damit Capture-Probleme nachvollziehbar sind.
        log::warn!("set_hotkeys abgelehnt: {msg} (cursor={cursor:?}, handoff={handoff:?})");
        return Err(msg);
    }
    settings::update(|s| {
        s.hotkeys.cursor = cursor;
        s.hotkeys.handoff = handoff;
    })
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Ist der Tastatur-Hook verbunden? Änderungen via Event `pw://hook-status`.
#[tauri::command]
fn hook_status() -> bool {
    hotkey::hook_ready()
}

/// Modell-Download starten; Fortschritt via `pw://model-download`.
#[tauri::command]
fn download_model(app: AppHandle) -> Result<(), String> {
    model_download::start(app).map_err(|e| e.to_string())
}

#[tauri::command]
fn cancel_model_download() {
    model_download::cancel();
}

/// Wörterbuch-Status für die Einstellungen (Profile ein-/ausschaltbar).
#[derive(Clone, Serialize)]
struct DictionaryStatus {
    dir: String,
    /// Profile, die an Termen vorkommen (unabhängig ob aktiv).
    profiles: Vec<String>,
    active_profiles: Vec<String>,
    locale: String,
    term_count: usize,
    alias_count: usize,
    /// Fachwörter, die aktuell in den Decode gehen.
    vocabulary_count: usize,
}

fn dictionary_status_inner(d: &dictionary::Dictionary) -> DictionaryStatus {
    DictionaryStatus {
        dir: dictionary::dir().display().to_string(),
        profiles: d.profiles.clone(),
        active_profiles: d.active_profiles.clone(),
        locale: d.locale.clone(),
        term_count: d.term_count,
        alias_count: d.alias_count,
        vocabulary_count: d.vocabulary().len(),
    }
}

#[tauri::command]
fn dictionary_status() -> DictionaryStatus {
    dictionary_status_inner(&dictionary::Dictionary::load())
}

/// Profile umschalten: schreibt die `profiles:`-Zeile der YAML zurück und
/// liefert den neuen Status (Terme bleiben unverändert).
#[tauri::command]
fn set_dictionary_profiles(active: Vec<String>) -> Result<DictionaryStatus, String> {
    dictionary::set_active_profiles(&active)
        .map(|d| dictionary_status_inner(&d))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn open_dictionary_dir() -> Result<(), String> {
    dictionary::open_dir().map_err(|e| e.to_string())
}

// ─── Historie (specs 4.8) ───────────────────────────────────────────────

#[tauri::command]
fn history_list(limit: u32) -> Vec<history::Entry> {
    history::list(limit.clamp(1, 100))
}

#[tauri::command]
fn history_delete(id: i64) -> Result<(), String> {
    history::delete(id).map_err(|e| e.to_string())
}

#[tauri::command]
fn history_clear() -> Result<(), String> {
    history::clear().map_err(|e| e.to_string())
}

/// Text in die Zwischenablage legen (Klick auf einen Historien-Eintrag).
#[tauri::command]
fn copy_text(text: String) -> Result<(), String> {
    paste::copy_to_clipboard(&text).map_err(|e| e.to_string())
}

// ─── Upgrade erkannter Installationen (specs 4.10: kein Auto-Update) ────

/// Upgrade-Plan, falls eine ältere Installation existiert und wir als
/// portable Kopie einer neueren Version laufen.
#[tauri::command]
fn upgrade_status() -> Option<upgrade::UpgradePlan> {
    upgrade::detect()
}

/// Das angebotene Upgrade ausführen (Dateien + Registry-Version).
#[tauri::command]
fn apply_upgrade(plan: upgrade::UpgradePlan) -> Result<upgrade::UpgradeReport, String> {
    upgrade::apply(&plan).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Webview),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("private-wispr".to_string()),
                    }),
                ])
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            // Panics app-weitem in die Log-Datei leiten (Threads sterben sonst
            // still — nur auf der Konsole sichtbar). Der Default-Hook
            // (Stderr) läuft anschließend weiter.
            let default_hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                let thread = std::thread::current();
                let name = thread.name().unwrap_or("<unbenannt>");
                log::error!("PANIK in Thread {name}: {info}");
                default_hook(info);
            }));

            let initial = settings::current();
            app.manage(AppState {
                engine: Arc::new(Mutex::new(None)),
                engine_info: Arc::new(Mutex::new(stt::EngineInfo::default())),
                recording: Arc::new(Mutex::new(None)),
                mic: Arc::new(Mutex::new(initial.mic_device)),
                mic_warm: Arc::new(Mutex::new(None)),
                focus_at_start: Mutex::new(None),
            });

            // Engine im Hintergrund laden (Modell-Load dauert Sekunden).
            let handle = app.handle().clone();
            std::thread::spawn(move || load_engine(&handle));

            hotkey::start(app.handle().clone())?;

            // Warmhalte-Reaper: Mikrofon nach WARM_IDLE ohne Diktat schließen
            // (sonst bliebe der Bluetooth-HFP-Link dauerhaft aktiv und
            // verschlechtert z. B. Musikwiedergabe auf dem Headset).
            {
                let handle = app.handle().clone();
                std::thread::Builder::new()
                    .name("pw-mic-warm-reaper".into())
                    .spawn(move || loop {
                        std::thread::sleep(Duration::from_secs(30));
                        let state = handle.state::<AppState>();
                        let expired = {
                            let mut warm = state.mic_warm.lock().unwrap();
                            match warm.as_ref() {
                                Some((_, last)) if last.elapsed() > WARM_IDLE => warm.take(),
                                _ => None,
                            }
                        };
                        if expired.is_some() {
                            log::info!("Mikrofon nach {WARM_IDLE:?} ohne Diktat geschlossen (Bluetooth trennt)");
                        }
                        // drop(expired) schließt das Gerät
                    })
                    .ok();
            }
            setup_tray(app)?;
            info!("Private Wispr M1 gestartet (Hotkeys frei konfigurierbar in den Einstellungen)");
            Ok(())
        })
        // X auf dem Hauptfenster versteckt es nur (App lebt im Tray weiter).
        // Ohne diesen Handler würde das Fenster zerstört; das verborgene
        // Overlay-Fenster hielte den Prozess am Leben und „Private Wispr
        // öffnen" fände kein Fenster mehr vor.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            status,
            list_devices,
            set_mic,
            get_settings,
            set_handoff_dir,
            set_overlay_enabled,
            set_feedback_enabled,
            set_theme,
            set_paste_chord,
            set_archive_audio,
            set_hotkeys,
            hook_status,
            download_model,
            cancel_model_download,
            dictionary_status,
            set_dictionary_profiles,
            open_dictionary_dir,
            history_list,
            history_delete,
            history_clear,
            copy_text,
            upgrade_status,
            apply_upgrade
        ])
        .run(tauri::generate_context!())
        .expect("Fehler beim Starten der Tauri-Anwendung");
}

/// Engine laden (beim Start und nach abgeschlossenem Modell-Download):
/// isolierter Worker-Prozess mit Modell-Load; Fehler landen sichtbar in
/// EngineInfo (kein stiller Fallback auf In-Process).
pub(crate) fn load_engine(app: &AppHandle) {
    match engine_worker::WorkerEngine::start_default() {
        Ok(engine) => {
            let mut info = engine.info().clone();
            info.loaded = true;
            info!(
                "Engine-Worker bereit: {} auf {} ({})",
                info.model_name, info.device_or_default(), info.backend
            );
            *app.state::<AppState>().engine.lock().unwrap() = Some(Box::new(engine));
            *app.state::<AppState>().engine_info.lock().unwrap() = info;
        }
        Err(e) => {
            let mut info = stt::EngineInfo::default();
            info.error = Some(e.to_string());
            *app.state::<AppState>().engine_info.lock().unwrap() = info;
            error!("Engine-Worker-Start fehlgeschlagen: {e:#}");
        }
    }
    emit_status(app);
}

fn emit_status(app: &AppHandle) {
    let state = app.state::<AppState>();
    let info = state.engine_info.lock().unwrap();
    let _ = app.emit(
        "pw://status",
        serde_json::json!({
            "ready": info.loaded,
            "model": info.model_name,
            "backend": info.backend,
            "on_gpu": info.on_gpu,
            "device": info.device,
            "error": info.error,
        }),
    );
}

/// Hauptfenster in den Vordergrund bringen (Tray-Menü, Tray-Doppelklick).
fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    let quit = MenuItem::with_id(app, "quit", "Beenden", true, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", "Private Wispr öffnen", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    tauri::tray::TrayIconBuilder::with_id("main-tray")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("Private Wispr")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => app.exit(0),
            "show" => show_main_window(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::DoubleClick { .. } = event {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Entscheidung des Hand-off-Toggles — rein (ohne AppHandle), damit unit-
/// testbar. `running`: Modus einer laufenden Aufnahme, falls eine läuft.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToggleAction {
    /// Keine Aufnahme läuft → Hand-off-Aufnahme starten.
    Start,
    /// Hand-off-Aufnahme läuft → beenden (transkribieren + ausgeben).
    Stop,
    /// Cursor-Aufnahme (PTT, gehalten) läuft → nicht anfassen.
    Ignore,
}

pub(crate) fn handoff_toggle_action(running: Option<OutputMode>) -> ToggleAction {
    match running {
        None => ToggleAction::Start,
        Some(OutputMode::Handoff) => ToggleAction::Stop,
        Some(OutputMode::Cursor) => ToggleAction::Ignore,
    }
}

/// Hand-off-Hotkey mit Toggle-Semantik (specs 4.3): Einmal antippen startet
/// die Aufnahme, nochmaliges Antippen beendet sie. Damit sind lange Diktate
/// (Brain-Dump ≥ 10 min, specs 4.2) möglich, ohne die Taste minutenlang
/// halten zu müssen. Die Cursor-Kombination behält Hold-Semantik — läuft
/// gerade eine Cursor-Aufnahme (Taste gehalten), wird das Antippen ignoriert.
pub(crate) fn on_handoff_toggle(app: &AppHandle) {
    let running = {
        let state = app.state::<AppState>();
        let running = state.recording.lock().unwrap().as_ref().map(|a| a.mode);
        running
    };
    match handoff_toggle_action(running) {
        ToggleAction::Start => on_ptt_down(app, OutputMode::Handoff),
        ToggleAction::Stop => {
            info!("Hand-off-Toggle: laufende Aufnahme wird beendet");
            on_ptt_up(app);
        }
        ToggleAction::Ignore => {
            info!("Hand-off-Toggle ignoriert: Cursor-Aufnahme (Halten) läuft");
        }
    }
}

/// Hotkey-Taste gedrückt: Aufnahme starten (wenn Engine bereit und nicht
/// bereits aufgenommen wird). Der Modus kommt vom auslösenden Hotkey.
pub(crate) fn on_ptt_down(app: &AppHandle, mode: OutputMode) {
    let state = app.state::<AppState>();
    if !state.engine_info.lock().unwrap().loaded {
        info!("PTT down ignoriert: Engine noch nicht bereit");
        return;
    }
    let mic = state.mic.lock().unwrap().clone();
    let mut slot = state.recording.lock().unwrap();
    if slot.is_some() {
        info!("PTT down ({mode:?}) ignoriert: Es läuft bereits eine Aufnahme");
        return;
    }
    // Warm gehaltenen Recorder übernehmen (Gerät blieb offen), sonst über
    // die Fallback-Kette neu öffnen. Schlägt der Warm-Start fehl, einmal
    // frisch öffnen.
    let warm = state.mic_warm.lock().unwrap().take();
    let warm_was_some = warm.is_some();
    let attempt = if let Some((recorder, _)) = warm {
        info!("PTT down ({mode:?}): warmes Mikrofon sofort startbereit");
        recording::start_with(app, recorder, false, mode)
    } else {
        info!("PTT down ({mode:?}): öffne Mikrofon ({:?})", mic.clone());
        match recording::open_for_mic(mic.clone(), Some(state.mic_warm.clone())) {
            Ok((recorder, used_fallback)) => {
                recording::start_with(app, recorder, used_fallback, mode)
            }
            Err(e) => Err(e),
        }
    };
    let attempt = match attempt {
        Ok(active) => Some(active),
        Err(e) if !warm_was_some => {
            warn!("Aufnahme-Start fehlgeschlagen: {e:#}");
            overlay_emit(
                app,
                OverlayPayload { phase: "error", message: Some(format!("Mikrofon: {e}")), level: None },
            );
            overlay_hide_later(app, Duration::from_secs(3));
            None
        }
        Err(e) => {
            // Warm-Start schlug fehl — einmal frisch öffnen.
            warn!("Warmes Mikrofon startet nicht ({e:#}) — öffne frisch");
            let (recorder, used_fallback) =
                match recording::open_for_mic(mic.clone(), Some(state.mic_warm.clone())) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("Aufnahme-Start fehlgeschlagen: {e:#}");
                        overlay_emit(
                            app,
                            OverlayPayload {
                                phase: "error",
                                message: Some(format!("Mikrofon: {e}")),
                                level: None,
                            },
                        );
                        overlay_hide_later(app, Duration::from_secs(3));
                        return;
                    }
                };
            recording::start_with(app, recorder, used_fallback, mode).ok()
        }
    };
    if let Some(active) = attempt {
        // Fokus-Check (specs 4.6): Ziel-Fenster jetzt merken — beim
        // Einfügen wird verglichen, ob es noch vorne steht.
        *state.focus_at_start.lock().unwrap() = Some(focus::capture());
        *slot = Some(active);
        drop(slot);
        feedback::play_click_if_enabled();
        overlay_show(app);
        overlay_emit(app, OverlayPayload { phase: "recording", message: None, level: None });
    }
}

/// Hotkey-Taste losgelassen: Aufnahme beenden, transkribieren, im Modus der
/// Aufnahme ausgeben (Cursor-Paste oder Hand-off-Datei).
pub(crate) fn on_ptt_up(app: &AppHandle) {
    info!("PTT up");
    let state = app.state::<AppState>();
    let active = state.recording.lock().unwrap().take();
    let Some(active) = active else { return };
    let mode = active.mode;
    // Fokus-Snapshot der Aufnahme mitnehmen (Cursor-Check + Historie-Ziel).
    let focus_snap = state.focus_at_start.lock().unwrap().take();

    let stopped = match recording::stop(active) {
        Ok(stopped) => stopped,
        Err(e) => {
            warn!("Aufnahme-Stopp fehlgeschlagen: {e:#}");
            overlay_emit(
                app,
                OverlayPayload { phase: "error", message: Some(format!("Aufnahme: {e}")), level: None },
            );
            overlay_hide_later(app, Duration::from_secs(3));
            return;
        }
    };
    // Sauberes Ende: Recorder warm halten (Gerät bleibt geöffnet, Bluetooth
    // trennt nicht). Unsauberes Ende (Gerät weg): verwerfen = schließen.
    if stopped.clean {
        *state.mic_warm.lock().unwrap() =
            Some((stopped.recorder, std::time::Instant::now()));
    }
    let spooled = stopped.spooled;

    overlay_emit(app, OverlayPayload { phase: "transcribing", message: None, level: None });
    let engine = state.engine.clone();
    let handle = app.clone();
    std::thread::spawn(move || {
        // Wörterbuch je Diktat frisch laden (Datei ist jederzeit editierbar);
        // Ladefehler werden in load() geloggt und ergeben ein leeres Wörterbuch.
        let dict = dictionary::Dictionary::load();
        // Naht-Kontext (specs 4.6): Zeichen links/rechts des Cursors in der
        // Ziel-App, gelesen während der Fokus noch dort steht — nur fürs
        // Cursor-Diktat; UIA-Lesen mit Zeit-Cap, Scheitern ⇒ none() (seam.rs).
        let seam = if mode == OutputMode::Cursor {
            seam::read()
        } else {
            fitting::SeamContext::none()
        };
        let mut guard = engine.lock().unwrap();
        // Engine weg (z. B. Worker-Crash): sichtbar melden statt still
        // zurückkehren — sonst bliebe die Pille in „transcribing" hängen.
        let Some(engine) = guard.as_mut() else {
            error!("Diktat verworfen: Engine nicht bereit");
            overlay_emit(
                &handle,
                OverlayPayload {
                    phase: "error",
                    message: Some("Spracherkennung nicht bereit — siehe Log".into()),
                    level: None,
                },
            );
            overlay_hide_later(&handle, Duration::from_secs(3));
            return;
        };
        // Diktat-Verarbeitung (GLOSSARY): die stille Pipeline inkl. Audio-
        // Politik (specs 4.8) steckt im tiefen Modul — derselbe Code, den
        // auch der Selftest misst. Der Naht-Kontext bringt die echten
        // Cursor-Zeichen der Ziel-App in die Naht-Regeln (fitting.rs).
        let outcome = dictation::process(
            &spooled,
            mode,
            &mut **engine,
            &dict,
            seam,
            settings::current().archive_audio,
        );
        match outcome {
            dictation::DictationOutcome::TooShort { .. } => {
                overlay_emit(&handle, OverlayPayload { phase: "done", message: Some("Zu kurz".into()), level: None });
                overlay_hide_later(&handle, Duration::from_millis(700));
            }
            dictation::DictationOutcome::Failed(msg) => {
                error!("Transkription fehlgeschlagen: {msg}");
                overlay_emit(
                    &handle,
                    OverlayPayload { phase: "error", message: Some(msg), level: None },
                );
                overlay_hide_later(&handle, Duration::from_secs(3));
            }
            dictation::DictationOutcome::Transcribed {
                raw,
                final_text,
                applied,
                dictionary_version,
                audio_secs,
                transcription_ms,
                ..
            } => {
                let info = engine.info().clone();
                // Ausliefern (GLOSSARY): reine Ziel-Entscheidung inklusive
                // Fokus-Semantik + dünne Ausführung im eigenen Modul — hier
                // bleibt nur die Verdrahtung Bericht → Event/Historie/Overlay.
                let plan = deliver::plan(mode, focus_snap.as_ref(), focus::foreground_hwnd());
                let report = deliver::execute(&plan, &final_text);
                let _ = handle.emit(
                    "pw://last",
                    LastPayload {
                        text: final_text.clone(),
                        raw_text: raw.clone(),
                        applied_rules: applied.clone(),
                        audio_secs,
                        transcription_ms,
                        target: report.target.clone(),
                    },
                );
                // Historie (specs 4.8): Rohtranskript, Endtext, angewandte
                // Regeln mit Version, Ziel-App, Modell/Engine. Ein Fehler
                // hier darf das Diktat nicht ungeschehen machen.
                let entry = history::NewEntry {
                    mode: report.history_mode.to_string(),
                    raw_transcript: raw.clone(),
                    final_text: final_text.clone(),
                    dictionary_version,
                    applied,
                    target_app: focus_snap.as_ref().and_then(|s| s.process.clone()),
                    duration_s: audio_secs,
                    model: info.model_name.clone(),
                    engine: info.backend.clone(),
                };
                if let Err(e) = history::insert(&entry) {
                    error!("Historie fehlgeschlagen ({e:#}) — Diktat bleibt trotzdem ausgegeben");
                } else {
                    let _ = handle.emit("pw://history-changed", ());
                }
                overlay_emit(&handle, OverlayPayload { phase: "done", message: report.done_msg, level: None });
                overlay_hide_later(&handle, Duration::from_millis(900));
            }
        }
    });
}

fn overlay_emit(app: &AppHandle, payload: OverlayPayload) {
    let _ = app.emit("pw://overlay", payload);
}

fn overlay_show(app: &AppHandle) {
    if !settings::current().overlay_enabled {
        return;
    }
    if let Some(w) = app.get_webview_window("overlay") {
        position_bottom_center(&w);
        let _ = w.show();
        // Bewusst kein set_focus: Die Ziel-App behält den Fokus fürs Einfügen.
    }
}

fn overlay_hide_later(app: &AppHandle, delay: Duration) {
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        if let Some(w) = handle.get_webview_window("overlay") {
            let _ = w.hide();
        }
    });
}

fn position_bottom_center(w: &tauri::WebviewWindow) {
    use tauri::PhysicalPosition;
    let Ok(Some(monitor)) = w.primary_monitor() else { return };
    let size = monitor.size();
    let scale = monitor.scale_factor();
    let (win_w, win_h) = ((380.0 * scale) as i32, (120.0 * scale) as i32);
    let _ = w.set_position(PhysicalPosition::new(
        monitor.position().x + (size.width as i32 - win_w) / 2,
        monitor.position().y + size.height as i32 - win_h - ((96.0 * scale) as i32),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_toggle_entscheidung() {
        // Ohne laufende Aufnahme startet das Antippen.
        assert_eq!(handoff_toggle_action(None), ToggleAction::Start);
        // Läuft eine Hand-off-Aufnahme, beendet das Antippen sie.
        assert_eq!(
            handoff_toggle_action(Some(OutputMode::Handoff)),
            ToggleAction::Stop
        );
        // Eine gehaltene Cursor-Aufnahme (PTT) wird nicht angetastet.
        assert_eq!(
            handoff_toggle_action(Some(OutputMode::Cursor)),
            ToggleAction::Ignore
        );
    }
}
