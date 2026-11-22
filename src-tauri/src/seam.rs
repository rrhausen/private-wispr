//! Naht-Kontext der Ziel-App (specs 4.6): die Zeichen links und rechts des
//! Cursors, als `fitting::SeamContext` an die Diktat-Verarbeitung gereicht
//! (Nahtstellen-Großschreibung, Abstände, Schluss-Punkt). Quelle ist UI
//! Automation — `TextPattern` der fokussierten App, Auswahl-Anfang/-Ende
//! je einen Charakter expandiert.
//!
//! Bewusst weichmaschig (gegrillt, 22.11.2026): Das Lesen ist ein
//! kosmetisches Extra, kein Kern-Fehlerkanal — jedes Scheitern (kein UIA,
//! kein fokussiertes Element, kein TextPattern — viele Eingabefelder und
//! alle elevated Ziele, mehrfache Auswahl, träge App) ergibt
//! `SeamContext::none()`, also exakt den Zustand vor Sprint 3.2, plus
//! Debug-Log mit Grund. Die Fitting-Regeln selbst sind rein und unit-
//! getestet (fitting.rs); hier kommt nur die Beschaffung dazu.
//!
//! UIA-Aufrufe können in träge antwortenden Prozessen hängen — deshalb
//! läuft das Lesen auf einem Hilfsthread mit Zeit-Cap (SEAM_CAP); die
//! Diktat-Verarbeitung wartet max. so lange, dann geht es ohne Naht
//! weiter. Der Hilfsthread initialisiert COM als MTA: STA ohne Message-
//! Pump wäre ein Deadlock-Kandidat für hereinkommende COM-Aufrufe.

use crate::fitting::SeamContext;
use log::debug;
use std::time::Duration;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, IUIAutomationTextRange,
    TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start, TextUnit_Character,
    UIA_TextPatternId,
};

/// Zeit-Cap fürs UIA-Lesen (ADR 0003): Die Transkription dauert 1–3 s —
/// 250 ms Verzögerung sind unkritisch, alles Darüber lohnt den Naht-Kontext
/// nicht.
const SEAM_CAP: Duration = Duration::from_millis(250);

/// Naht-Kontext der fokussierten App lesen (Cursor-Modus). Liefert im
/// Zweifel `SeamContext::none()` — siehe Modulkopf.
pub fn read() -> SeamContext {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("pw-seam".into())
        .spawn(move || {
            let ctx = unsafe { read_on_thread() };
            let _ = tx.send(ctx);
        })
        .ok();
    match rx.recv_timeout(SEAM_CAP) {
        Ok(ctx) => ctx,
        Err(_) => {
            debug!("Naht-Kontext: UIA nicht innerhalb {:?} gelesen", SEAM_CAP);
            SeamContext::none()
        }
    }
}

/// UIA-Lesen auf dem Hilfsthread. Fehlerweg ist None + Debug-Log (Grund).
unsafe fn read_on_thread() -> SeamContext {
    // COM initialisieren (S_FALSE = schon initialisiert, auch ok).
    if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
        debug!("Naht-Kontext: COM nicht initialisierbar");
        return SeamContext::none();
    }

    let ctx = (|| -> SeamContext {
        let automation: IUIAutomation =
            match CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
                Ok(a) => a,
                Err(e) => {
                    debug!("Naht-Kontext: UIA nicht erzeugbar ({e})");
                    return SeamContext::none();
                }
            };
        let focused = match automation.GetFocusedElement() {
            Ok(el) => el,
            Err(e) => {
                debug!("Naht-Kontext: kein fokussiertes Element ({e})");
                return SeamContext::none();
            }
        };
        // TextPattern: Editoren, Browser-Contenteditable, Word & Co. Viele
        // einfache Eingabefelder haben nur ValuePattern (ohne Cursor-Position)
        // — dort gibt es ehrlich keinen Naht-Kontext.
        let pattern: IUIAutomationTextPattern =
            match focused.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) {
                Ok(p) => p,
                Err(_) => {
                    debug!("Naht-Kontext: Ziel hat kein TextPattern");
                    return SeamContext::none();
                }
            };
        let selection = match pattern.GetSelection() {
            Ok(s) => s,
            Err(e) => {
                debug!("Naht-Kontext: Auswahl nicht lesbar ({e})");
                return SeamContext::none();
            }
        };
        // Mehrfachauswahl: mehrdeutig, wo die Naht ist — lieber kein Kontext.
        let count = selection.Length().unwrap_or(0);
        if count != 1 {
            debug!("Naht-Kontext: Auswahl-Anzahl {count}");
            return SeamContext::none();
        }
        let range = match selection.GetElement(0) {
            Ok(r) => r,
            Err(e) => {
                debug!("Naht-Kontext: Auswahl-Range nicht holbar ({e})");
                return SeamContext::none();
            }
        };
        let before = char_before(&range);
        let after = char_after(&range);
        debug!("Naht-Kontext: vor={before:?} nach={after:?}");
        seam_from(before.as_deref(), after.as_deref())
    })();

    CoUninitialize();
    ctx
}

