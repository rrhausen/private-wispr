//! Paste-Transaktion (specs 4.6, GLOSSARY): Einfügen als Transaktion statt
//! Feuer-und-vergessen. Konzept-Blaupause ist Handys `paste_tx` — nach
//! IP-Direktive als komplette Eigenimplementierung, kein Code übernommen.
//!
//! Ablauf einer Transaktion:
//!
//! 1. **Vollformat-Snapshot** der aktuellen Zwischenablage (alle Formate,
//!    Bytes kopiert — inkl. Bitmaps über die DIB-Familie; nur HGLOBAL-
//!    backed Formate sind sperrbar, GDI-Handle-Formate werden übersprun-
//!    gen, siehe `hglobal_backed`). Riesenformate (Kappen s. u.) verzichten
//!    auf die Transaktion: Dann läuft der plain Kopier-Pfad wie vor der
//!    Transaktion (deliver.rs), nur der Restore entfällt.
//! 2. **Delayed Render**: `EmptyClipboard` + `SetClipboardData(CF_UNICODE-
//!    TEXT, NULL)` — wir versprechen den Text, liefern ihn erst auf
//!    Anforderung. Dazu Opt-out-Marker gegen Win+V-Historie/Cloud-Sync.
//! 3. **Chord** (injizierbarer Trigger, meist `paste::send_paste_chord`).
//! 4. **Lesebestätigung**: das erste `WM_RENDERFORMAT` nach dem Ab-senden
//!    des Chords beweist, dass die Ziel-App `GetClipboardData` gerufen hat
//!    — das Einfügen findet wirklich statt. Renders davor zählen nicht:
//!    Clipboard-Manager reagieren auf `EmptyClipboard` und würden sonst
//!    eine falsche Bestätigung liefern (und der Restore würde dem echten
//!    Einfügen zuvorkommen). Bleibt die Bestätigung aus (Zeitfenster),
//!    ist nachweisbar nicht eingefügt — elevated/UIPI-Ziele laufen natür-
//!    lich in diesen Fall; der Text bleibt als Delayed-Owner in der
//!    Zwischenablage, manuelles Einfügen geht weiter.
//! 5. **Clipboard-Restore** aus dem Snapshot, bewacht durch
//!    `GetClipboardSequenceNumber`: Hat jemand anderes zwischen unserem
//!    Render und dem Restore geschrieben, wird nicht überschrieben.
//!
//! Fenster/Thread: Ein unsichtbares Fenster (bewusst KEIN Message-Only-
//! Fenster — nur ein echtes Fenster ist als Clipboard-Owner zuverlässig
//! adressierbar) auf einem eigenen langlebigen Thread mit Message-Loop;
//! es empfängt `WM_RENDERFORMAT`/`WM_RENDERALLFORMATS`/`WM_DESTROYCLIP-
//! BOARD`. Nach dem Restore bleiben wir Owner mit echten Daten (schulden
//! nichts mehr); übernimmt ein anderer, erlischt das Versprechen.
//!
//! Zeit-Werte sind bewusst Konstanten (keine Settings — nichts davon ist
//! Nutzer-Geschmack; Begründungen jeweils daneben, Gesamtdiskussion in
//! ADR 0003).

use anyhow::Result;
use log::{debug, info, warn};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
    TranslateMessage, MSG, WNDCLASSW, WM_DESTROYCLIPBOARD, WM_RENDERALLFORMATS,
    WM_RENDERFORMAT, WS_EX_NOACTIVATE, WS_OVERLAPPED,
};

// ─── Zeit- und Größen-Konstanten (ADR 0003) ──────────────────────────────

/// Clipboard-Öffnen am Transaktionsstart: 250 ms reichen für kurzfristig
/// gehaltene Clipboard-Manager; länger Blockiertes ⇒ plain Pfad.
const OPEN_RETRY_START: Duration = Duration::from_millis(250);
/// Fenster für die Lesebestätigung: träge Ziel-Apps (Word beim ersten
/// Einfügen) brauchen bis zu einer Sekunde — 2 s sind großzügig, ohne
/// die Pille spürbar aufzuhalten.
const RECEIPT_TIMEOUT: Duration = Duration::from_millis(2000);
/// Gnadenfrist nach der Bestätigung: Apps lesen mehrere Formate nachein-
/// ander und halten die Zwischenablage beim Einfügen kurz offen.
const RESTORE_GRACE: Duration = Duration::from_millis(120);
/// Synchrones Öffnen für den Restore: Das Diktat wartet darauf (Pille).
const RESTORE_OPEN_SYNC: Duration = Duration::from_millis(500);
/// Gesamtbudget des Hintergrund-Restore, wenn synchron nicht öffnungsfähig.
const RESTORE_BG_TOTAL: Duration = Duration::from_secs(5);
/// Kappen für den Snapshot: Einzelformat 32 MB / Summe 128 MB — darüber
/// verzögert das Kopieren jedes Diktat spürbar; dann plain Pfad (kein
/// Restore, Verhalten wie vor der Transaktion).
const MAX_FORMAT_BYTES: usize = 32 * 1024 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 128 * 1024 * 1024;
/// Abfrage-Rhythmus während des Wartens auf die Lesebestätigung.
const POLL: Duration = Duration::from_millis(5);

