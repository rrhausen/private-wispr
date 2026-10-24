// Einstellungen (M1 Sprint 1): Hotkeys (Key-Capture, Konflikt-Erkennung),
// Hand-off-Ablageort, Mikrofon, Overlay, Autostart.
import { useEffect, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  disable as disableAutostart,
  enable as enableAutostart,
  isEnabled,
} from "@tauri-apps/plugin-autostart";

interface Hotkeys {
  cursor: string;
  handoff: string;
}

interface Props {
  hotkeys: Hotkeys;
  onHotkeysChange: (hotkeys: Hotkeys) => void;
  mic: string | null;
  devices: string[];
  onMicChange: (name: string | null) => void;
  effectiveHandoffDir: string;
  onHandoffDirChange: (effective: string) => void;
  modelPresent: boolean;
  engineReady: boolean;
  engineModel: string;
}

type CaptureTarget = "cursor" | "handoff" | null;

interface DownloadEvent {
  state: "resuming" | "downloading" | "verifying" | "done" | "cancelled" | "error";
  received?: number;
  total?: number;
  message?: string;
  path?: string;
}

const MODEL_MB = 705;

function mb(n: number): string {
  return (n / (1024 * 1024)).toFixed(0);
}

const KEY_LABELS: Record<string, string> = {
  ctrl: "Strg",
  alt: "Alt",
  shift: "Shift",
  super: "Win",
  space: "Leertaste",
  escape: "Esc",
};

/** handy-keys-String („ctrl+alt+space") als deutsche Anzeige („Strg+Alt+Leertaste"). */
function hotkeyLabel(hotkey: string): string {
  return hotkey
    .split("+")
    .map((part) => KEY_LABELS[part.trim()] ?? part.trim())
    .join("+");
}

