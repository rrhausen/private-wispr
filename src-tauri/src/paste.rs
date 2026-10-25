//! Einfügen (M0.2): Text in die Zwischenablage, dann Strg+V als echte
//! Virtual-Key-Events über Win32 `SendInput` (Eigenimplementierung).
//!
//! Vorgeschichte: die enigo-Variante (Unicode-Injection) füllte zwar die
//! Zwischenablage, aber der Tastendruck kam bei fokussierten Editoren nicht
//! als Strg+V an (reproduziert und bewiesen per eigenem Testtextfeld, 24.10.).
//! Paste-Transaktion mit Delayed-Render/Read-Receipt/Clipboard-Restore
//! (Blaupause Handy paste_tx, eigenständig implementiert) folgt in M1.

use anyhow::Result;
use log::debug;
use std::time::{Duration, Instant};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_V,
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
/// ins falsche Feld.
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(text.to_string())?;
    Ok(())
}

pub fn paste_text(text: &str) -> Result<()> {
    copy_to_clipboard(text)?;

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

    // Strg + V als klassische VK-Sequenz: down(V) erst nach down(Ctrl),
    // up in umgekehrter Reihenfolge.
    let sequence = [
        key_event(VK_CONTROL.0, false),
        key_event(VK_V.0, false),
        key_event(VK_V.0, true),
        key_event(VK_CONTROL.0, true),
    ];
    let sent = unsafe { SendInput(&sequence, std::mem::size_of::<INPUT>() as i32) };
    anyhow::ensure!(
        sent == sequence.len() as u32,
        "SendInput unvollständig: {sent}/{} Events",
        sequence.len()
    );
    Ok(())
}