// ─── Ergebnis ────────────────────────────────────────────────────────────

/// Ehrliches Ergebnis der Transaktion (deliver.rs entscheidet über Meldung
/// und Fallbacks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteTxOutcome {
    /// Ziel-App hat gelesen (Lesebestätigung) — eingefügt. `restored`
    /// sagt, ob die Nutzer-Zwischenablage zurückkam (Fehlschlag nur Log).
    Pasted { restored: bool },
    /// Keine Lesebestätigung im Zeitfenster: nachweislich nicht eingefügt
    /// (elevated/UIPI oder blockiertes Ziel). Text bleibt als Delayed-
    /// Owner in der Zwischenablage — manuelles Einfügen geht weiter.
    NoReceipt,
    /// Transaktion konnte nicht starten (Clipboard blockiert oder Snapshot-
    /// Kappen) — Aufrufer nimmt den plain Kopier-Pfad.
    ClipboardGone,
}

// ─── Host: unsichtbares Owner-Fenster auf eigenem Thread ─────────────────

/// Transaktionszustand, geteilt zwischen WndProc (Tx-Thread) und run()
/// (Transkriptions-Thread). Kurz gesperrt, nie über Sleeps gehalten.
#[derive(Default)]
struct TxState {
    /// Text, den wir per Delayed Render schulden (None = nichts versprochen).
    pending_text: Option<String>,
    /// Zeitpunkte ALLER WM_RENDERFORMAT-Anfragen (auch Clipboard-Manager).
    renders: Vec<Instant>,
    /// Ab wann Renders als Lesebestätigung zählen (gesetzt vor dem Chord).
    receipt_not_before: Option<Instant>,
    /// Sequenznummer direkt nach unserem eigenen Render (Restore-Guard).
    seq_after_render: Option<u32>,
}

struct Shared {
    hwnd: AtomicIsize,
    state: Mutex<TxState>,
}

static HOST: OnceLock<Arc<Shared>> = OnceLock::new();
/// Zugriff für die WndProc (läuft auf dem Tx-Thread, vor CreateWindow gesetzt).
static WND_SHARED: OnceLock<Arc<Shared>> = OnceLock::new();
/// Serialisiert Transaktionen: ein Diktat im Sekundentakt darf die laufende
/// (auf Bestätigung wartende) Transaktion nicht überholen.
static TX_SERIAL: Mutex<()> = Mutex::new(());

/// Host einmalig hochfahren (Fenster + Message-Loop). Läuft die App weiter,
/// bleibt der Thread bis zum Prozessende am Pumpen — die Zwischenablage
/// kann jederzeit Render-Anfragen schicken, solange wir Owner sind.
fn host() -> &'static Arc<Shared> {
    HOST.get_or_init(|| {
        let shared = Arc::new(Shared {
            hwnd: AtomicIsize::new(0),
            state: Mutex::new(TxState::default()),
        });
        let _ = WND_SHARED.set(shared.clone());
        let (ready_tx, ready_rx) = mpsc::channel::<bool>();
        let for_thread = shared.clone();
        match std::thread::Builder::new()
            .name("pw-paste-tx".into())
            .spawn(move || tx_thread(&for_thread, ready_tx))
        {
            Ok(_) => {
                // Fenster-Feilschlingen (RegisterClass/CreateWindow) kurz
                // warten — Scheitern wird in run() als ClipboardGone sichtbar.
                let _ok = ready_rx.recv_timeout(Duration::from_secs(2)).unwrap_or(false);
            }
            Err(e) => warn!("Paste-Transaktion: Thread nicht startbar ({e})"),
        }
        shared
    })
}

