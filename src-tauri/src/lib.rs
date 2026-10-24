//! Private Wispr — M0 Walking Skeleton.
//!
//! Datenfluss: PTT-Hotkey (hold) → Aufnahme 16 kHz mono mit Disk-Spooling →
//! transcribe.cpp (Parakeet, Vulkan) → einfaches Paste (Clipboard+SendInput).
//! Overlay-Pille zeigt die Phase; das Hauptfenster Status und letztes Diktat.
//!
//! M0 bewusst offene Punkte (siehe specs, Abschnitt 8): Engine läuft noch
//! in-process (Worker-Prozess-Isolation kommt in M1), Paste noch ohne
//! Clipboard-Restore/Read-Receipt (paste_tx-Portierung M1), Wörterbuch erst M1.

mod handoff;
mod hotkey;
mod paste;
mod recording;
pub mod selftest;
mod settings;
mod stt;

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
}

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
    text: String,
    audio_secs: f64,
    transcription_ms: u128,
    /// Wohin der Text ging: "Cursor" oder der hand-off-Dateipfad.
    target: String,
}

#[tauri::command]
fn status(state: tauri::State<AppState>) -> serde_json::Value {
    let info = state.engine_info.lock().unwrap();
    serde_json::json!({
        "hotkeys": {
            "cursor": hotkey::CURSOR_HOTKEY,
            "handoff": hotkey::HANDOFF_HOTKEY,
        },
        "mic": state.mic.lock().unwrap().clone(),
        "engine": {
            "ready": info.loaded,
            "model": info.model_name,
            "backend": info.backend,
            "on_gpu": info.on_gpu,
            "device": info.device,
            "error": info.error,
            "model_path": stt::default_model_path().display().to_string(),
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
    *state.mic.lock().unwrap() = name.clone();
    let mut settings = settings::load();
    settings.mic_device = name;
    settings::save(&settings).map_err(|e| e.to_string())?;
    info!("Eingabegerät gesetzt: {:?}", state.mic.lock().unwrap());
    emit_status(&app);
    Ok(())
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
        .setup(|app| {
            let initial = settings::load();
            app.manage(AppState {
                engine: Arc::new(Mutex::new(None)),
                engine_info: Arc::new(Mutex::new(stt::EngineInfo::default())),
                recording: Arc::new(Mutex::new(None)),
                last_dictation: Arc::new(Mutex::new(None)),
                mic: Arc::new(Mutex::new(initial.mic_device)),
            });

            // Engine im Hintergrund laden (Modell-Load dauert Sekunden).
            let handle = app.handle().clone();
            std::thread::spawn(move || load_engine(&handle));

            hotkey::start(app.handle().clone())?;
            setup_tray(app)?;
            info!(
                "Private Wispr M0 gestartet (Cursor: {} halten · Hand-off: {} halten)",
                hotkey::CURSOR_HOTKEY,
                hotkey::HANDOFF_HOTKEY
            );
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            status,
            list_devices,
            set_mic
        ])
        .run(tauri::generate_context!())
        .expect("Fehler beim Starten der Tauri-Anwendung");
}

fn load_engine(app: &AppHandle) {
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
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            _ => {}
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
        return;
    }
    info!("PTT down ({mode:?}): starte Aufnahme über {:?}", mic);
    match recording::start(app, mic, mode) {
        Ok(active) => {
            *slot = Some(active);
            drop(slot);
            overlay_show(app);
            overlay_emit(app, OverlayPayload { phase: "recording", message: None, level: None });
        }
        Err(e) => {
            warn!("Aufnahme-Start fehlgeschlagen: {e:#}");
            overlay_emit(
                app,
                OverlayPayload { phase: "error", message: Some(format!("Mikrofon: {e}")), level: None },
            );
            overlay_hide_later(app, Duration::from_secs(3));
        }
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

    let spooled = match recording::stop(active) {
        Ok(spooled) => spooled,
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
        let mut guard = engine.lock().unwrap();
        let Some(engine) = guard.as_mut() else { return };
        match engine.transcribe(&spooled.samples) {
            Ok(result) => {
                info!(
                    "Transkript ({} s Audio, {} ms): {}",
                    spooled.duration_secs, result.transcription_ms, result.text
                );
                *last.lock().unwrap() = Some(result.clone());
                // Ausgabe im Modus der Aufnahme: Cursor-Paste oder Hand-off-Datei.
                let (target, done_msg) = match mode {
                    OutputMode::Cursor => {
                        if let Err(e) = paste::paste_text(&result.text) {
                            warn!("Einfügen fehlgeschlagen ({e:#}) — Text liegt in der Zwischenablage.");
                        }
                        ("Cursor".to_string(), None)
                    }
                    OutputMode::Handoff => {
                        let (target, ok) = match handoff::append(&result.text) {
                            Ok(path) => {
                                info!("Hand-off geschrieben: {}", path.display());
                                (path.display().to_string(), true)
                            }
                            Err(e) => {
                                error!("Hand-off fehlgeschlagen ({e:#}) — Fallback: Cursor-Paste");
                                if let Err(e) = paste::paste_text(&result.text) {
                                    warn!("Einfügen fehlgeschlagen ({e:#}) — Text liegt in der Zwischenablage.");
                                }
                                ("Cursor (Hand-off fehlgeschlagen)".to_string(), false)
                            }
                        };
                        let done_msg = if ok { Some("→ hand-off ✓".to_string()) } else { None };
                        (target, done_msg)
                    }
                };
                let _ = handle.emit(
                    "pw://last",
                    LastPayload {
                        text: result.text.clone(),
                        audio_secs: spooled.duration_secs,
                        transcription_ms: result.transcription_ms,
                        target,
                    },
                );
                overlay_emit(&handle, OverlayPayload { phase: "done", message: done_msg, level: None });
                overlay_hide_later(&handle, Duration::from_millis(900));
            }
            Err(e) => {
                error!("Transkription fehlgeschlagen: {e:#}");
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
