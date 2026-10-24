// Einstellungen (M1 Sprint 1): Hand-off-Ablageort, Mikrofon, Overlay, Autostart.
// Hotkeys und Feedback-Klick kommen als eigene Karten dazu (siehe unten).
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import {
  disable as disableAutostart,
  enable as enableAutostart,
  isEnabled,
} from "@tauri-apps/plugin-autostart";

interface PwSettings {
  mic_device: string | null;
  handoff_dir: string | null;
  overlay_enabled: boolean;
}

interface Props {
  mic: string | null;
  devices: string[];
  onMicChange: (name: string | null) => void;
  effectiveHandoffDir: string;
  onHandoffDirChange: (effective: string) => void;
}

export default function SettingsPanel({
  mic,
  devices,
  onMicChange,
  effectiveHandoffDir,
  onHandoffDirChange,
}: Props) {
  const [overlay, setOverlay] = useState(true);
  const [autostart, setAutostart] = useState(false);
  const [customHandoffDir, setCustomHandoffDir] = useState<string | null>(null);
  const [handoffError, setHandoffError] = useState<string | null>(null);

  useEffect(() => {
    invoke<PwSettings>("get_settings")
      .then((s) => {
        setOverlay(s.overlay_enabled);
        setCustomHandoffDir(s.handoff_dir);
      })
      .catch(console.error);
    isEnabled()
      .then(setAutostart)
      .catch(() => setAutostart(false));
  }, []);

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

  return (
    <div className="settings">
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

      <section className="card">
        <h2>Anzeige &amp; Start</h2>
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
            checked={autostart}
            onChange={(e) => void toggleAutostart(e.target.checked)}
          />
          Private Wispr nach Windows-Start automatisch starten
        </label>
      </section>
    </div>
  );
}