fn tx_thread(shared: &Arc<Shared>, ready: mpsc::Sender<bool>) {
    unsafe {
        let Ok(hinstance) = GetModuleHandleW(PCWSTR::null()) else {
            warn!("Paste-Transaktion: HINSTANCE nicht ermittelbar");
            let _ = ready.send(false);
            return;
        };
        let class = w!("PrivateWisprPasteTx");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: HINSTANCE(hinstance.0),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            warn!("Paste-Transaktion: Fensterklasse nicht registrierbar");
            let _ = ready.send(false);
            return;
        }
        // Unsichtbar: nie ShowWindow; WS_EX_NOACTIVATE raubt keinen Fokus.
        let hwnd = CreateWindowExW(
            WS_EX_NOACTIVATE,
            class,
            w!("pw-paste-tx"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(HINSTANCE(hinstance.0)),
            None,
        );
        match hwnd {
            Ok(hwnd) => {
                shared.hwnd.store(hwnd.0 as isize, Ordering::Release);
                let _ = ready.send(true);
                let mut msg = MSG::default();
                loop {
                    let r = GetMessageW(&mut msg, None, 0, 0);
                    if r.0 == 0 || r.0 == -1 {
                        break;
                    }
                    let _ = TranslateMessage(&mut msg);
                    DispatchMessageW(&mut msg);
                }
            }
            Err(e) => {
                warn!("Paste-Transaktion: Fenster nicht erstellbar ({e})");
                let _ = ready.send(false);
            }
        }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let Some(shared) = WND_SHARED.get() else {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    };
    match msg {
        // Delayed Render: Ein Leser will CF_UNICODETEXT. Das System hat die
        // Zwischenablage schon im Namen des Lesers geöffnet — hier gehört
        // SetClipboardData OHNE OpenClipboard hin. IMMER rendern (auch für
        // Clipboard-Manager), die Einordnung als Lesebestätigung macht run().
        WM_RENDERFORMAT => {
            let fmt = wparam.0 as u32;
            if fmt == CF_UNICODETEXT.0 as u32 {
                let mut st = lock(&shared.state);
                if let Some(text) = st.pending_text.clone() {
                    match string_to_hglobal(&text) {
                        Ok(h) => {
                            if let Err(e) = SetClipboardData(fmt, Some(HANDLE(h.0))) {
                                warn!("Delayed Render fehlgeschlagen ({e})");
                            }
                        }
                        Err(e) => warn!("Delayed Render: Speicher ({e})"),
                    }
                }
                st.renders.push(Instant::now());
                st.seq_after_render = Some(GetClipboardSequenceNumber());
            }
            LRESULT(0)
        }
        // Herunterfahren: versprochene Formate echt rendern, damit ein
        // späteres Einfügen nicht ins Leere geht (best effort).
        WM_RENDERALLFORMATS => {
            let text = lock(&shared.state).pending_text.clone();
            if let Some(text) = text {
                if OpenClipboard(Some(hwnd)).is_ok() {
                    let _ = EmptyClipboard();
                    if let Ok(h) = string_to_hglobal(&text) {
                        let _ = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(h.0)));
                    }
                    let _ = CloseClipboard();
                }
            }
            LRESULT(0)
        }
        // Ein anderer hat die Zwischenablage übernommen — unser Versprechen
        // ist erloschen, weitere Renders liefern nichts mehr.
        WM_DESTROYCLIPBOARD => {
            lock(&shared.state).pending_text = None;
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn lock(state: &Mutex<TxState>) -> std::sync::MutexGuard<'_, TxState> {
    state.lock().unwrap_or_else(|p| p.into_inner())
}

/// Erfolgs-Prüfung für SetClipboardData mit Delayed Render: Win32 liefert
/// bei Erfolg das Daten-Handle zurück — bei `hmem = NULL` (unser Versprechen)
/// ist das dann eben NULL, und die windows-Crate macht daraus fälschlich ein
/// `Err` mit HRESULT 0 („Vorgang wurde erfolgreich beendet"). Code 0 heißt
/// hier also Erfolg.
fn set_data_ok(r: windows::core::Result<HANDLE>) -> bool {
    match r {
        Ok(_) => true,
        Err(e) => e.code().0 == 0,
    }
}

// ─── Transaktion ─────────────────────────────────────────────────────────

/// Eine Paste-Transaktion fahren. `trigger` schickt den Paste-Chord (inji-
/// zierbar: Tests spielen die Ziel-App, ohne SendInput). Der Aufrufer ist
/// deliver.rs — Fehlerbilder und Meldungen liegen dort.
pub fn run(text: &str, trigger: &mut dyn FnMut() -> Result<()>) -> PasteTxOutcome {
    let _serial = TX_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let shared = host();
    let raw = shared.hwnd.load(Ordering::Acquire);
    if raw == 0 {
        warn!("Paste-Transaktion: Fenster nicht verfügbar — plain Pfad");
        return PasteTxOutcome::ClipboardGone;
    }
    let hwnd = HWND(raw as _);

    // 1. Snapshot (None ⇒ Grund geloggt: blockiert oder Riesenformate).
    let Some(snapshot) = snapshot_clipboard() else {
        return PasteTxOutcome::ClipboardGone;
    };

    // 2. Owner werden: leeren, Delayed-Render versprechen, Opt-out-Marker.
    if !open_with_retry(Some(hwnd), OPEN_RETRY_START) {
        warn!("Zwischenablage für Transaktion nicht öffnungsfähig — plain Pfad");
        return PasteTxOutcome::ClipboardGone;
    }
    let started = unsafe {
        let ok = EmptyClipboard().is_ok()
            && set_data_ok(SetClipboardData(CF_UNICODETEXT.0 as u32, None));
        if ok {
            set_marker("ExcludeClipboardContentFromMonitorProcessing", 1);
            set_marker("CanUploadToCloudClipboard", 0);
        }
        let _ = CloseClipboard();
        ok
    };
    if !started {
        warn!("Delayed-Render nicht setzbar — plain Pfad");
        return PasteTxOutcome::ClipboardGone;
    }

    // Transaktionszustand: Renders zählen erst ab JETZT — Clipboard-Manager
    // haben auf Schritt 2 reagiert, bevor der Chord unterwegs ist.
    {
        let mut st = lock(&shared.state);
        st.pending_text = Some(text.to_string());
        st.renders.clear();
        st.seq_after_render = None;
        st.receipt_not_before = Some(Instant::now());
    }

    // 3. Chord losschicken (Modifier-Warte + SendInput stecken im Trigger).
    if let Err(e) = trigger() {
        warn!("Paste-Chord fehlgeschlagen ({e:#}) — Text bleibt in der Zwischenablage");
        return PasteTxOutcome::NoReceipt;
    }

    // 4. Auf die Lesebestätigung warten (erste Render-Anfrage ab Not-Before).
    let deadline = Instant::now() + RECEIPT_TIMEOUT;
    loop {
        {
            let st = lock(&shared.state);
            if let Some(not_before) = st.receipt_not_before {
                if st.renders.iter().any(|t| *t >= not_before) {
                    break;
                }
            }
        }
        if Instant::now() >= deadline {
            info!(
                "Keine Lesebestätigung innerhalb {RECEIPT_TIMEOUT:?} — Text bleibt in der Zwischenablage"
            );
            return PasteTxOutcome::NoReceipt;
        }
        std::thread::sleep(POLL);
    }

    // Gnadenfrist: Ziel-App liest ggf. mehrere Formate, hält die Zwischen-
    // ablage beim Einfügen kurz offen.
    std::thread::sleep(RESTORE_GRACE);

    // 5. Restore aus dem Snapshot (Guard + Retries dort).
    let restored = restore(shared, hwnd, snapshot);
    PasteTxOutcome::Pasted { restored }
}

// ─── Restore ─────────────────────────────────────────────────────────────

enum RestoreStep {
    Done,
    /// Nochmal versuchen (Clipboard kurz blockiert).
    Retry,
    /// Aufgeben (Sequenznummer verändert — jemand hat zwischenzeitlich
    /// kopiert; überschreiben wäre Datenverlust beim Nutzer).
    Stop,
}

/// HWND ist ein roher Zeiger und damit nicht Send — für den Hintergrund-
/// Restore-Thread als Zahl verpackt (der Wert ist nur eine Fenster-ID,
/// überall im Prozess gültig).
#[derive(Clone, Copy)]
struct SendHwnd(isize);
unsafe impl Send for SendHwnd {}

fn restore(shared: &Arc<Shared>, hwnd: HWND, snapshot: Snapshot) -> bool {
    let Some(expected) = lock(&shared.state).seq_after_render else {
        warn!("Restore ohne eigene Render-Sequenz — übersprungen");
        return false;
    };

    // Synchron probieren (das Diktat wartet, Pille hängt an diesem Thread).
    let sync_deadline = Instant::now() + RESTORE_OPEN_SYNC;
    loop {
        match try_restore(hwnd, &snapshot, expected) {
            RestoreStep::Done => {
                lock(&shared.state).pending_text = None;
                debug!("Clipboard-Restore erfolgreich");
                return true;
            }
            RestoreStep::Stop => return false,
            RestoreStep::Retry => {
                if Instant::now() >= sync_deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }

    // Hintergrund: Der Sync-Teil ist abgelaufen, das Diktat ist fertig
    // gemeldet — der Restore läuft leise weiter (bis RESTORE_BG_TOTAL).
    let shared = shared.clone();
    let hwnd_id = SendHwnd(hwnd.0 as isize);
    std::thread::Builder::new()
        .name("pw-paste-restore".into())
        .spawn(move || {
            let hwnd = HWND(hwnd_id.0 as _);
            let deadline = Instant::now() + RESTORE_BG_TOTAL;
            loop {
                match try_restore(hwnd, &snapshot, expected) {
                    RestoreStep::Done => {
                        lock(&shared.state).pending_text = None;
                        info!("Clipboard-Restore (Hintergrund) erfolgreich");
                        return;
                    }
                    RestoreStep::Stop => {
                        warn!("Zwischenablage zwischenzeitlich verändert — Restore aufgegeben");
                        return;
                    }
                    RestoreStep::Retry => {
                        if Instant::now() >= deadline {
                            warn!(
                                "Clipboard-Restore fehlgeschlagen — Zwischenablage enthält weiterhin das Diktat"
                            );
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        })
        .ok();
    false
}

fn try_restore(hwnd: HWND, snapshot: &Snapshot, expected_seq: u32) -> RestoreStep {
    unsafe {
        // Guard (specs 4.6): Nur überschreiben, wenn seit unserem Render
        // niemand anderes geschrieben hat.
        if GetClipboardSequenceNumber() != expected_seq {
            return RestoreStep::Stop;
        }
        if OpenClipboard(Some(hwnd)).is_err() {
            return RestoreStep::Retry;
        }
        // Öffnen kann Zeit kosten — noch einmal prüfen.
        if GetClipboardSequenceNumber() != expected_seq {
            let _ = CloseClipboard();
            return RestoreStep::Stop;
        }
        if EmptyClipboard().is_err() {
            let _ = CloseClipboard();
            return RestoreStep::Retry;
        }
        let mut failed = false;
        for (fmt, bytes) in formats_to_restore(&snapshot.formats) {
            match bytes_to_hglobal(bytes) {
                Ok(h) => {
                    if SetClipboardData(*fmt, Some(HANDLE(h.0))).is_err() {
                        failed = true;
                    }
                }
                Err(_) => failed = true,
            }
        }
        let _ = CloseClipboard();
        if failed {
            debug!("Einzelne Formate beim Restore nicht setzbar");
        }
        RestoreStep::Done
    }
}

/// Formate, die der Snapshot zurückspielen soll. Synthetisierte/abgeleitete
/// Formate (CF_TEXT aus CF_UNICODETEXT, CF_BITMAP aus CF_DIB, …) überspringt
/// das System nach dem Restore automatisch wieder; Owner-Display-Formate
/// brauchen Kooperation des Vorbesitzers und sind nicht wiederherstellbar.
fn formats_to_restore(formats: &[(u32, Vec<u8>)]) -> Vec<&(u32, Vec<u8>)> {
    let present: Vec<u32> = formats.iter().map(|(f, _)| *f).collect();
    formats.iter().filter(|(f, _)| keep_format(*f, &present)).collect()
}

fn keep_format(fmt: u32, present: &[u32]) -> bool {
    const CF_TEXT: u32 = 1;
    const CF_BITMAP: u32 = 2;
    const CF_METAFILEPICT: u32 = 3;
    const CF_OEMTEXT: u32 = 7;
    const CF_DIB: u32 = 8;
    const CF_PALETTE: u32 = 9;
    const CF_LOCALE: u32 = 16;
    const CF_DIBV5: u32 = 17;
    const CF_ENHMETAFILE: u32 = 14;
    const CF_OWNERDISPLAY: u32 = 0x0080;
    const CF_DSPTEXT: u32 = 0x0082;
    const CF_DSPBITMAP: u32 = 0x0083;
    const CF_DSPMETAFILEPICT: u32 = 0x0084;
    const CF_DSPENHMETAFILE: u32 = 0x008E;
    match fmt {
        // Synthetisiert (leitet sich nach dem Restore wieder ab) oder ohne
        // HGLOBAL-Speicher gar nicht im Snapshot (hglobal_backed).
        CF_TEXT | CF_OEMTEXT | CF_METAFILEPICT | CF_LOCALE | CF_PALETTE | CF_ENHMETAFILE
        | CF_OWNERDISPLAY | CF_DSPTEXT | CF_DSPBITMAP | CF_DSPMETAFILEPICT | CF_DSPENHMETAFILE => {
            false
        }
        CF_BITMAP => !(present.contains(&CF_DIB) || present.contains(&CF_DIBV5)),
        _ => true,
    }
}

// ─── Snapshot ────────────────────────────────────────────────────────────

struct Snapshot {
    formats: Vec<(u32, Vec<u8>)>,
}

/// Vollformat-Snapshot der aktuellen Zwischenablage. `None` = keine Trans-
/// aktion (Grund geloggt: nicht öffnungsfähig oder Riesenformate). Ein
/// einzelnes nicht lesbares Format wird übersprungen — meist ist es syn-
/// thetisiert und wird nach dem Restore automatisch wieder abgeleitet.
fn snapshot_clipboard() -> Option<Snapshot> {
    if !open_with_retry(None, OPEN_RETRY_START) {
        warn!("Zwischenablage für Snapshot nicht öffnungsfähig — plain Pfad");
        return None;
    }
    let snapshot = (|| -> Option<Snapshot> {
        let mut formats: Vec<(u32, Vec<u8>)> = Vec::new();
        let mut fmt = unsafe { EnumClipboardFormats(0) };
        while fmt != 0 {
            if let Some(bytes) = read_format(fmt) {
                formats.push((fmt, bytes));
            }
            fmt = unsafe { EnumClipboardFormats(fmt) };
        }
        let sizes: Vec<usize> = formats.iter().map(|(_, b)| b.len()).collect();
        match size_verdict(&sizes) {
            SizeVerdict::Fits => Some(Snapshot { formats }),
            SizeVerdict::FormatTooBig => {
                info!("Zwischenablage enthält Einzelformat > 32 MB — kein Restore, plain Pfad");
                None
            }
            SizeVerdict::TotalTooBig => {
                info!("Zwischenablage enthält > 128 MB — kein Restore, plain Pfad");
                None
            }
        }
    })();
    unsafe {
        let _ = CloseClipboard();
    }
    snapshot
}

#[derive(Debug, PartialEq, Eq)]
enum SizeVerdict {
    Fits,
    FormatTooBig,
    TotalTooBig,
}

fn size_verdict(sizes: &[usize]) -> SizeVerdict {
    if sizes.iter().any(|&s| s > MAX_FORMAT_BYTES) {
        SizeVerdict::FormatTooBig
    } else if sizes.iter().sum::<usize>() > MAX_SNAPSHOT_BYTES {
        SizeVerdict::TotalTooBig
    } else {
        SizeVerdict::Fits
    }
}

/// Formate, deren Clipboard-Handle KEIN HGLOBAL ist (GDI-Handles bzw.
/// Owner-Display-Familie): `GlobalSize`/`GlobalLock` darauf ist undefi-
/// niert und hat Heap-Korruption verursacht (22.11., Diktat nach Bild-
/// Kopie — CF_BITMAP ist ein HBITMAP). Der Snapshot überspringt sie;
/// Bitmaps bleiben über CF_DIB/CF_DIBV5 (HGLOBAL) vertreten, das System
/// synthetisiert CF_BITMAP daraus zurück. Metafile/Palette gehen verloren
/// (selten, geloggt) — ehrlicher als instabiler Speicherzugriff.
fn hglobal_backed(fmt: u32) -> bool {
    const CF_BITMAP: u32 = 2;
    const CF_METAFILEPICT: u32 = 3;
    const CF_PALETTE: u32 = 9;
    const CF_ENHMETAFILE: u32 = 14;
    const CF_OWNERDISPLAY: u32 = 0x0080;
    const CF_DSPTEXT: u32 = 0x0082;
    const CF_DSPBITMAP: u32 = 0x0083;
    const CF_DSPMETAFILEPICT: u32 = 0x0084;
    const CF_DSPENHMETAFILE: u32 = 0x008E;
    !matches!(
        fmt,
        CF_BITMAP
            | CF_METAFILEPICT
            | CF_PALETTE
            | CF_ENHMETAFILE
            | CF_OWNERDISPLAY
            | CF_DSPTEXT
            | CF_DSPBITMAP
            | CF_DSPMETAFILEPICT
            | CF_DSPENHMETAFILE
    )
}

/// Ein Format lesen (Bytes kopieren; das Handle gehört der Zwischenablage).
fn read_format(fmt: u32) -> Option<Vec<u8>> {
    if !hglobal_backed(fmt) {
        debug!("Format {fmt} ohne HGLOBAL-Speicher — nicht snapshotbar");
        return None;
    }
    unsafe {
        let Ok(handle) = GetClipboardData(fmt) else {
            debug!("Format {fmt} nicht lesbar — wird im Restore fehlen");
            return None;
        };
        let hglobal = HGLOBAL(handle.0);
        let size = GlobalSize(hglobal);
        if size == 0 {
            return Some(Vec::new());
        }
        let ptr = GlobalLock(hglobal) as *const u8;
        if ptr.is_null() {
            debug!("Format {fmt} nicht sperrbar — wird im Restore fehlen");
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
        let _ = GlobalUnlock(hglobal);
        Some(bytes)
    }
}

// ─── Win32-Kleinkram ─────────────────────────────────────────────────────

/// Zwischenablage öffnen mit Retry — Clipboard-Manager halten sie gern
/// kurz fest; wer länger blockiert, ist kein Partner für die Transaktion.
fn open_with_retry(owner: Option<HWND>, total: Duration) -> bool {
    let deadline = Instant::now() + total;
    loop {
        if unsafe { OpenClipboard(owner) }.is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Opt-out-Marker setzen (specs 4.6):DWORD-Format mit RegisterClipboard-
/// FormatW. Best-effort — schlägt einer fehl, nur Log (das Diktat sitzt
/// nur Sekundenbruchteile in der Zwischenablage).
fn set_marker(name: &str, value: u32) {
    let mut wide: Vec<u16> = name.encode_utf16().collect();
    wide.push(0);
    let fmt = unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) };
    if fmt == 0 {
        warn!("Marker {name:?} nicht registrierbar");
        return;
    }
    match bytes_to_hglobal(&value.to_le_bytes()) {
        Ok(h) => {
            if unsafe { SetClipboardData(fmt, Some(HANDLE(h.0))) }.is_err() {
                warn!("Marker {name:?} nicht setzbar");
            }
        }
        Err(e) => warn!("Marker {name:?}: Speicher ({e})"),
    }
}

fn string_to_hglobal(text: &str) -> Result<HGLOBAL> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2) };
    bytes_to_hglobal(bytes)
}

fn bytes_to_hglobal(bytes: &[u8]) -> Result<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let ptr = GlobalLock(h) as *mut u8;
        anyhow::ensure!(!ptr.is_null(), "GlobalLock fehlgeschlagen");
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        let _ = GlobalUnlock(h);
        Ok(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Riesenformate kappen die Transaktion: Einzelformat über 32 MB.
    #[test]
    fn snapshot_kappt_einzelformat() {
        assert_eq!(size_verdict(&[100]), SizeVerdict::Fits);
        assert_eq!(size_verdict(&[MAX_FORMAT_BYTES + 1]), SizeVerdict::FormatTooBig);
    }

    /// Summe über 128 MB kappt ebenfalls (viele mittelgroße Formate, jeder
    /// Einzelne unter der Format-Kappe); exakt 128 MB gehört noch dazu.
    /// (Ein einzelnes 128-MB-Format greift schon die Einzelformat-Kappe.)
    #[test]
    fn snapshot_kappt_summe() {
        let four = [MAX_FORMAT_BYTES; 4];
        assert_eq!(four.iter().sum::<usize>(), MAX_SNAPSHOT_BYTES);
        assert_eq!(size_verdict(&four), SizeVerdict::Fits);
        let sizes = [MAX_FORMAT_BYTES, MAX_FORMAT_BYTES, MAX_FORMAT_BYTES, MAX_FORMAT_BYTES, 1];
        assert_eq!(size_verdict(&sizes), SizeVerdict::TotalTooBig);
    }

    /// Synthetisierte Formate überspringt der Restore — das System leitet
    /// sie nach dem Zurückschreiben automatisch wieder ab; Owner-Display-
    /// Formate sind gar nicht wiederherstellbar. CF_BITMAP nur dann, wenn
    /// kein DIB-Äquivalent da ist (sonst Dopplung ohne Synthese-Quelle).
    #[test]
    fn restore_überspringt_synthetisierte_formate() {
        let cf = |ids: &[u32]| {
            formats_to_restore(
                &ids.iter().map(|&f| (f, Vec::<u8>::new())).collect::<Vec<_>>(),
            )
            .into_iter()
            .map(|(f, _)| *f)
            .collect::<Vec<u32>>()
        };
        // Nur Unicode-Text: CF_TEXT/CF_LOCALE fallen weg.
        assert_eq!(cf(&[13, 1, 16]), vec![13]);
        // Bitmap mit DIB: nur DIB bleibt (Bitmap wird neu synthetisiert).
        assert_eq!(cf(&[2, 8]), vec![8]);
        // Bitmap ohne DIB: bleibt (sonst ginge sie verloren).
        assert_eq!(cf(&[2]), vec![2]);
        // Registriertes Format (>= 0xC000) bleibt immer.
        assert_eq!(cf(&[13, 0xC001]), vec![13, 0xC001]);
    }

    /// Pflicht-Test nach dem Heap-Crash (22.11.): Formate mit GDI-Handles
    /// (Bitmap, Metafile, Palette) sind KEIN HGLOBAL — GlobalSize/GlobalLock
    /// darauf war die Heap-Korruption. Der Snapshot nimmt sie nie an.
    #[test]
    fn snapshot_nimmt_nur_hglobal_formate() {
        assert!(hglobal_backed(13), "CF_UNICODETEXT ist HGLOBAL");
        assert!(hglobal_backed(8), "CF_DIB ist HGLOBAL");
        assert!(hglobal_backed(17), "CF_DIBV5 ist HGLOBAL");
        assert!(hglobal_backed(0xC001), "registrierte Formate sind HGLOBAL");
        for gdi in [2u32, 3, 9, 14, 0x0080, 0x0082, 0x0083, 0x0084, 0x008E] {
            assert!(!hglobal_backed(gdi), "Format {gdi} darf nicht gesperrt werden");
        }
    }

    /// ECHTE Clipboard-Integration: fasst die Zwischenablage des Rechners an
    /// (sichert und stellt sie wieder her, aber ein Kopieren genau in
    /// dieser Sekunde verlöre). Deshalb #[ignore] — nur gezielt laufen
    /// lassen: `cargo test -p private-wispr paste_tx -- --ignored`.
    ///
    /// Phase 1 — der Test spielt die Ziel-App: sein GetClipboardData triggert
    /// WM_RENDERFORMAT (Lesebestätigung) und muss den Delayed-Render-Text
    /// bekommen; danach muss der Restore den Ausgangszustand zurückbringen.
    ///
    /// Phase 2 — Regression des Heap-Crashes (22.11.): CF_BITMAP auf der
    /// Zwischenablage liefert ein GDI-Handle, kein HGLOBAL — der Snapshot
    /// darf es NICHT sperren (früher: STATUS_HEAP_CORRUPTION beim ersten
    /// Diktat nach Bild-Kopie). Die Bitmap geht im Restore verloren (dokumen-
    /// tiertes Verhalten), der Ablauf darf nicht abstürzen. Beide Phasen in
    /// EINEM Test, weil sie die eine echte Zwischenablage seriell brauchen.
    #[test]
    #[ignore = "fasst die echte Zwischenablage an — nur gezielt laufen lassen"]
    fn transaktion_liefert_bestaetigt_stellt_wieder_her_und_ubersteht_bitmap() {
        // ── Phase 1: Text-Snapshot, Delayed Render, Bestätigung, Restore ──
        // Ausgangslage: bekannter Nutzer-Inhalt in der Zwischenablage.
        crate::paste::copy_to_clipboard("pw-tx-vorher").unwrap();
        std::thread::sleep(Duration::from_millis(150));

        let mut reads = 0;
        let mut failure = None;
        let mut trigger = || -> Result<()> {
            match clipboard_text_now() {
                Some(text) if text == "pw-tx-text" => {
                    reads += 1;
                    Ok(())
                }
                None => {
                    failure = Some("Clipboard nicht lesbar".into());
                    Err(anyhow::anyhow!("Clipboard nicht lesbar"))
                }
                Some(text) => {
                    let msg = format!("Delayed Render lieferte {text:?}");
                    failure = Some(msg.clone());
                    Err(anyhow::anyhow!(msg))
                }
            }
        };
        let out = run("pw-tx-text", &mut trigger);
        assert_eq!(reads, 1, "Trigger muss genau einmal gelesen haben: {failure:?}");
        assert_eq!(out, PasteTxOutcome::Pasted { restored: true });

        // Restore: Nutzer-Zwischenablage wieder da (Hintergrund-Retry darf
        // etwas dauern).
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if clipboard_text_now().as_deref() == Some("pw-tx-vorher") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Restore kam nicht: {:?}",
                clipboard_text_now()
            );
            std::thread::sleep(Duration::from_millis(25));
        }

        // ── Phase 2: GDI-Handle-Format darf den Snapshot nicht sprengen ──
        unsafe {
            let hdc = windows::Win32::Graphics::Gdi::GetDC(None);
            let hbm = windows::Win32::Graphics::Gdi::CreateCompatibleBitmap(hdc, 4, 4);
            windows::Win32::Graphics::Gdi::ReleaseDC(None, hdc);
            assert!(!hbm.0.is_null(), "Testbitmap nicht erzeugbar");
            assert!(OpenClipboard(None).is_ok());
            let _ = EmptyClipboard();
            let set = SetClipboardData(2 /* CF_BITMAP */, Some(HANDLE(hbm.0 as _)));
            assert!(set.is_ok() || set.err().map(|e| e.code().0 == 0).unwrap_or(false));
            let _ = CloseClipboard();
        }
        let mut no_read = || -> Result<()> { Ok(()) }; // Receipt bleibt aus — ok
        let out = run("pw-tx-bild", &mut no_read);
        // Kein Lesen ⇒ NoReceipt (Text bleibt in der Zwischenablage) —
        // Hauptbehauptung: der Prozess lebt noch und hat geantwortet.
        assert_eq!(out, PasteTxOutcome::NoReceipt);
    }

    /// Test-Hilfe: CF_UNICODETEXT der echten Zwischenablage lesen.
    fn clipboard_text_now() -> Option<String> {
        unsafe {
            if OpenClipboard(None).is_err() {
                return None;
            }
            let out = (|| {
                let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
                let hglobal = HGLOBAL(handle.0);
                let len = GlobalSize(hglobal) / 2;
                if len == 0 {
                    return Some(String::new());
                }
                let ptr = GlobalLock(hglobal) as *const u16;
                if ptr.is_null() {
                    return None;
                }
                let mut s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
                let _ = GlobalUnlock(hglobal);
                while s.ends_with('\0') {
                    s.pop();
                }
                Some(s)
            })();
            let _ = CloseClipboard();
            out
        }
    }
}
