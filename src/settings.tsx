// Einstellungen (M1 Sprint 1): Hotkeys (Key-Capture, Konflikt-Erkennung),
// Hand-off-Ablageort, Mikrofon, Overlay, Sound, Autostart, Modell-Download.
// Aufbau: Sektionskopf über der Karte, Zeilen mit Titel/Untertitel links und
// Control rechts — eine Control-Breite pro Karte für ruhige Kanten.
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

interface DictionaryStatus {
  dir: string;
  profiles: string[];
  active_profiles: string[];
  locale: string;
  term_count: number;
  alias_count: number;
  vocabulary_count: number;
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

const KEY_LABELS: Record<string, string> = {
  ctrl: "Strg",
  alt: "Alt",
  shift: "Shift",
  super: "Win",
  space: "Leertaste",
  escape: "Esc",
};

/** handy-keys-String („ctrl+alt+space") als deutsche Anzeige („Strg+Alt+Leertaste"). */
export function hotkeyLabel(hotkey: string): string {
  return hotkey
    .split("+")
    .map((part) => {
      const key = part.trim();
      return KEY_LABELS[key] ?? (key.length === 1 ? key.toUpperCase() : key);
    })
    .join("+");
}

function mb(n: number): string {
  return (n / (1024 * 1024)).toFixed(0);
}

function Section({ head, children }: { head: string; children: ReactNode }) {
  return (
    <section className="section">
      <div className="section-head">{head}</div>
      <div className="card">{children}</div>
    </section>
  );
}

function SwitchRow({
  title,
  subtitle,
  checked,
  onChange,
}: {
  title: string;
  subtitle?: string;
  checked: boolean;
  onChange: (on: boolean) => void;
}) {
  return (
    <div className="row-line">
      <div className="row-text">
        <span className="row-title">{title}</span>
        {subtitle && <span className="row-sub">{subtitle}</span>}
      </div>
      <input
        type="checkbox"
        className="switch"
        role="switch"
        aria-checked={checked}
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
    </div>
  );
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
  const [hookReady, setHookReady] = useState(true);
  const [download, setDownload] = useState<DownloadEvent | null>(null);
  const [dict, setDict] = useState<DictionaryStatus | null>(null);
  const [dictError, setDictError] = useState<string | null>(null);

  useEffect(() => {
    invoke<DictionaryStatus>("dictionary_status").then(setDict).catch(console.error);
  }, []);

  // Profil umschalten: aktive Liste neu setzen, Status kommt zurück.
  async function toggleProfile(name: string) {
    if (!dict) return;
    const active = new Set(dict.active_profiles);
    if (active.has(name)) {
      active.delete(name);
    } else {
      active.add(name);
    }
    setDictError(null);
    try {
      setDict(
        await invoke<DictionaryStatus>("set_dictionary_profiles", {
          active: [...active],
        }),
      );
    } catch (e) {
      setDictError(String(e));
    }
  }

  // Tastatur-Hook-Status: Ändern ist erst möglich, wenn der Hook verbunden
  // ist (Installation kann beim Start mehrfach fehlschlagen → Neuinstallation
  // im Hintergrund, UI zeigt „wird verbunden").
  useEffect(() => {
    invoke<boolean>("hook_status")
      .then(setHookReady)
      .catch(() => {});
    const un = listen<{ ready: boolean }>("pw://hook-status", (e) =>
      setHookReady(e.payload.ready),
    );
    return () => {
      void un.then((f) => f());
    };
  }, []);

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

  // Key-Capture direkt im Fenster: Beim Klick auf „Ändern“ ist diese App
  // fokussiert, die gedrückte Kombination kommt als normales KeyboardEvent
  // hier an — unabhängig vom globalen Hook (dessen Installation eine
  // Lotterie ist und der bei elevated Fenstern im Vordergrund gar keine
  // Ereignisse sieht, UIPI). Verhaltensvorbild Epicenter/Whispering:
  // preventDefault, Kleinbuchstaben, Fertig beim Loslassen, Esc bricht ab.
  useEffect(() => {
    if (!capturing) return;
    const mods = new Set<string>();
    let sawKey = false;
    const MOD: Record<string, string> = {
      Control: "ctrl",
      Alt: "alt",
      Shift: "shift",
      Meta: "super",
    };
    // handy-keys-Schlüsselname: Einzelzeichen klein, F-Tasten klein,
    // Leerstelle über code (e.key wäre " "). F24 ignorieren — der Hook-
    // Selbsttest im Backend injiziert sie als Gesundheitscheck.
    const keyName = (e: KeyboardEvent): string | null => {
      if (e.key === "F24") return null;
      if (e.key.length === 1 && e.key !== " ") return e.key.toLowerCase();
      if (/^F\d{1,2}$/.test(e.key)) return e.key.toLowerCase();
      if (e.code === "Space") return "space";
      return null;
    };
    const finish = (combo: string | null) => {
      cleanup();
      setCapturing(null);
      if (combo) void applyHotkeys(combo);
    };
    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.repeat) return;
      if (e.key === "Escape") return finish(null);
      if (e.key in MOD) {
        mods.add(MOD[e.key]);
        return;
      }
      const key = keyName(e);
      if (!key) return; // nicht darstellbare Taste (z. B. tot) — weiter lauschen
      sawKey = true;
      // Modifier-Flags direkt aus dem Event — sie sind auch dann korrekt,
      // wenn Modifier schon vor dem Capture-Start gehalten wurden.
      const held = [
        e.ctrlKey ? "ctrl" : "",
        e.altKey ? "alt" : "",
        e.shiftKey ? "shift" : "",
        e.metaKey ? "super" : "",
      ].filter(Boolean);
      finish([...held, key].join("+"));
    };
    const onKeyUp = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (sawKey) return;
      if (!(e.key in MOD)) return;
      // Letzter Modifier losgelassen und keine echte Taste dazwischen →
      // Modifier-only-Kombination (z. B. nur Strg+Alt).
      if (
        mods.size > 0 &&
        !e.ctrlKey &&
        !e.altKey &&
        !e.shiftKey &&
        !e.metaKey
      ) {
        finish([...mods].join("+"));
      }
    };
    const onBlur = () => finish(null);
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    window.addEventListener("blur", onBlur);
    function cleanup() {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
      window.removeEventListener("blur", onBlur);
    }
    return cleanup;
    // applyHotkeys braucht capturing/hotkeys zum Zeitpunkt des Drucks.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [capturing, hotkeys]);

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
  }

  function hotkeyRow(target: Exclude<CaptureTarget, null>, title: string, subtitle: string, hotkey: string) {
    return (
      <div className="row-line" key={target}>
        <div className="row-text">
          <span className="row-title">{title}</span>
          <span className="row-sub">{subtitle}</span>
        </div>
        {capturing === target ? (
          <>
            <span className="kbd capturing">Taste drücken…</span>
            <button className="btn quiet" onClick={() => setCapturing(null)}>
              Abbrechen
            </button>
          </>
        ) : (
          <>
            <span className="kbd">{hotkeyLabel(hotkey)}</span>
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

  function modelBody(): ReactNode {
    const busy =
      download?.state === "downloading" ||
      download?.state === "resuming" ||
      download?.state === "verifying";

    if (busy) {
      const received = download.received ?? 0;
      const total = download.total ?? MODEL_MB * 1024 * 1024;
      const pct = Math.min(100, (received / total) * 100);
      return (
        <>
          <div className="progress-track">
            <span className="progress-fill" style={{ width: `${pct}%` }} />
          </div>
          <div className="row-line">
            <span className="row-sub">
              {download.state === "verifying"
                ? "Prüfsumme wird verifiziert…"
                : `${mb(received)} / ${mb(total)} MB`}
            </span>
            {download.state !== "verifying" && (
              <button
                className="btn quiet"
                onClick={() => invoke("cancel_model_download").catch(() => {})}
              >
                Abbrechen
              </button>
            )}
          </div>
        </>
      );
    }
    if (download?.state === "error") {
      return (
        <>
          <div className="notice" data-tone="error">
            <span className="notice-icon">⚠</span>
            <div>{download.message}</div>
          </div>
          <div className="row-line">
            <span className="row-sub">Der Teilstand bleibt erhalten.</span>
            <button className="btn primary" onClick={startDownload}>
              Erneut versuchen
            </button>
          </div>
        </>
      );
    }
    if (download?.state === "cancelled") {
      return (
        <div className="row-line">
          <span className="row-sub">Abgebrochen — Teilstand bleibt erhalten.</span>
          <button className="btn primary" onClick={startDownload}>
            Fortsetzen
          </button>
        </div>
      );
    }
    if (modelPresent || download?.state === "done") {
      return (
        <div className="row-line">
          <div className="row-text">
            <span className="row-title">
              {engineReady ? "Parakeet TDT 0.6B v3 (Q8)" : "Modell wird geladen…"}
            </span>
            <span className="row-sub">
              {engineReady
                ? `Bereit · ${engineModel} · Vulkan-GPU`
                : "Engine startet nach dem Download automatisch"}
            </span>
          </div>
          <span className="capsule" data-tone={engineReady ? "ok" : "busy"}>
            <span className="dot" />
            {engineReady ? "OK" : "lädt"}
          </span>
        </div>
      );
    }
    return (
      <>
        <div className="row-line">
          <div className="row-text">
            <span className="row-title">Parakeet TDT 0.6B v3 (Q8)</span>
            <span className="row-sub">
              {MODEL_MB} MB von HuggingFace · SHA256-verifiziert · abbrechbar und
              fortsetzbar
            </span>
          </div>
        </div>
        <div className="row-line">
          <span className="row-sub">Lokale Spracherkennung, dauerhaft geladen.</span>
          <button className="btn primary" onClick={startDownload}>
            Modell herunterladen
          </button>
        </div>
      </>
    );
  }

  return (
    <div className="settings">
      <Section head="Tastenkombinationen — halten zum Diktieren">
        {hotkeyRow(
          "cursor",
          "Diktat am Cursor",
          "Text erscheint in der fokussierten App",
          hotkeys.cursor,
        )}
        <div className="divider" />
        {hotkeyRow(
          "handoff",
          "Hand-off-Diktat",
          "Eintrag in die Tagesdatei (für elevated Apps)",
          hotkeys.handoff,
        )}
        <span className="row-sub">
          Gewünschte Kombination direkt hier drücken (dieses Fenster bleibt fokussiert) — auch nur
          Modifier (z. B. Strg+Alt). Escape bricht ab. Nackte Tasten ohne Modifier sind nicht
          erlaubt, weil sie in anderen Apps blockiert wären.
        </span>
        {!hookReady && (
          <div className="notice" data-tone="error">
            <span className="notice-icon">⏳</span>
            <div>
              Tastatur-Anbindung fürs Diktat wird hergestellt… (wird automatisch neu aufgebaut,
              einen Moment gedulden.)
            </div>
          </div>
        )}
        {hotkeyError && (
          <div className="notice" data-tone="error">
            <span className="notice-icon">⚠</span>
            <div>{hotkeyError}</div>
          </div>
        )}
      </Section>

      <Section head="Hand-off-Ablageort">
        <div className="row-line">
          <div className="row-text">
            <span className="row-title">Tagesdateien (JJJJ-MM-TT.md)</span>
            <span className="row-sub">{effectiveHandoffDir}</span>
          </div>
          <button className="btn" onClick={() => void pickHandoffDir()}>
            Auswählen…
          </button>
        </div>
        {customHandoffDir !== null && (
          <div className="row-line">
            <span className="row-sub">Zurück zum Projekt-Standardordner.</span>
            <button className="btn quiet" onClick={() => void applyHandoffDir(null)}>
              Standard wiederherstellen
            </button>
          </div>
        )}
        {handoffError && (
          <div className="notice" data-tone="error">
            <span className="notice-icon">⚠</span>
            <div>{handoffError}</div>
          </div>
        )}
      </Section>

      <Section head="Mikrofon">
        <select
          className="select"
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
        <span className="row-sub">
          Ist dein Headset nicht dabei, Windows-Soundeinstellungen prüfen (Bluetooth-Headsets
          müssen verbunden und als Eingabegerät aktiv sein).
        </span>
      </Section>

      <Section head="Wörterbuch — Fachbegriffe richtig schreiben">
        <div className="row-line">
          <div className="row-text">
            <span className="row-title">
              {dict
                ? `${dict.term_count} Begriffe · ${dict.alias_count} Aliase · ${dict.vocabulary_count} in die Erkennung`
                : "Lädt…"}
            </span>
            <span className="row-sub">
              Regeln wirken auf jedes Diktat (Datei jederzeit im Editor änderbar).
            </span>
          </div>
          <button className="btn" onClick={() => invoke("open_dictionary_dir").catch(console.error)}>
            Ordner öffnen
          </button>
        </div>
        {dict && dict.profiles.length > 0 && (
          <>
            <div className="chips">
              {dict.profiles.map((p) => (
                <button
                  key={p}
                  className={dict.active_profiles.includes(p) ? "chip active" : "chip"}
                  onClick={() => void toggleProfile(p)}
                >
                  {p}
                </button>
              ))}
            </div>
            <span className="row-sub">
              Profile schalten deren Begriffe frei. Beim Umschalten hier gehen Kommentare in
              der YAML verloren — Begriffe nicht.
            </span>
          </>
        )}
        {dictError && (
          <div className="notice" data-tone="error">
            <span className="notice-icon">⚠</span>
            <div>{dictError}</div>
          </div>
        )}
      </Section>

      <Section head="Modell">
        {modelBody()}
      </Section>

      <Section head="Anzeige, Sound & Start">
        <SwitchRow
          title="Overlay-Pille während des Diktats"
          subtitle="Zustand am unteren Bildschirmrand, ohne Fokus zu rauben"
          checked={overlay}
          onChange={toggleOverlay}
        />
        <div className="divider" />
        <SwitchRow
          title="Klick beim Aufnahme-Start"
          subtitle="Dezenter Doppelklick als Aufnahme-Bestätigung"
          checked={feedback}
          onChange={toggleFeedback}
        />
        <div className="divider" />
        <SwitchRow
          title="Autostart mit Windows"
          checked={autostart}
          onChange={(v) => void toggleAutostart(v)}
        />
      </Section>
    </div>
  );
}
