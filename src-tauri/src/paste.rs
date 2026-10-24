//! Einfaches Einfügen (M0): Text in die Zwischenablage, dann Strg+V via
//! SendInput (enigo). Terminal-Erkennung (Strg+Umschalt+V) folgt mit dem
//! paste_tx-Port aus Handy in M1, ebenso Delayed-Render + Clipboard-Restore.

use anyhow::Result;
use enigo::{Direction, Enigo, Keyboard, Key, Settings};

pub fn paste_text(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(text.to_string())?;

    // Kurz warten, damit die Zwischenablage sicher gefüllt ist.
    std::thread::sleep(std::time::Duration::from_millis(40));

    let mut enigo = Enigo::new(&Settings::default())?;
    enigo.key(Key::Control, Direction::Press)?;
    enigo.key(Key::Unicode('v'), Direction::Click)?;
    enigo.key(Key::Control, Direction::Release)?;
    Ok(())
}
