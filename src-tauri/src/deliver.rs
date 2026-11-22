//! Ausliefern (GLOSSARY.md): den fertigen Text eines Diktats an sein Ziel
//! bringen — am Cursor einfügen, in die Zwischenablage (nachgewiesener
//! Fensterwechsel, specs 4.6) oder in die Hand-off-Tagesdatei.
//!
//! Zwei Funktionen, getrennt nach Prüfbarkeit (tiefe Module, schmale
//! Schnittstelle): [`plan`] ist rein — Ziel-Modus und Fokus-Semantik werden
//! hier entschieden, ohne Fenster, Zwischenablage oder Platte, deshalb
//! unit-testbar. [`execute`] ist dünn: führt den Plan über Win32/Dateisystem
//! aus und liefert den ehrlichen Bericht (Ziel, Meldung, Historien-Modus)
//! für Event, Historie und Overlay — bewusst ungetestet (SendInput/Platte).
//!
//! Fokus-Semantik (vereinheitlicht, specs 4.6): Nicht prüfbar — kein
//! Fokus-Snapshot oder Snapshot ohne Fenster (`hwnd == 0`) — heißt vertrauen
//! und einfügen; geblockt wird NUR bei nachgewiesenem Fensterwechsel (das
//! bei Aufnahmebeginn gemerkte Fenster steht beim Einfügen nicht mehr vorne).
//! Früher löste `hwnd == 0` fälschlich „Fokus gewechselt" aus.
//!
//! Fehlermodi (Hausregel: keine stillen Fallbacks — Fehler sichtbar machen):
//! * Hand-off fehlgeschlagen ⇒ Text in die Zwischenablage + laute Meldung.
//!   NIEMALS Strg+V als Ersatz: Hand-off-Ziele sind elevated, UIPI blockt
//!   injizierte Tasten — der frühere Paste-Fallback ging ins Leere.
//! * Einfügen nicht bestätigt (Paste-Transaktion ohne Lesebestätigung,
//!   z. B. elevated/UIPI-Ziel) ⇒ Text bleibt in der Zwischenablage
//!   (manuelles Einfügen möglich); die Meldung sagt es.
//! * Transaktion nicht startbar (Zwischenablage blockiert/Riesenformate)
//!   ⇒ plain Kopieren + Chord ohne Restore (Verhalten wie vor der Trans-
//!   aktion), im Log begründet.

use log::{error, info, warn};

use crate::focus;
use crate::handoff;
use crate::paste;
use crate::paste_tx;
use crate::recording::OutputMode;
use crate::settings;

/// Rein berechnete Entscheidung, wohin ein Diktat-Text geht.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryPlan {
    /// Am Cursor der Ziel-App einfügen: Fokus unverändert oder nicht prüfbar.
    /// `target_app`: Ziel-App der Aufnahme — steuert die Terminal-Erkennung
    /// des Paste-Chords (paste::select_chord) und die Historie.
    InsertAtCursor { target_app: Option<String> },
    /// Nur in die Zwischenablage: Fensterwechsel seit Aufnahmebeginn
    /// nachgewiesen (specs 4.6) — Strg+V ginge ins falsche Feld.
    /// `target_app`: Ziel-App der Aufnahme (Log-/Historien-Kontext).
    ClipboardAfterFocusChange { target_app: Option<String> },
    /// In die Hand-off-Tagesdatei schreiben (elevated Ziele, specs 4.3).
    HandoffFile,
}

/// Ehrlicher Bericht über das Ausliefern: Ziel für `pw://last` und die
/// Historie, optionale Meldung für die done-Pille, Modus für die Historie
/// („cursor" | „clipboard" | „handoff"). Der Modus gehört zum Plan; das
/// Ziel sagt ehrlich, was tatsächlich geworden ist.
#[derive(Clone, Debug)]
pub struct DeliveryReport {
    pub target: String,
    pub done_msg: Option<String>,
    pub history_mode: &'static str,
}

