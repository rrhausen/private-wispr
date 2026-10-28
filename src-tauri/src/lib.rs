//! Private Wispr — M0 Walking Skeleton.
//!
//! Datenfluss: PTT-Hotkey (hold) → Aufnahme 16 kHz mono mit Disk-Spooling →
//! transcribe.cpp (Parakeet, Vulkan) → einfaches Paste (Clipboard+SendInput).
//! Overlay-Pille zeigt die Phase; das Hauptfenster Status und letztes Diktat.
//!
//! M0 bewusst offene Punkte (siehe specs, Abschnitt 8): Engine läuft noch
//! in-process (Worker-Prozess-Isolation kommt in M1), Paste noch ohne
//! Clipboard-Restore/Read-Receipt (paste_tx-Portierung M1), Wörterbuch erst M1.

mod dictionary;
mod feedback;
mod fitting;
mod focus;
mod handoff;
mod history;
mod hotkey;
mod model_download;
mod paste;
mod recording;
pub mod selftest;
mod settings;
mod stt;
mod upgrade;

pub use recording::OutputMode;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use log::{error, info, warn};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

/// Aufnahmen unter dieser Schwelle gelten als versehentlich (Blitztext-Gate).
const MIN_RECORDING_SECS: f64 = 0.3;

pub struct AppState {
    pub engine: Arc<Mutex<Option<stt::Engine>>>,
    pub engine_info: Arc<Mutex<stt::EngineInfo>>,
    pub recording: Arc<Mutex<Option<recording::ActiveRecording>>>,
    pub last_dictation: Arc<Mutex<Option<stt::TranscriptResult>>>,
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
    /// Wohin der Text ging: "Cursor" oder der hand-off-Dateipfad.
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
    let mut settings = settings::load();
    settings.mic_device = name;
    settings::save(&settings).map_err(|e| e.to_string())?;
    info!("Eingabegerät gesetzt: {:?}", state.mic.lock().unwrap());
    emit_status(&app);
    Ok(())
}

#[tauri::command]
fn get_settings() -> settings::Settings {
    settings::load()
}

/// Hand-off-Ablageort setzen. Der Ordner wird sofort angelegt (klarer Fehler
/// statt späterem Fallback beim Diktat); `None` stellt den Default wieder her.
#[tauri::command]
fn set_handoff_dir(path: Option<String>) -> Result<String, String> {
    if let Some(p) = &path {
        std::fs::create_dir_all(p)
            .map_err(|e| format!("Verzeichnis nicht anlegbar: {p} ({e})"))?;
    }
    let mut settings = settings::load();
    settings.handoff_dir = path.clone();
    settings::save(&settings).map_err(|e| e.to_string())?;
    let effective = handoff::dir().display().to_string();
    info!("Hand-off-Ablageort: {effective}");
    Ok(effective)
}

#[tauri::command]
fn set_overlay_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::load();
    settings.overlay_enabled = enabled;
    settings::save(&settings).map_err(|e| e.to_string())?;
    if !enabled {
        if let Some(w) = app.get_webview_window("overlay") {
            let _ = w.hide();
        }
    }
    Ok(())
}

#[tauri::command]
fn set_feedback_enabled(enabled: bool) -> Result<(), String> {
    let mut settings = settings::load();
    settings.feedback_sound = enabled;
    settings::save(&settings).map_err(|e| e.to_string())
}

/// Farbschema setzen ("dark" | "light" | "system").
#[tauri::command]
fn set_theme(theme: String) -> Result<(), String> {
    if !settings::THEMES.contains(&theme.as_str()) {
        return Err(format!("Unbekanntes Farbschema: {theme}"));
    }
    let mut settings = settings::load();
    settings.theme = theme;
    settings::save(&settings).map_err(|e| e.to_string())
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
    let mut settings = settings::load();
    settings.hotkeys.cursor = cursor;
    settings.hotkeys.handoff = handoff;
    settings::save(&settings).map_err(|e| e.to_string())?;
    Ok(())
}

/// Key-Capture starten; das Ergebnis kommt asynchron via `pw://hotkey-captured`.
#[tauri::command]
fn capture_hotkey(app: AppHandle) {
    hotkey::start_capture(app);
}

