//! Einfügen (M0.2): Text in die Zwischenablage, dann die Paste-Tasten-
//! kombination als echte Virtual-Key-Events über Win32 `SendInput`
//! (Eigenimplementierung).
//!
//! Vorgeschichte: die enigo-Variante (Unicode-Injection) füllte zwar die
//! Zwischenablage, aber der Tastendruck kam bei fokussierten Editoren nicht
//! als Strg+V an (reproduziert und bewiesen per eigenem Testtextfeld, 24.10.).
//!
//! Die Kombination ist wählbar (specs 4.6, „Paste-Chord"): Default „auto"
//! erkennt Terminals am Prozessnamen und schickt dort Umschalt+Einfügen
//! (funktioniert in Windows Terminal, conhost und den gängigen Emulatoren;
//! Strg+V ist dort teils verbatim belegt). Die Transaktion drumherum
//! (Delayed-Render, Lesebestätigung, Clipboard-Restore) lebt in paste_tx.rs.

use anyhow::Result;
use log::{debug, warn};
use std::time::{Duration, Instant};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_CONTROL, VK_INSERT, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_V,
};

fn key_event(vk: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Physisch noch gehaltene Modifier (Alt/Strg/Win/Shift) — die Tasten, die aus
/// einem injizierten Strg+V versehentlich Strg+Alt+V, Win+V o. ä. machen.
fn held_modifiers() -> Vec<&'static str> {
    let mut held = Vec::new();
    for (vk, name) in [
        (VK_MENU, "Alt"),
        (VK_CONTROL, "Strg"),
        (VK_LWIN, "Win"),
        (VK_RWIN, "Win"),
        (VK_SHIFT, "Shift"),
    ] {
        if unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000 != 0 {
            if held.last() != Some(&name) || name != "Win" {
                held.push(name);
            }
        }
    }
    held
}

/// Text nur in die Zwischenablage legen — Fokus-Wechsel-Fallback (specs
/// 4.6): Ist beim Einfügen ein anderes Fenster vorne, kommt kein Strg+V
/// ins falsche Feld. Auch der ehrliche Ablageort, wenn Hand-off oder
/// Einfügen fehlschlagen (Fehlermodi in deliver.rs).
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(text.to_string())?;
    Ok(())
}

/// Tastenkombination fürs Einfügen (specs 4.6, „Paste-Chord").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chord {
    CtrlV,
    CtrlShiftV,
    ShiftInsert,
}

/// Prozess-EXE-Namen von Terminal-Fenstern. Bewusst hart im Code (kein
/// UI-Editor — YAGNI) und bewusst eine Blacklist der Eindeutigen: Das
/// VS-Code-Terminal läuft unter Code.exe und ist vom Editor nicht zu
/// unterscheiden — dokumentiertes Limit, dort gilt Strg+V (VS Code
/// bindet Strg+V im integrierten Terminal selbst als Einfügen).
pub const TERMINAL_EXES: &[&str] = &[
    "windowsterminal.exe",
    "openconsole.exe",
    "conhost.exe",
    "alacritty.exe",
    "wezterm-gui.exe",
    "mintty.exe",
    "putty.exe",
    "conemu64.exe",
    "kitty.exe",
    "hyper.exe",
];

/// Chord wählen: explizites Setting gewinnt immer; „auto" (Default)
/// schickt im Terminal Umschalt+Einfügen, sonst Strg+V. Unbekannte
/// Setting-Werte (z. B. aus einer künftigen Version) fallen mit Warnung
/// auf „auto" zurück — das Verhalten bleibt definiert.
pub fn select_chord(setting: &str, process: Option<&str>) -> Chord {
    match setting {
        "ctrl_v" => Chord::CtrlV,
        "ctrl_shift_v" => Chord::CtrlShiftV,
        "shift_insert" => Chord::ShiftInsert,
        "auto" => auto_chord(process),
        other => {
            warn!("Unbekannter paste_chord-Wert {other:?} — nutze „auto“");
            auto_chord(process)
        }
    }
}

fn auto_chord(process: Option<&str>) -> Chord {
    let is_terminal = process
        .map(|p| TERMINAL_EXES.iter().any(|t| p.eq_ignore_ascii_case(t)))
        .unwrap_or(false);
    if is_terminal {
        Chord::ShiftInsert
    } else {
        Chord::CtrlV
    }
}