/// Auslieferungs-Plan berechnen (rein). `focus`: Snapshot von
/// Aufnahmebeginn; `foreground`: Vordergrund-Fenster JETZT — erst nach der
/// Transkription abfragen (`focus::foreground_hwnd()`), der Nutzer kann
/// währenddessen das Fenster wechseln.
pub fn plan(mode: OutputMode, focus: Option<&focus::Snapshot>, foreground: isize) -> DeliveryPlan {
    match mode {
        OutputMode::Handoff => DeliveryPlan::HandoffFile,
        OutputMode::Cursor => {
            // Nicht prüfbar (kein Snapshot oder `hwnd == 0`) ⇒ vertrauen und
            // einfügen. Geblockt nur bei nachgewiesenem Wechsel: Snapshot mit
            // Fenster, und ein anderes (oder gar kein) Fenster steht vorne.
            let changed = focus
                .filter(|s| s.hwnd != 0)
                .map_or(false, |s| s.hwnd != foreground);
            if changed {
                DeliveryPlan::ClipboardAfterFocusChange {
                    target_app: focus.and_then(|s| s.process.clone()),
                }
            } else {
                DeliveryPlan::InsertAtCursor {
                    target_app: focus.and_then(|s| s.process.clone()),
                }
            }
        }
    }
}

/// Plan ausführen: Zwischenablage/Strg+V/Hand-off-Datei anfassen und den
/// Bericht darüber liefern. Dünne Verdrahtung, bewusst ungetestet.
pub fn execute(plan: &DeliveryPlan, text: &str) -> DeliveryReport {
    match plan {
        DeliveryPlan::InsertAtCursor { target_app } => {
            insert_at_cursor(target_app.as_deref(), text)
        }
        DeliveryPlan::ClipboardAfterFocusChange { target_app } => {
            clipboard_after_focus_change(target_app.as_deref(), text)
        }
        DeliveryPlan::HandoffFile => handoff_file(text),
    }
}

fn insert_at_cursor(target_app: Option<&str>, text: &str) -> DeliveryReport {
    let chord = paste::select_chord(&settings::current().paste_chord, target_app);
    // Transaktion (paste_tx, specs 4.6): Delayed-Render, Lesebestätigung,
    // Clipboard-Restore. Der Trigger schickt den Chord (injizierbar für
    // Tests); Modifier-Warte steckt darin.
    let mut trigger = || paste::send_paste_chord(chord);
    match paste_tx::run(text, &mut trigger) {
        paste_tx::PasteTxOutcome::Pasted { .. } => DeliveryReport {
            target: "Cursor".into(),
            done_msg: None,
            history_mode: "cursor",
        },
        paste_tx::PasteTxOutcome::NoReceipt => {
            // Nachweislich nicht eingefügt (elevated/UIPI oder blockiertes
            // Ziel): Der Text bleibt als Delayed-Owner in der Zwischenablage.
            warn!("Einfügen nicht bestätigt — Text liegt in der Zwischenablage");
            DeliveryReport {
                target: "Zwischenablage (Einfügen nicht bestätigt)".into(),
                done_msg: Some("→ Zwischenablage — Einfügen nicht bestätigt".into()),
                history_mode: "cursor",
            }
        }
        paste_tx::PasteTxOutcome::ClipboardGone => plain_insert(chord, text),
    }
}

/// Transaktion konnte nicht starten (Zwischenablage blockiert oder Snapshot-
/// Kappen bei Riesenformaten): plain Kopieren + Chord — das Einfügen funk-
/// tioniert, nur der Restore entfällt. Verhalten wie vor der Transaktion,
/// im Log begründet (kein stiller Fallback).
fn plain_insert(chord: paste::Chord, text: &str) -> DeliveryReport {
    if let Err(e) = paste::copy_to_clipboard(text) {
        return clipboard_unavailable("cursor", e);
    }
    match paste::send_paste_chord(chord) {
        Ok(()) => DeliveryReport {
            target: "Cursor".into(),
            done_msg: None,
            history_mode: "cursor",
        },
        Err(e) => {
            warn!("Einfügen fehlgeschlagen ({e:#}) — Text liegt in der Zwischenablage.");
            DeliveryReport {
                target: "Zwischenablage (Einfügen fehlgeschlagen)".into(),
                done_msg: Some("→ Zwischenablage — Einfügen fehlgeschlagen".into()),
                history_mode: "cursor",
            }
        }
    }
}

fn clipboard_after_focus_change(target_app: Option<&str>, text: &str) -> DeliveryReport {
    info!("Fokus gewechselt (Ziel war {target_app:?}) — Text in die Zwischenablage");
    match paste::copy_to_clipboard(text) {
        Ok(()) => DeliveryReport {
            target: "Zwischenablage (Fokus gewechselt)".into(),
            done_msg: Some("→ Zwischenablage — Fokus gewechselt".into()),
            history_mode: "clipboard",
        },
        Err(e) => clipboard_unavailable("clipboard", e),
    }
}