/// Ein Zeichen links vom Auswahl-Anfang: dorthin kollabieren, einen
/// Charakter zurückgehen (0 = Dokumentanfang), auf dieses Zeichen
/// expandieren.
unsafe fn char_before(range: &IUIAutomationTextRange) -> Option<String> {
    let rb = range.Clone().ok()?;
    rb.MoveEndpointByRange(TextPatternRangeEndpoint_Start, range, TextPatternRangeEndpoint_Start)
        .ok()?;
    if rb.Move(TextUnit_Character, -1).ok()? == 0 {
        return None; // Cursor steht am Dokumentanfang
    }
    rb.ExpandToEnclosingUnit(TextUnit_Character).ok()?;
    Some(range_text(&rb))
}

/// Ein Zeichen rechts vom Auswahl-Ende: dorthin kollabieren; UIA expandiert
/// degenerate Ranges auf die FOLGENDE Einheit — genau das Zeichen nach dem
/// Cursor.
unsafe fn char_after(range: &IUIAutomationTextRange) -> Option<String> {
    let ra = range.Clone().ok()?;
    ra.MoveEndpointByRange(TextPatternRangeEndpoint_Start, range, TextPatternRangeEndpoint_End)
        .ok()?;
    ra.ExpandToEnclosingUnit(TextUnit_Character).ok()?;
    Some(range_text(&ra))
}

/// Text eines Ranges als String (UIA-Puffer freigeben nicht vergessen).
unsafe fn range_text(range: &IUIAutomationTextRange) -> String {
    match range.GetText(-1) {
        Ok(pw) => {
            let s = pw.to_string();
            CoTaskMemFree(Some(pw.as_ptr().cast()));
            s
        }
        Err(_) => String::new(),
    }
}

/// Reine Extraktion aus den gelesenen Range-Texten (unit-testbar): Das
/// Zeichen links ist das LETZTE des Vortextes, rechts das ERSTE des
/// Nachtextes — die Ranges sollten je ein Zeichen umfassen, aber Treu-
/// hand ist besser als Vertrauen. Leere Texte ⇒ None.
fn seam_from(before: Option<&str>, after: Option<&str>) -> SeamContext {
    SeamContext {
        char_before: before.and_then(|s| s.chars().last()),
        char_after: after.and_then(|s| s.chars().next()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zeichen davor/danach aus Ein-Zeichen-Texten.
    #[test]
    fn naht_aus_ein_zeichen() {
        let ctx = seam_from(Some("r"), Some("w"));
        assert_eq!(ctx.char_before, Some('r'));
        assert_eq!(ctx.char_after, Some('w'));
    }

    /// Mehrzeichen-Texte (Treuhand): links das letzte, rechts das erste
    /// Zeichen — die Naht liegt am Cursor, nicht am Range-Rand.
    #[test]
    fn naht_nimmt_die_zeichen_an_der_naht() {
        let ctx = seam_from(Some("der"), Some("wird"));
        assert_eq!(ctx.char_before, Some('r'));
        assert_eq!(ctx.char_after, Some('w'));
    }

    /// Fehlende/leere Seiten ⇒ None auf genau dieser Seite.
    #[test]
    fn fehlende_seiten_bleiben_none() {
        assert_eq!(seam_from(None, None), SeamContext::none());
        let ctx = seam_from(Some(""), Some(""));
        assert_eq!(ctx.char_before, None);
        assert_eq!(ctx.char_after, None);
        let only_before = seam_from(Some("."), None);
        assert_eq!(only_before.char_before, Some('.'));
        assert_eq!(only_before.char_after, None);
    }
}