#[tauri::command]
fn cancel_hotkey_capture() {
    hotkey::cancel_capture();
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

            let initial = settings::load();
            app.manage(AppState {
                engine: Arc::new(Mutex::new(None)),
                engine_info: Arc::new(Mutex::new(stt::EngineInfo::default())),
                recording: Arc::new(Mutex::new(None)),
                last_dictation: Arc::new(Mutex::new(None)),
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
            set_hotkeys,
            capture_hotkey,
            cancel_hotkey_capture,
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

/// Engine laden (beim Start und nach abgeschlossenem Modell-Download).
pub(crate) fn load_engine(app: &AppHandle) {
    match stt::Engine::load_default() {
        Ok(engine) => {
            let mut info = engine.info().clone();
            info.loaded = true;
            info!(
                "Engine bereit: {} auf {} ({})",
                info.model_name, info.device_or_default(), info.backend
            );
            *app.state::<AppState>().engine.lock().unwrap() = Some(engine);
            *app.state::<AppState>().engine_info.lock().unwrap() = info;
        }
        Err(e) => {
            let mut info = stt::EngineInfo::default();
            info.error = Some(e.to_string());
            *app.state::<AppState>().engine_info.lock().unwrap() = info;
            error!("Engine-Load fehlgeschlagen: {e:#}");
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

    if spooled.duration_secs < MIN_RECORDING_SECS {
        overlay_emit(app, OverlayPayload { phase: "done", message: Some("Zu kurz".into()), level: None });
        overlay_hide_later(app, Duration::from_millis(700));
        return;
    }

    overlay_emit(app, OverlayPayload { phase: "transcribing", message: None, level: None });
    let engine = state.engine.clone();
    let last = state.last_dictation.clone();
    let handle = app.clone();
    std::thread::spawn(move || {
        // Wörterbuch je Diktat frisch laden (Datei ist jederzeit editierbar);
        // Ladefehler werden in load() geloggt und ergeben ein leeres Wörterbuch.
        let dict = dictionary::Dictionary::load();
        let vocabulary = dict.vocabulary();
        let mut guard = engine.lock().unwrap();
        let Some(engine) = guard.as_mut() else { return };
        if !vocabulary.is_empty() {
            info!("Decode mit {} Fachwörtern aus dem Wörterbuch", vocabulary.len());
        }
        match engine.transcribe(&spooled.samples, vocabulary) {
            Ok(result) => {
                info!(
                    "Transkript ({} s Audio, {} ms): {}",
                    spooled.duration_secs, result.transcription_ms, result.text
                );
                let info = engine.info().clone();
                // Audio standardmäßig löschen (specs 4.8) — sofort nach der
                // Transkription. (Bei Transkriptions-Fehlern bleibt das WAV
                // für die Fehlersuche liegen, siehe Err-Zweig.)
                if let Err(e) = std::fs::remove_file(&spooled.wav_path) {
                    warn!("Spool-WAV nicht löschbar ({e}): {}", spooled.wav_path.display());
                }
                // Regel-Pipeline (specs 4.5/4.6): NFC → Filler → Phrasen-
                // Regeln → Text-Fitting. Naht-Kontext (Zeichen am Cursor
                // der Ziel-App) folgt mit der Paste-Transaktion, Sprint 3.
                let outcome = dict.apply(&result.text);
                if !outcome.applied.is_empty() {
                    info!("Wörterbuch angewandt: {:?}", outcome.applied);
                }
                let final_text = fitting::fit(&outcome.text, &fitting::SeamContext::none());
                *last.lock().unwrap() = Some(result.clone());
                // Ausgabe im Modus der Aufnahme: Cursor-Paste oder Hand-off-Datei.
                // history_mode protokolliert, was tatsächlich passiert ist
                // („clipboard" = Fokus-Wechsel-Fallback).
                let (target, done_msg, history_mode) = match mode {
                    OutputMode::Cursor => {
                        // Fokus-Check (specs 4.6): Ein anderes Fenster vorne
                        // heißt, Strg+V würde ins falsche Feld gehen — Text
                        // nur in die Zwischenablage, mit klarer Meldung.
                        let focus_ok =
                            focus_snap.as_ref().map_or(true, |s| s.is_foreground());
                        if focus_ok {
                            if let Err(e) = paste::paste_text(&final_text) {
                                warn!(
                                    "Einfügen fehlgeschlagen ({e:#}) — Text liegt in der Zwischenablage."
                                );
                            }
                            ("Cursor".to_string(), None, "cursor")
                        } else {
                            info!(
                                "Fokus gewechselt (Ziel war {:?}) — Text in die Zwischenablage",
                                focus_snap.as_ref().and_then(|s| s.process.clone())
                            );
                            if let Err(e) = paste::copy_to_clipboard(&final_text) {
                                error!("Zwischenablage fehlgeschlagen ({e:#})");
                            }
                            (
                                "Zwischenablage (Fokus gewechselt)".to_string(),
                                Some("→ Zwischenablage — Fokus gewechselt".to_string()),
                                "clipboard",
                            )
                        }
                    }
                    OutputMode::Handoff => {
                        let (target, ok) = match handoff::append(&final_text) {
                            Ok(path) => {
                                info!("Hand-off geschrieben: {}", path.display());
                                (path.display().to_string(), true)
                            }
                            Err(e) => {
                                error!("Hand-off fehlgeschlagen ({e:#}) — Fallback: Cursor-Paste");
                                if let Err(e) = paste::paste_text(&final_text) {
                                    warn!("Einfügen fehlgeschlagen ({e:#}) — Text liegt in der Zwischenablage.");
                                }
                                ("Cursor (Hand-off fehlgeschlagen)".to_string(), false)
                            }
                        };
                        let done_msg = if ok { Some("→ hand-off ✓".to_string()) } else { None };
                        (target, done_msg, "handoff")
                    }
                };
                let _ = handle.emit(
                    "pw://last",
                    LastPayload {
                        text: final_text.clone(),
                        raw_text: result.text.clone(),
                        applied_rules: outcome.applied.clone(),
                        audio_secs: spooled.duration_secs,
                        transcription_ms: result.transcription_ms,
                        target,
                    },
                );
                // Historie (specs 4.8): Rohtranskript, Endtext, angewandte
                // Regeln mit Version, Ziel-App, Modell/Engine. Ein Fehler
                // hier darf das Diktat nicht ungeschehen machen.
                let entry = history::NewEntry {
                    mode: history_mode.to_string(),
                    raw_transcript: result.text.clone(),
                    final_text: final_text.clone(),
                    dictionary_version: dict.version.clone(),
                    applied: outcome.applied.clone(),
                    target_app: focus_snap.as_ref().and_then(|s| s.process.clone()),
                    duration_s: spooled.duration_secs,
                    model: info.model_name.clone(),
                    engine: info.backend.clone(),
                };
                if let Err(e) = history::insert(&entry) {
                    error!("Historie fehlgeschlagen ({e:#}) — Diktat bleibt trotzdem ausgegeben");
                } else {
                    let _ = handle.emit("pw://history-changed", ());
                }
                overlay_emit(&handle, OverlayPayload { phase: "done", message: done_msg, level: None });
                overlay_hide_later(&handle, Duration::from_millis(900));
            }
            Err(e) => {
                error!("Transkription fehlgeschlagen: {e:#}");
                // WAV bleibt für die Fehlersuche liegen (Audio wird sonst
                // standardmäßig direkt nach der Transkription gelöscht).
                warn!("Audio aufbewahrt für Retry: {}", spooled.wav_path.display());
                overlay_emit(
                    &handle,
                    OverlayPayload { phase: "error", message: Some(e.to_string()), level: None },
                );
                overlay_hide_later(&handle, Duration::from_secs(3));
            }
        }
    });
}

fn overlay_emit(app: &AppHandle, payload: OverlayPayload) {
    let _ = app.emit("pw://overlay", payload);
}

fn overlay_show(app: &AppHandle) {
    if !settings::load().overlay_enabled {
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