fn handoff_file(text: &str) -> DeliveryReport {
    match handoff::append(text) {
        Ok(path) => {
            info!("Hand-off geschrieben: {}", path.display());
            DeliveryReport {
                target: path.display().to_string(),
                done_msg: Some("→ hand-off ✓".into()),
                history_mode: "handoff",
            }
        }
        Err(e) => {
            // Früher folgte hier der falsche Fallback: Cursor-Paste. Hand-
            // off-Ziele nehmen Strg+V nicht an (elevated/UIPI) — der Text
            // wäre verloren gewesen. Jetzt: Zwischenablage, laut gemeldet.
            error!("Hand-off fehlgeschlagen ({e:#}) — Text in die Zwischenablage");
            match paste::copy_to_clipboard(text) {
                Ok(()) => DeliveryReport {
                    target: "Zwischenablage (Hand-off fehlgeschlagen)".into(),
                    done_msg: Some("Hand-off fehlgeschlagen — Text in der Zwischenablage".into()),
                    history_mode: "handoff",
                },
                Err(ce) => clipboard_unavailable("handoff", ce),
            }
        }
    }
}

/// Letzter Anker: Nicht mal die Zwischenablage nahm den Text. Er bleibt
/// über `pw://last` und die Historie erreichbar — die Meldung weist dorthin.
fn clipboard_unavailable(history_mode: &'static str, e: anyhow::Error) -> DeliveryReport {
    error!("Zwischenablage fehlgeschlagen ({e:#}) — Text nicht ablegbar");
    DeliveryReport {
        target: "Zwischenablage (fehlgeschlagen)".into(),
        done_msg: Some("Zwischenablage fehlgeschlagen — Text in der Historie".into()),
        history_mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(hwnd: isize) -> focus::Snapshot {
        focus::Snapshot { hwnd, process: Some("Code.exe".into()) }
    }

    /// Pflicht-Test (a): Cursor-Diktat fügt ein, solange das bei
    /// Aufnahmebeginn gemerkte Fenster noch vorne steht. Die Ziel-App
    /// reist mit (Terminal-Erkennung des Paste-Chords, Historie).
    #[test]
    fn cursor_fuegt_ein_wenn_fenster_vorne_steht() {
        assert_eq!(
            plan(OutputMode::Cursor, Some(&snap(42)), 42),
            DeliveryPlan::InsertAtCursor { target_app: Some("Code.exe".into()) }
        );
    }

    /// Pflicht-Test (b): Geblockt NUR bei nachgewiesenem Fensterwechsel —
    /// ein anderes (oder gar kein) Fenster steht vorne; die Ziel-App der
    /// Aufnahme wandert als Kontext in den Plan.
    #[test]
    fn cursor_bloeckt_nur_bei_nachgewiesenem_fensterwechsel() {
        assert_eq!(
            plan(OutputMode::Cursor, Some(&snap(42)), 77),
            DeliveryPlan::ClipboardAfterFocusChange { target_app: Some("Code.exe".into()) }
        );
        assert!(matches!(
            plan(OutputMode::Cursor, Some(&snap(42)), 0),
            DeliveryPlan::ClipboardAfterFocusChange { .. }
        ));
    }

    /// Pflicht-Test (c): Nicht prüfbar ⇒ vertrauen. Kein Snapshot (Check
    /// nicht gelaufen) ODER Snapshot ohne Fenster — `hwnd == 0` löste
    /// früher fälschlich „Fokus gewechselt" aus (lib.rs, Cursor-Arm).
    /// Ohne Snapshot gibt es auch keine Ziel-App (Chord läuft auf „auto").
    #[test]
    fn cursor_vertraut_wenn_nicht_pruefbar() {
        assert_eq!(
            plan(OutputMode::Cursor, None, 77),
            DeliveryPlan::InsertAtCursor { target_app: None }
        );
        assert_eq!(
            plan(OutputMode::Cursor, Some(&snap(0)), 77),
            DeliveryPlan::InsertAtCursor { target_app: Some("Code.exe".into()) }
        );
    }

    /// Pflicht-Test (d): Hand-off plant IMMER die Datei — egal, was der
    /// Fokus tut. Damit kann kein Plan auf ein Cursor-Ziel zeigen und der
    /// frühere Fehlermodus „Hand-off fehlt → Strg+V" ist undenkbar.
    #[test]
    fn handoff_plant_immer_die_datei() {
        for focus in [None, Some(&snap(0)), Some(&snap(42))] {
            for foreground in [0isize, 42, 77] {
                assert_eq!(
                    plan(OutputMode::Handoff, focus, foreground),
                    DeliveryPlan::HandoffFile
                );
            }
        }
    }
}
