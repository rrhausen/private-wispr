// Hauptfenster (M1): Tabs Übersicht/Einstellungen. Übersicht zeigt Engine-
// Status, Hotkey-Hinweise und letztes Diktat; Einstellungen (settings.tsx)
// Hotkeys, Hand-off-Ablageort, Mikrofon, Overlay, Sound, Autostart, Modell.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import SettingsPanel, { hotkeyLabel } from "./settings";

interface StatusInfo {
  hotkeys: { cursor: string; handoff: string };
  mic: string | null;
  handoff_dir: string;
  engine: {
    ready: boolean;
    model: string;
    backend: string;
    on_gpu: boolean;
    device: string | null;
    error: string | null;
    model_path: string;
    model_present: boolean;
    model_downloading: boolean;
  };
}

interface LastEvent {
  text: string;
  audio_secs: number;
  transcription_ms: number;
  target: string;
}

function statusCapsule(engine: StatusInfo["engine"] | undefined) {
  if (!engine) return { tone: "busy", label: "Status…" } as const;
  if (engine.ready) {
    return {
      tone: "ok",
      label: `Bereit · ${engine.on_gpu ? "GPU" : engine.backend}`,
    } as const;
  }
  if (engine.error) return { tone: "error", label: "Modell fehlt" } as const;
  return { tone: "busy", label: "Modell lädt" } as const;
}

export default function App() {
  const [tab, setTab] = useState<"overview" | "settings">("overview");
  const [status, setStatus] = useState<StatusInfo | null>(null);
  const [last, setLast] = useState<LastEvent | null>(null);
  const [devices, setDevices] = useState<string[]>([]);
  const [mic, setMic] = useState<string | null>(null);
  const [handoffDir, setHandoffDir] = useState("");
  const [hotkeys, setHotkeys] = useState<StatusInfo["hotkeys"]>({
    cursor: "ctrl+alt+space",
    handoff: "ctrl+alt+h",
  });

  useEffect(() => {
    invoke<StatusInfo>("status")
      .then((s) => {
        setStatus(s);
        setMic(s.mic);
        setHandoffDir(s.handoff_dir);
        setHotkeys(s.hotkeys);
      })
      .catch(console.error);
    invoke<string[]>("list_devices")
      .then((names) => setDevices(names))
      .catch(console.error);
    const un1 = listen<StatusInfo["engine"]>("pw://status", (e) => {
      setStatus((prev) => (prev ? { ...prev, engine: e.payload } : prev));
    });
    const un2 = listen<LastEvent>("pw://last", (e) => setLast(e.payload));
    return () => {
      void un1.then((f) => f());
      void un2.then((f) => f());
    };
  }, []);

  function chooseMic(name: string | null) {
    setMic(name);
    invoke("set_mic", { name }).catch(console.error);
  }

  const engine = status?.engine;
  const capsule = statusCapsule(engine);

  return (
    <main className="page">
      <header className="app-header">
        <div className="brand">
          <span className="brand-mark">🎙</span>
          <div>
            <div className="brand-name">Private Wispr</div>
            <div className="brand-sub">Diktat, lokal auf diesem Rechner</div>
          </div>
        </div>
        <span className="capsule" data-tone={capsule.tone} style={{ marginLeft: "auto" }}>
          <span className="dot" />
          {capsule.label}
        </span>
      </header>

      <nav className="segmented" role="tablist">
        <button
          className={tab === "overview" ? "segment active" : "segment"}
          onClick={() => setTab("overview")}
        >
          <span className="check">{tab === "overview" ? "✓" : ""}</span>
          Übersicht
        </button>
        <button
          className={tab === "settings" ? "segment active" : "segment"}
          onClick={() => setTab("settings")}
        >
          <span className="check">{tab === "settings" ? "✓" : ""}</span>
          Einstellungen
        </button>
      </nav>

      {tab === "overview" ? (
        <>
          <section className="section">
            <div className="section-head">Spracherkennung</div>
            <div className="card">
              <div className="row-line">
                <div className="row-text">
                  <span className="row-title">
                    {engine?.ready
                      ? `${engine.model} · ${engine.backend}${engine.on_gpu ? " · GPU" : ""}`
                      : "Engine wird geladen…"}
                  </span>
                  <span className="row-sub">
                    Sprache Deutsch · Text verlässt diesen Rechner nicht
                  </span>
                </div>
              </div>
              {engine?.error && (
                <div className="notice" data-tone="error">
                  <span className="notice-icon">⚠</span>
                  <div>
                    <strong>Modell fehlt.</strong> In{" "}
                    <em>Einstellungen → Modell</em> direkt in der App herunterladen oder{" "}
                    <code className="inline">scripts/download-model.ps1</code> ausführen.
                    <br />
                    Pfad: <code className="inline">{engine.model_path}</code>
                  </div>
                </div>
              )}
            </div>
          </section>

          <section className="section">
            <div className="section-head">Diktat — Taste halten, sprechen, loslassen</div>
            <div className="card">
              <div className="row-line">
                <div className="row-text">
                  <span className="row-title">Am Cursor einfügen</span>
                  <span className="row-sub">Text erscheint direkt in der fokussierten App</span>
                </div>
                <span className="kbd">{hotkeyLabel(hotkeys.cursor)}</span>
              </div>
              <div className="divider" />
              <div className="row-line">
                <div className="row-text">
                  <span className="row-title">Hand-off-Tagesdatei</span>
                  <span className="row-sub">
                    Für elevated Apps (z. B. Agenten-IDEs), die Strg+V nicht annehmen
                  </span>
                </div>
                <span className="kbd">{hotkeyLabel(hotkeys.handoff)}</span>
              </div>
            </div>
          </section>

          <section className="section">
            <div className="section-head">Letztes Diktat</div>
            <div className="card">
              {last ? (
                <>
                  <blockquote className="quote">{last.text}</blockquote>
                  <span className="meta-line">
                    {last.audio_secs.toFixed(1)} s Audio · {last.transcription_ms} ms
                    Transkription · Ziel: {last.target}
                  </span>
                </>
              ) : (
                <span className="row-sub">Noch nichts diktiert.</span>
              )}
            </div>
          </section>
        </>
      ) : (
        <SettingsPanel
          hotkeys={hotkeys}
          onHotkeysChange={setHotkeys}
          mic={mic}
          devices={devices}
          onMicChange={chooseMic}
          effectiveHandoffDir={handoffDir}
          onHandoffDirChange={setHandoffDir}
          modelPresent={engine?.model_present ?? false}
          engineReady={engine?.ready ?? false}
          engineModel={engine?.model ?? ""}
        />
      )}

      <footer className="foot">Privat und lokal — Audio verlässt diesen Rechner nicht.</footer>
    </main>
  );
}
