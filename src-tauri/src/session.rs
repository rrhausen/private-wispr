//! Diktat-Session (GLOSSARY.md): der Lebenszyklus eines Diktats von der
//! Hotkey-Kante bis zur abgeschlossenen Auslieferung und Historie — Start
//! (warmes Mikrofon übernehmen oder frisch öffnen), Ende (Aufnahme stoppen,
//! warm halten, Verarbeitungs-Thread anstoßen) und die Toggle-Semantik des
//! Hand-off-Hotkeys.
//!
//! Tiefe: lib.rs verdrahtet nur noch Hotkey → Session; alles, was zwischen
//! Kante und fertigem Diktat passiert (Warm-Start-Fallback, Fokus-Mitnahme,
//! Engine/VAD-Übergabe, Ausliefern, Historie, Pille), liegt hier. Die stille
//! Pipeline selbst ist und bleibt dictation::process — unangetastet, denn
//! sie trägt die WER-Kennzahl.

use log::{error, info, warn};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::deliver;
use crate::dictation;
use crate::dictionary;
use crate::feedback;
use crate::fitting;
use crate::focus;
use crate::history;
use crate::pill;
use crate::recording::{self, OutputMode};
use crate::seam;
use crate::settings;
use crate::vad;
use crate::{emit_status, AppState};

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
    let mut slot = state.recording.lock().unwrap();
    if slot.is_some() {
        info!("PTT down ({mode:?}) ignoriert: Es läuft bereits eine Aufnahme");
        return;
    }
    // Recorder besorgen (warm übernehmen, bei Fehlschlag einmal frisch
    // über die Fallback-Kette) — die ganze Politik steckt in acquire().
    let active = match recording::acquire(
        app,
        state.mic.lock().unwrap().clone(),
        &state.mic_warm,
        mode,
    ) {
        Ok(active) => active,
        Err(e) => {
            warn!("Aufnahme-Start fehlgeschlagen: {e:#}");
            pill::Pill::error(format!("Mikrofon: {e}")).finish(app);
            return;
        }
    };
    // Fokus-Check (specs 4.6): Ziel-Fenster jetzt merken — beim
    // Einfügen wird verglichen, ob es noch vorne steht.
    *state.focus_at_start.lock().unwrap() = Some(focus::capture());
    *slot = Some(active);
    drop(slot);
    feedback::play_click_if_enabled();
    pill::Pill::recording().show(app);
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
            pill::Pill::error(format!("Aufnahme: {e}")).finish(app);
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

    pill::Pill::transcribing().emit(app);
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
            pill::Pill::error("Spracherkennung nicht bereit — siehe Log".into()).finish(&handle);
            return;
        };
        // Diktat-Verarbeitung (GLOSSARY): die stille Pipeline inkl. Audio-
        // Politik (specs 4.8) steckt im tiefen Modul — derselbe Code, den
        // auch der Selftest misst. Der Naht-Kontext bringt die echten
        // Cursor-Zeichen der Ziel-App in die Naht-Regeln (fitting.rs).
        // VAD (ADR 0004): lazy beim ersten Diktat initialisiert, danach
        // gemerkt; Lock-Ordnung Engine → VAD wie überall.
        let (outcome, vad_changed) = {
            let app_state = handle.state::<AppState>();
            let mut vad_slot = app_state.vad.lock().unwrap();
            let vad_changed = vad_slot.ensure_loaded();
            let analyzer =
                vad_slot.analyzer.as_mut().map(|a| a as &mut dyn vad::SpeechAnalyzer);
            let outcome = dictation::process(
                &spooled,
                mode,
                &mut **engine,
                &dict,
                seam,
                analyzer,
                settings::current().archive_audio,
            );
            (outcome, vad_changed)
        };
        if vad_changed {
            emit_status(&handle);
        }
        // Ausliefern + Historie nur bei transkribiertem Diktat; die Pille
        // fürs Ende bildet pill_for() aus Ausgang + Bericht (reine Politik).
        let report = match &outcome {
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
                // bleibt nur die Verdrahtung Bericht → Event/Historie.
                let plan = deliver::plan(mode, focus_snap.as_ref(), focus::foreground_hwnd());
                let report = deliver::execute(&plan, final_text);
                let _ = handle.emit(
                    "pw://last",
                    LastPayload {
                        text: final_text.clone(),
                        raw_text: raw.clone(),
                        applied_rules: applied.clone(),
                        audio_secs: *audio_secs,
                        transcription_ms: *transcription_ms,
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
                    dictionary_version: dictionary_version.clone(),
                    applied: applied.clone(),
                    target_app: focus_snap.as_ref().and_then(|s| s.process.clone()),
                    duration_s: *audio_secs,
                    model: info.model_name.clone(),
                    engine: info.backend.clone(),
                };
                if let Err(e) = history::insert(&entry) {
                    error!("Historie fehlgeschlagen ({e:#}) — Diktat bleibt trotzdem ausgegeben");
                } else {
                    let _ = handle.emit("pw://history-changed", ());
                }
                Some(report)
            }
            _ => None,
        };
        pill::pill_for(&outcome, report.as_ref()).finish(&handle);
    });
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
