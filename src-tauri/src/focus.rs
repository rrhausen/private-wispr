//! Fokus-Check (specs 4.6): Bei Aufnahmebeginn wird das Vordergrund-Fenster
//! gemerkt (HWND + Prozessname). Ist beim Einfügen ein anderes Fenster
//! vorne, landet der Text in der Zwischenablage statt im falschen Feld —
//! mit klarer Meldung (Overlay-Zielangabe; E-Mail an den Chef tippt nicht
//! versehentlich in den Chat). Prozessname dient zugleich als `target_app`
//! für die Historie (specs 4.8).

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// Vordergrund-Fenster zum Zeitpunkt des Aufnahme-Starts.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub hwnd: isize,
    /// Prozess-EXE-Name (z. B. „Code.exe"), wenn ermittelbar.
    pub process: Option<String>,
}

impl Snapshot {
    /// Steht das gemerkte Fenster noch im Vordergrund?
    pub fn is_foreground(&self) -> bool {
        if self.hwnd == 0 {
            return false;
        }
        let fg = unsafe { GetForegroundWindow() };
        fg.0 as isize == self.hwnd
    }

    /// Kurze Ziel-App-Angabe für Meldung/Historie.
    pub fn target_label(&self) -> String {
        self.process
            .clone()
            .unwrap_or_else(|| format!("Fenster {}", self.hwnd))
    }
}

/// Aktuelles Vordergrund-Fenster einfangen (bei Aufnahmebeginn aufrufen).
pub fn capture() -> Snapshot {
    let hwnd = unsafe { GetForegroundWindow() };
    Snapshot { hwnd: hwnd.0 as isize, process: process_name(hwnd) }
}

fn process_name(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let queried = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(handle);
        if !queried || len == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_label_nennt_prozess_oder_fenster() {
        let with = Snapshot { hwnd: 42, process: Some("Code.exe".into()) };
        assert_eq!(with.target_label(), "Code.exe");
        let without = Snapshot { hwnd: 4242, process: None };
        assert_eq!(without.target_label(), "Fenster 4242");
    }
}
