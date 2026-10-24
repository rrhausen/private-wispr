// Hauptfenster (M0): Status der Engine, PTT-Hinweis, Mikrofonauswahl, letztes Diktat.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface StatusInfo {
  hotkey: string;
  mic: string | null;
  engine: {
    ready: boolean;
    model: string;
    backend: string;
    on_gpu: boolean;
    device: string | null;
    error: string | null;
    model_path: string;
  };
}

interface LastEvent {
  text: string;
  audio_secs: number;
  transcription_ms: number;
}

export default function App() {
  const [status, setStatus] = useState<StatusInfo | null>(null);
  const [last, setLast] = useState<LastEvent | null>(null);
  const [devices, setDevices] = useState<string[]>([]);
  const [mic, setMic] = useState<string | null>(null);

  useEffect(() => {
    invoke<StatusInfo>("status").then((s) => {
      setStatus(s);
      setMic(s.mic);
    }).catch(console.error);
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

  return (
    <main className="dashboard">
      <h1>Private Wispr <span className="tag">M0</span></h1>

      <section className="card">
        <div className="row">
          <span className="dot" data-on={engine?.ready ?? false} />
          <span>
            {engine?.ready
              ? `Bereit — ${engine.model} · ${engine.backend}${engine.on_gpu ? " (GPU)" : ""}`
              : "Engine wird geladen…"}
          </span>
        </div>
        {engine?.error && (
          <p className="error">
            {engine.error}
            <br />
            <code>scripts/download-model.ps1</code> ausführen, dann App neu starten.
            <br />
            Pfad: <code>{engine.model_path}</code>
          </p>
        )}
      </section>

      <section className="card">
        <h2>Mikrofon</h2>
        <select
          className="mic-select"
          value={mic ?? ""}
          onChange={(e) => chooseMic(e.target.value === "" ? null : e.target.value)}
        >
          <option value="">System-Standard</option>
          {devices.map((name) => (
            <option key={name} value={name}>{name}</option>
          ))}
        </select>
        <p className="meta">
          Ist dein Headset nicht dabei, Windows-Soundeinstellungen prüfen
          (Bluetooth-Headsets müssen verbunden und als Eingabegerät aktiv sein).
        </p>
      </section>

      <section className="card hint">
        <strong>{status?.hotkey ?? "ctrl+space"}</strong> halten, sprechen, loslassen —
        der Text erscheint am Cursor. Brain-Dumps beliebiger Länge sind
        ausdrücklich vorgesehen (Disk-Spooling, kein Timeout).
      </section>

      <section className="card">
        <h2>Letztes Diktat</h2>
        {last ? (
          <>
            <blockquote>{last.text}</blockquote>
            <p className="meta">
              {last.audio_secs.toFixed(1)} s Audio · {last.transcription_ms} ms Transkription
              {last.transcription_ms > 0 && last.audio_secs > 0
                ? ` · ${(last.transcription_ms / 1000 / last.audio_secs).toFixed(2)}× Echtzeit`
                : ""}
            </p>
          </>
        ) : (
          <p className="meta">Noch nichts diktiert.</p>
        )}
      </section>

      <footer className="meta">Privat und lokal — Audio verlässt diesen Rechner nicht.</footer>
    </main>
  );
}
