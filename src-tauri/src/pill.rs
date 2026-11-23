//! Overlay-Pille (GLOSSARY.md): die phasengesteuerte Anzeige eines Diktats
//! ohne Fokusraub — transparent, always-on-top, vom Backend positioniert.
//!
//! Tiefe durch ein kleines Interface: Die gesamte Phasen-Politik — welche
//! Phasen es gibt, welcher Text dabei steht und wie lange die Pille nach dem
//! Ende sichtbar bleibt — liegt hinter den `Pill`-Konstruktoren und der
//! reinen Funktion `pill_for()`. Dauern, `overlay_enabled`-Check,
//! Fensterposition und der Event-Name sind Implementation und nach außen
//! unsichtbar.
//!
//! Draht-Vertrag: Die Phase serialisiert als `"recording" | "transcribing" |
//! "done" | "error"` — Gegenseite ist der `Phase`-Typ in `src/overlay.tsx`;
//! der Test `wire_vertrag_phase_strings` pinned die exakten Strings. Der
//! Pegel läuft separat über `pw://level` (recording.rs — auch der Setup-
//! Assistent hört dort, deshalb ist er keine Pillen-Angelegenheit).

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::deliver::DeliveryReport;
use crate::dictation::DictationOutcome;
use crate::settings;

/// Event-Name der Pille — Draht-Vertrag mit `src/overlay.tsx`.
const EVENT: &str = "pw://overlay";

/// Sichtbarkeits-Politik der offenen Phasen: Pille bleibt, bis die nächste
/// Phase kommt (recording/transcribing).
const KEEP_OPEN: Option<Duration> = None;

/// Zu-kurz-Blip: kürzer als ein Ergebnis, damit er nicht als solches liest.
const HIDE_AFTER_TOO_SHORT: Duration = Duration::from_millis(700);

/// Ergebnis / Keine-Sprache: kurz zeigen, dann weg.
const HIDE_AFTER_DONE: Duration = Duration::from_millis(900);

/// Fehler: lange genug zum Lesen.
const HIDE_AFTER_ERROR: Duration = Duration::from_secs(3);

/// Phase der Pille auf dem Draht (`pw://overlay`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Recording,
    Transcribing,
    Done,
    Error,
}

/// Draht-Format. `level` ist bewusst weg: Der Pegel lief nie über dieses
/// Event (Rust sandte immer `None`), sondern über `pw://level`.
#[derive(Clone, Serialize)]
struct Payload {
    phase: Phase,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

/// Eine anzuzeigende Pille: Phase, Text und wie lange sie nach dem Ende
/// sichtbar bleibt. Nur über die Konstruktoren bauen — die Dauern sind
/// Phasen-Politik und liegen in diesem Modul.
#[derive(Clone, Debug)]
pub struct Pill {
    phase: Phase,
    message: Option<String>,
    /// `None` = offen bleiben.
    hide_after: Option<Duration>,
}

impl Pill {
    /// Aufnahme läuft — Pille zeigen (ohne Fokusraub).
    pub fn recording() -> Pill {
        Pill { phase: Phase::Recording, message: None, hide_after: KEEP_OPEN }
    }

    /// Transkription läuft — nur Text tauschen, Fenster bleibt wie es ist.
    pub fn transcribing() -> Pill {
        Pill {
            phase: Phase::Transcribing,
            message: Some("schreibt…".into()),
            hide_after: KEEP_OPEN,
        }
    }

    /// Ende mit Ergebnis — `msg` `None` ⇒ „fertig ✓".
    pub fn done(msg: Option<String>) -> Pill {
        Pill {
            phase: Phase::Done,
            message: Some(msg.unwrap_or_else(|| "fertig ✓".into())),
            hide_after: Some(HIDE_AFTER_DONE),
        }
    }

    /// Fehler — Meldung immer gesetzt (keine stumme Pille).
    pub fn error(msg: String) -> Pill {
        Pill {
            phase: Phase::Error,
            message: Some(msg),
            hide_after: Some(HIDE_AFTER_ERROR),
        }
    }

    /// Pille in den Vordergrund bringen (falls Overlay aktiviert ist) und
    /// die Phase anzeigen. Für den Aufnahme-Beginn; die Ziel-App behält den
    /// Fokus fürs Einfügen.
    pub fn show(&self, app: &AppHandle) {
        if !settings::current().overlay_enabled {
            return;
        }
        if let Some(w) = app.get_webview_window("overlay") {
            position_bottom_center(&w);
            let _ = w.show();
            // Bewusst kein set_focus: Fokusraub würde das Einfügen brechen.
        }
        self.emit(app);
    }

    /// Phase anzeigen, ohne das Fenster anzufassen (ist schon sichtbar).
    pub fn emit(&self, app: &AppHandle) {
        let _ = app.emit(EVENT, Payload { phase: self.phase, message: self.message.clone() });
    }