/// Paste-Chord injizieren: Die fokussierte App fügt aus der Zwischenablage
/// ein. Das Bereitstellen des Textes muss der Aufrufer vorher erledigt
/// haben (Transaktion in paste_tx.rs oder plain Kopieren) — die Trennung
/// macht den Fehlerfall benennbar: Schlägt nur dies hier, liegt der Text
/// wenigstens in der Zwischenablage (deliver.rs meldet genau das).
pub fn send_paste_chord(chord: Chord) -> Result<()> {
    // Auf das physische Loslassen der Modifier warten (Handy-#2051-Muster,
    // eigenständig implementiert): direkt nach dem PTT-Loslassen können Alt/
    // Strg noch unten sein, und aus dem injizierten Strg+V würde dann z. B.
    // Strg+Alt+V — VSCode & Co. fügen dann nichts ein.
    let deadline = Instant::now() + Duration::from_millis(400);
    loop {
        let held = held_modifiers();
        if held.is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            debug!("Paste trotz gehaltener Modifier ({held:?}) — Timeout");
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }

    // Der Ziel-App Zeit geben, den Clipboard-Write zuverlässig zu sehen.
    std::thread::sleep(Duration::from_millis(60));

    // Klassische VK-Sequenz: Buchstabe/Einfügen erst nach den Modifiern,
    // up in umgekehrter Reihenfolge.
    let sequence: Vec<INPUT> = match chord {
        Chord::CtrlV => vec![(VK_CONTROL.0, 0), (VK_V.0, 0), (VK_V.0, 1), (VK_CONTROL.0, 1)],
        Chord::CtrlShiftV => vec![
            (VK_CONTROL.0, 0),
            (VK_SHIFT.0, 0),
            (VK_V.0, 0),
            (VK_V.0, 1),
            (VK_SHIFT.0, 1),
            (VK_CONTROL.0, 1),
        ],
        Chord::ShiftInsert => {
            vec![(VK_SHIFT.0, 0), (VK_INSERT.0, 0), (VK_INSERT.0, 1), (VK_SHIFT.0, 1)]
        }
    }
    .into_iter()
    .map(|(vk, up)| key_event(vk, up != 0))
    .collect();
    let sent = unsafe { SendInput(&sequence, std::mem::size_of::<INPUT>() as i32) };
    anyhow::ensure!(
        sent == sequence.len() as u32,
        "SendInput unvollständig: {sent}/{} Events",
        sequence.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// „auto" ohne Terminal-Prozess ⇒ Strg+V — der Normalfall.
    #[test]
    fn auto_ohne_terminal_nimmt_strg_v() {
        assert_eq!(select_chord("auto", Some("Code.exe")), Chord::CtrlV);
        assert_eq!(select_chord("auto", None), Chord::CtrlV);
        assert_eq!(select_chord("auto", Some("FIREFOX.exe")), Chord::CtrlV);
    }

    /// „auto" mit Terminal-EXE ⇒ Umschalt+Einfügen — funktioniert in
    /// Windows Terminal, conhost und den Emulatoren, wo Strg+V teils
    /// verbatim belegt ist. Groß-/Kleinschreibung des Prozessnamens ist
    /// keine Information (Windows-Terminal meldet „WindowsTerminal.exe").
    #[test]
    fn auto_mit_terminal_nimmt_shift_insert() {
        assert_eq!(select_chord("auto", Some("WindowsTerminal.exe")), Chord::ShiftInsert);
        assert_eq!(select_chord("auto", Some("alacritty.exe")), Chord::ShiftInsert);
    }

    /// Explizites Setting gewinnt immer — auch im Terminal und gegen die
    /// Terminal-Erkennung.
    #[test]
    fn explizites_setting_gewinnt_ueber_erkennung() {
        assert_eq!(select_chord("shift_insert", Some("Code.exe")), Chord::ShiftInsert);
        assert_eq!(select_chord("ctrl_v", Some("WindowsTerminal.exe")), Chord::CtrlV);
        assert_eq!(select_chord("ctrl_shift_v", None), Chord::CtrlShiftV);
    }

    /// Unbekannter Wert (z. B. aus einer künftigen Version) ⇒ „auto"-Ver-
    /// halten mit Warnung statt Panik oder Stillstand.
    #[test]
    fn unbekannter_wert_faellt_auf_auto_zurueck() {
        assert_eq!(select_chord("ctrl_alt_entf", Some("Code.exe")), Chord::CtrlV);
        assert_eq!(
            select_chord("ctrl_alt_entf", Some("putty.exe")),
            Chord::ShiftInsert
        );
    }
}