export default function SettingsPanel({
  hotkeys,
  onHotkeysChange,
  mic,
  devices,
  onMicChange,
  effectiveHandoffDir,
  onHandoffDirChange,
  modelPresent,
  engineReady,
  engineModel,
}: Props) {
  const [overlay, setOverlay] = useState(true);
  const [feedback, setFeedback] = useState(true);
  const [autostart, setAutostart] = useState(false);
  const [customHandoffDir, setCustomHandoffDir] = useState<string | null>(null);
  const [handoffError, setHandoffError] = useState<string | null>(null);
  const [capturing, setCapturing] = useState<CaptureTarget>(null);
  const [hotkeyError, setHotkeyError] = useState<string | null>(null);
  const [download, setDownload] = useState<DownloadEvent | null>(null);

  useEffect(() => {
    invoke<{ overlay_enabled: boolean; feedback_sound: boolean; handoff_dir: string | null }>(
      "get_settings",
    )
      .then((s) => {
        setOverlay(s.overlay_enabled);
        setFeedback(s.feedback_sound);
        setCustomHandoffDir(s.handoff_dir);
      })
      .catch(console.error);
    isEnabled()
      .then(setAutostart)
      .catch(() => setAutostart(false));
  }, []);

  // Capture-Ergebnis einsammeln. Dependencies bewusst [capturing, hotkeys]:
  // der Handler braucht den Stand zum Zeitpunkt des Tastendrucks. Ein laufender
  // Capture wird beim Verlassen der Seite abgebrochen.
  useEffect(() => {
    const un = listen<{ hotkey: string | null }>("pw://hotkey-captured", (e) => {
      setCapturing(null);
      const captured = e.payload.hotkey;
      if (captured === null) return;
      void applyHotkeys(captured);
    });
    return () => {
      void un.then((f) => f());
      if (capturing) {
        invoke("cancel_hotkey_capture").catch(() => {});
      }
    };
  }, [capturing, hotkeys]);

  // Während des Captures Tasten im Fenster schlucken (Space würde sonst
  // Buttons klicken, Tab die Oberfläche bewegen).
  useEffect(() => {
    if (!capturing) return;
    const swallow = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", swallow, true);
    return () => window.removeEventListener("keydown", swallow, true);
  }, [capturing]);

  // Modell-Download-Verlauf; einmaliges Abonnieren genügt.
  useEffect(() => {
    const un = listen<DownloadEvent>("pw://model-download", (e) => setDownload(e.payload));
    return () => {
      void un.then((f) => f());
    };
  }, []);

  function startDownload() {
    setDownload({ state: "downloading", received: 0, total: MODEL_MB * 1024 * 1024 });
    invoke("download_model").catch(console.error);
  }

  async function applyHotkeys(captured: string) {
    if (!capturing) return;
    const next: Hotkeys = {
      cursor: capturing === "cursor" ? captured : hotkeys.cursor,
      handoff: capturing === "handoff" ? captured : hotkeys.handoff,
    };
    setHotkeyError(null);
    try {
      await invoke("set_hotkeys", { cursor: next.cursor, handoff: next.handoff });
      onHotkeysChange(next);
    } catch (err) {
      setHotkeyError(String(err));
    }
  }

  function startCapture(target: Exclude<CaptureTarget, null>) {
    setHotkeyError(null);
    setCapturing(target);
    invoke("capture_hotkey").catch(console.error);
  }

  async function applyHandoffDir(path: string | null) {
    setHandoffError(null);
    try {
      const effective = await invoke<string>("set_handoff_dir", { path });
      setCustomHandoffDir(path);
      onHandoffDirChange(effective);
    } catch (e) {
      setHandoffError(String(e));
    }
  }

  async function pickHandoffDir() {
    const picked = await open({
      directory: true,
      title: "Hand-off-Ablageort wählen",
      defaultPath: effectiveHandoffDir,
    });
    if (typeof picked === "string") {
      await applyHandoffDir(picked);
    }
  }

  function toggleOverlay(enabled: boolean) {
    setOverlay(enabled);
    invoke("set_overlay_enabled", { enabled }).catch(console.error);
  }

  function toggleFeedback(enabled: boolean) {
    setFeedback(enabled);
    invoke("set_feedback_enabled", { enabled }).catch(console.error);
  }

  async function toggleAutostart(enabled: boolean) {
    try {
      if (enabled) {
        await enableAutostart();
      } else {
        await disableAutostart();
      }
      setAutostart(await isEnabled());
    } catch (e) {
      console.error(e);
    }
  }

  function hotkeyRow(
    target: Exclude<CaptureTarget, null>,
    desc: string,
    hotkey: string,
  ) {
    return (
      <div className="hotkey-row" key={target}>
        <span className="desc">{desc}</span>
        {capturing === target ? (
          <>
            <kbd className="capturing">Taste drücken…</kbd>
            <button className="btn" onClick={() => invoke("cancel_hotkey_capture").catch(() => {})}>
              Abbrechen
            </button>
          </>
        ) : (
          <>
            <kbd>{hotkeyLabel(hotkey)}</kbd>
            <button
              className="btn"
              disabled={capturing !== null}
              onClick={() => startCapture(target)}
            >
              Ändern
            </button>
          </>
        )}
      </div>
    );
  }

  function modelCard() {
    const busy =
      download?.state === "downloading" ||
      download?.state === "resuming" ||
      download?.state === "verifying";

    let body: ReactNode;
    if (busy) {
      const received = download.received ?? 0;
      const total = download.total ?? MODEL_MB * 1024 * 1024;
      const pct = Math.min(100, (received / total) * 100);
      body = (
        <>
          <div className="progress">
            <span style={{ width: `${pct}%` }} />
          </div>
          <div className="row" style={{ justifyContent: "space-between" }}>
            <span className="meta">
              {download!.state === "verifying"
                ? "Prüfsumme wird verifiziert…"
                : `${mb(received)} / ${mb(total)} MB`}
            </span>
            {download!.state !== "verifying" && (
              <button
                className="btn"
                onClick={() => invoke("cancel_model_download").catch(() => {})}
              >
                Abbrechen
              </button>
            )}
          </div>
        </>
      );
    } else if (download?.state === "error") {
      body = (
        <>
          <p className="error">{download.message}</p>
          <button className="btn primary" onClick={startDownload}>
            Erneut versuchen
          </button>
        </>
      );
    } else if (download?.state === "cancelled") {
      body = (
        <div className="row" style={{ justifyContent: "space-between" }}>
          <span className="meta">Abgebrochen — Teilstand bleibt erhalten und wird fortgesetzt.</span>
          <button className="btn primary" onClick={startDownload}>
            Fortsetzen
          </button>
        </div>
      );
    } else if (modelPresent || download?.state === "done") {
      body = engineReady ? (
        <p className="meta">Modell vorhanden ✓ — Engine bereit ({engineModel}).</p>
      ) : (
        <p className="meta">Modell vorhanden ✓ — Engine wird geladen…</p>
      );
    } else {
      body = (
        <>
          <p className="meta">
            Sprachmodell (Parakeet TDT 0.6B v3, Q8) von HuggingFace laden — {MODEL_MB} MB,
            mit SHA256-Verifikation, abbrechbar und fortsetzbar.
          </p>
          <button className="btn primary" onClick={startDownload}>
            Modell herunterladen
          </button>
        </>
      );
    }

    return (
      <section className="card" key="model">
        <h2>Modell</h2>
        {body}
      </section>
    );
  }

  return (
    <div className="settings">
      <section className="card">
        <h2>Tastenkombinationen (halten zum Diktieren)</h2>
        {hotkeyRow("cursor", "Diktat am Cursor", hotkeys.cursor)}
        {hotkeyRow("handoff", "Hand-off-Diktat (Tagesdatei)", hotkeys.handoff)}
        <p className="meta">
          Gewünschte Kombination drücken — auch nur Modifier (z. B. nur Strg+Alt) sind
          möglich. Escape bricht ab. Nackte Tasten ohne Modifier sind nicht erlaubt,
          weil sie in anderen Apps blockt wären.
        </p>
        {hotkeyError && <p className="error">{hotkeyError}</p>}
      </section>

      <section className="card">
        <h2>Hand-off-Ablageort</h2>
        <p className="meta">
          Diktate des Hand-off-Modus landen als Tagesdatei <code>JJJJ-MM-TT.md</code> in
          diesem Ordner.
        </p>
        <div className="path-row">
          <code className="path">{effectiveHandoffDir}</code>
        </div>
        <div className="row" style={{ marginTop: 8, gap: 8 }}>
          <button className="btn" onClick={() => void pickHandoffDir()}>
            Durchsuchen…
          </button>
          {customHandoffDir !== null && (
            <button className="btn" onClick={() => void applyHandoffDir(null)}>
              Standard wiederherstellen
            </button>
          )}
        </div>
        {handoffError && <p className="error">{handoffError}</p>}
      </section>

      <section className="card">
        <h2>Mikrofon</h2>
        <select
          className="mic-select"
          value={mic ?? ""}
          onChange={(e) => onMicChange(e.target.value === "" ? null : e.target.value)}
        >
          <option value="">System-Standard</option>
          {devices.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
        <p className="meta">
          Ist dein Headset nicht dabei, Windows-Soundeinstellungen prüfen
          (Bluetooth-Headsets müssen verbunden und als Eingabegerät aktiv sein).
        </p>
      </section>

      {modelCard()}

      <section className="card">
        <h2>Anzeige, Sound &amp; Start</h2>
        <label className="check-row">
          <input
            type="checkbox"
            checked={overlay}
            onChange={(e) => toggleOverlay(e.target.checked)}
          />
          Overlay-Pille während des Diktats anzeigen
        </label>
        <label className="check-row">
          <input
            type="checkbox"
            checked={feedback}
            onChange={(e) => toggleFeedback(e.target.checked)}
          />
          Dezenten Klick beim Aufnahme-Start abspielen
        </label>
        <label className="check-row">
          <input
            type="checkbox"
            checked={autostart}
            onChange={(e) => void toggleAutostart(e.target.checked)}
          />
          Private Wispr nach Windows-Start automatisch starten
        </label>
      </section>
    </div>
  );
}