    /// Letzte Phase zeigen und die Pille nach `hide_after` verstecken.
    pub fn finish(&self, app: &AppHandle) {
        self.emit(app);
        if let Some(delay) = self.hide_after {
            let handle = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                if let Some(w) = handle.get_webview_window("overlay") {
                    let _ = w.hide();
                }
            });
        }
    }
}

/// Ergebnis-Pille für ein Diktat-Ende: bildet den Ausgang der Diktat-
/// Verarbeitung (plus Auslieferungs-Bericht, falls einer existierte) auf die
/// anzuzeigende Pille ab. Reine Funktion — die ganze Endphasen-Politik an
/// einem Ort und unit-testbar.
pub fn pill_for(outcome: &DictationOutcome, report: Option<&DeliveryReport>) -> Pill {
    match outcome {
        DictationOutcome::TooShort { .. } => Pill {
            phase: Phase::Done,
            message: Some("Zu kurz".into()),
            hide_after: Some(HIDE_AFTER_TOO_SHORT),
        },
        DictationOutcome::NoSpeech => Pill {
            phase: Phase::Done,
            message: Some("Keine Sprache erkannt".into()),
            hide_after: Some(HIDE_AFTER_DONE),
        },
        DictationOutcome::Failed(msg) => Pill::error(msg.clone()),
        DictationOutcome::Transcribed { .. } => {
            Pill::done(report.and_then(|r| r.done_msg.clone()))
        }
    }
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
    fn zu_kurz_ist_kurzer_blip() {
        let p = pill_for(&DictationOutcome::TooShort { duration_secs: 0.2 }, None);
        assert_eq!(p.phase, Phase::Done);
        assert_eq!(p.message.as_deref(), Some("Zu kurz"));
        assert_eq!(p.hide_after, Some(HIDE_AFTER_TOO_SHORT));
    }

    #[test]
    fn keine_sprache_liest_wie_fertig() {
        let p = pill_for(&DictationOutcome::NoSpeech, None);
        assert_eq!(p.phase, Phase::Done);
        assert_eq!(p.message.as_deref(), Some("Keine Sprache erkannt"));
        assert_eq!(p.hide_after, Some(HIDE_AFTER_DONE));
    }

    #[test]
    fn fehlgeschlagen_zeigt_fehlertext_lange() {
        let p = pill_for(&DictationOutcome::Failed("Boom".into()), None);
        assert_eq!(p.phase, Phase::Error);
        assert_eq!(p.message.as_deref(), Some("Boom"));
        assert_eq!(p.hide_after, Some(HIDE_AFTER_ERROR));
    }

    #[test]
    fn transkribiert_uebernimmt_auslieferungstext() {
        let report = DeliveryReport {
            target: "Cursor".into(),
            done_msg: Some("→ Zwischenablage — Einfügen nicht bestätigt".into()),
            history_mode: "clipboard",
        };
        let outcome = DictationOutcome::Transcribed {
            raw: "x".into(),
            final_text: "x".into(),
            applied: Vec::new(),
            dictionary_version: "v".into(),
            language: None,
            audio_secs: 1.0,
            transcription_ms: 1,
            pipeline_ms: 1,
        };
        let p = pill_for(&outcome, Some(&report));
        assert_eq!(p.phase, Phase::Done);
        assert_eq!(p.message.as_deref(), report.done_msg.as_deref());
        assert_eq!(p.hide_after, Some(HIDE_AFTER_DONE));
    }

    #[test]
    fn transkribiert_ohne_bericht_nimmt_default() {
        let outcome = DictationOutcome::Transcribed {
            raw: "x".into(),
            final_text: "x".into(),
            applied: Vec::new(),
            dictionary_version: "v".into(),
            language: None,
            audio_secs: 1.0,
            transcription_ms: 1,
            pipeline_ms: 1,
        };
        let p = pill_for(&outcome, None);
        assert_eq!(p.message.as_deref(), Some("fertig ✓"));
    }

    /// Draht-Vertrag mit `src/overlay.tsx` (dort `type Phase`): Verschiebt
    /// sich ein String, bricht die Anzeige still — dieser Test hält das fest.
    #[test]
    fn wire_vertrag_phase_strings() {
        assert_eq!(serde_json::to_string(&Phase::Recording).unwrap(), "\"recording\"");
        assert_eq!(serde_json::to_string(&Phase::Transcribing).unwrap(), "\"transcribing\"");
        assert_eq!(serde_json::to_string(&Phase::Done).unwrap(), "\"done\"");
        assert_eq!(serde_json::to_string(&Phase::Error).unwrap(), "\"error\"");
    }

    /// Offene Phasen bleiben offen: recording/transcribing verstecken nie.
    #[test]
    fn offene_phasen_verstecken_nicht() {
        assert_eq!(Pill::recording().hide_after, None);
        assert_eq!(Pill::transcribing().hide_after, None);
    }
}
