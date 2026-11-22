// Setup-Assistent erster Start (specs 4.10, gegrillt 22.11.): Willkommen →
// Mikrofon (mit 5-s-Pegel-Diagnose) → Modell (Download oder bereits da) →
// Diktat-Test (E2E-Finale: Hook + Engine + Mikro in einem echten Durchlauf)
// → Fertig. Vollfläche im Hauptfenster statt der Tabs; Status läuft über
// die bestehenden Events (pw://level, pw://mic-test, pw://model-download,
// pw://hook-status, pw://last, pw://status), kein Polling. „Später" beendet
// ohne setup_done (der Assistent kommt beim nächsten Start wieder);
// Bestandsinstallationen sieht die Migration in setup.rs gar nicht erst.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { hotkeyLabel } from "./settings";

interface WizardStatus {
  hotkeys: { cursor: string; handoff: string };
  mic: string | null;
  engine: {
    ready: boolean;
    model: string;
    backend: string;
    model_path: string;
    model_present: boolean;
    model_downloading: boolean;
  };
}

interface DownloadEvent {
  state: "resuming" | "downloading" | "verifying" | "done" | "cancelled" | "error";
  received?: number;
  total?: number;
  message?: string;
}

const MODEL_MB = 705;
const STEPS = ["Willkommen", "Mikrofon", "Sprachmodell", "Diktat-Test", "Fertig"];

function mb(n: number): string {
  return (n / (1024 * 1024)).toFixed(0);
}

export default function SetupWizard({ onFinished }: { onFinished: () => void }) {
  const [step, setStep] = useState(0);
  const [status, setStatus] = useState<WizardStatus | null>(null);
  const [devices, setDevices] = useState<string[]>([]);
  const [mic, setMic] = useState<string | null>(null);
  const [level, setLevel] = useState<number | null>(null);
  const [micTesting, setMicTesting] = useState(false);
  const [download, setDownload] = useState<DownloadEvent | null>(null);
  const [hookReady, setHookReady] = useState(true);
  const [dictated, setDictated] = useState<string | null>(null);

  function loadStatus() {
    invoke<WizardStatus>("status")
      .then((s) => {
        setStatus(s);
        setMic(s.mic);
      })
      .catch(console.error);
  }

  useEffect(() => {
    loadStatus();
    invoke<string[]>("list_devices")
      .then(setDevices)
      .catch(console.error);
    invoke<boolean>("hook_status")
      .then(setHookReady)
      .catch(() => {});
  }, []);

  useEffect(() => {
    const subs = [
      listen<WizardStatus["engine"]>("pw://status", (e) =>
        setStatus((prev) => (prev ? { ...prev, engine: e.payload } : prev)),
      ),
      listen<number | null>("pw://level", (e) => setLevel(e.payload)),
      listen<boolean>("pw://mic-test", (e) => {
        setMicTesting(e.payload);
        if (!e.payload) setLevel(null);
      }),
      listen<DownloadEvent>("pw://model-download", (e) => setDownload(e.payload)),
      listen<{ ready: boolean }>("pw://hook-status", (e) => setHookReady(e.payload.ready)),
      listen<{ text: string }>("pw://last", (e) => setDictated(e.payload.text)),
    ];
    return () => {
      for (const un of subs) void un.then((f) => f());
    };
  }, []);

  // Schritt verlassen oder Assistent schließen: laufenden Mikro-Test sauber
  // beenden (Gerät schließt, Warmhalten bleibt erhalten).
  useEffect(() => {
    if (step !== 1 && micTesting) {
      invoke("mic_test_stop").catch(() => {});
    }
  }, [step, micTesting]);

  useEffect(
    () => () => {
      invoke("mic_test_stop").catch(() => {});
    },
    [],
  );

  function chooseMic(name: string | null) {
    setMic(name);
    invoke("set_mic", { name }).catch(console.error);
    setMicTesting(false);
    setLevel(null);
  }

  function startMicTest() {
    setDictated(null);
    invoke("mic_test_start").catch(console.error);
  }

  function startDownload() {
    invoke("download_model").catch(console.error);
  }

  function finish() {
    invoke("finish_setup")
      .then(onFinished)
      .catch(console.error);
  }

  const busyDownload =
    download?.state === "downloading" ||
    download?.state === "resuming" ||
    download?.state === "verifying";
  const engineReady = status?.engine.ready ?? false;
  const modelPresent = status?.engine.model_present ?? false;
  const cursorKey = hotkeyLabel(status?.hotkeys.cursor ?? "ctrl+alt+space");

  return (
    <section className="section">
      <div className="section-head">Einrichtung · Schritt {step + 1} von {STEPS.length}</div>
      <div className="card">
        {/* Schritt-Punkte */}
        <div className="segmented" role="tablist" aria-label="Einrichtungsschritte">
          {STEPS.map((label, i) => (
            <span
              key={label}
              className={i === step ? "segment active" : i < step ? "segment" : "segment"}
              style={i < step ? { opacity: 0.85 } : i > step ? { opacity: 0.5 } : undefined}
            >
              <span className="check">{i < step ? "✓" : ""}</span>
              {label}
            </span>
          ))}
        </div>

        {step === 0 && (
          <>
            <div className="row-line">
              <div className="row-text">
                <span className="row-title">Willkommen bei Private Wispr</span>
                <span className="row-sub">
                  Diktieren durch Halten der Diktattaste, Sprechen, Loslassen — der Text
                  erscheint am Cursor. Alles läuft lokal auf diesem Rechner. Drei kurze
                  Schritte: Mikrofon, Sprachmodell, erster Test.
                </span>
              </div>
            </div>
            <div className="row-line">
              <div className="row-text">
                <span className="row-title">Deine Diktattaste: {cursorKey} halten</span>
                <span className="row-sub">
                  Hand-off-Taste (Tippen für lange Diktate in die Tagesdatei):{" "}
                  {hotkeyLabel(status?.hotkeys.handoff ?? "ctrl+alt+h")}. Beides ist später
                  frei konfigurierbar.
                </span>
              </div>
            </div>
            <div className="row-line">
              <button className="btn quiet" onClick={() => onFinished()}>
                Später
              </button>
              <button className="btn primary" onClick={() => setStep(1)}>
                Los geht&apos;s
              </button>
            </div>
          </>
        )}

        {step === 1 && (
          <>
            <div className="row-line">
              <div className="row-text">
                <span className="row-title">Mikrofon wählen</span>
                <span className="row-sub">
                  Sprich beim Test etwas normal laut — der Balken sollte sich deutlich
                  bewegen. Bluetooth-Headsets brauchen nach dem Verbinden manchmal einen
                  Moment.
                </span>
              </div>
            </div>
            <div className="row-line">
              <select
                className="select"
                value={mic ?? ""}
                onChange={(e) => chooseMic(e.target.value === "" ? null : e.target.value)}
              >
                <option value="">System-Standard</option>
                {devices.map((name) => (
                  <option key={name} value={name}>
                    {name}
                  </option>
                ))}
              </select>
              <button
                className="btn"
                onClick={startMicTest}
                disabled={micTesting}
              >
                {micTesting ? "Test läuft…" : "Mikrofon testen"}
              </button>
            </div>
            {micTesting && (
              <div className="progress-track">
                <span className="progress-fill" style={{ width: `${Math.min(100, (level ?? 0) * 100)}%` }} />
              </div>
            )}
            {micTesting && (
              <span className="row-sub">
                Sprich etwas — der Balken zeigt die Lautstärke. Der Test endet von selbst.
              </span>
            )}
            <div className="row-line">
              <button className="btn quiet" onClick={() => setStep(0)}>
                Zurück
              </button>
              <button className="btn primary" onClick={() => setStep(2)}>
                Weiter
              </button>
            </div>
          </>
        )}

        {step === 2 && (
          <>
            <div className="row-line">
              <div className="row-text">
                <span className="row-title">Sprachmodell (705 MB, einmalig)</span>
                <span className="row-sub">
                  Parakeet TDT 0.6B v3 (Q8) von HuggingFace · SHA256-verifiziert ·
                  abbrechbar und fortsetzbar. Liegt eine Kopie bereit (z. B. neben der
                  private-wispr.exe), wird sie automatisch erkannt.
                </span>
              </div>
            </div>

            {modelPresent || download?.state === "done" ? (
              <div className="row-line">
                <div className="row-text">
                  <span className="row-title">Modell bereits vorhanden ✓</span>
                  <span className="row-sub">
                    {engineReady
                      ? `Bereit · ${status?.engine.model} · ${status?.engine.backend}`
                      : "Engine startet und lädt das Modell…"}
                  </span>
                </div>
                <span className="capsule" data-tone={engineReady ? "ok" : "busy"}>
                  <span className="dot" />
                  {engineReady ? "OK" : "lädt"}
                </span>
              </div>
            ) : busyDownload ? (
              <>
                <div className="progress-track">
                  <span
                    className="progress-fill"
                    style={{
                      width: `${Math.min(100, ((download.received ?? 0) / (download.total ?? MODEL_MB * 1024 * 1024)) * 100)}%`,
                    }}
                  />
                </div>
                <div className="row-line">
                  <span className="row-sub">
                    {download.state === "verifying"
                      ? "Prüfsumme wird verifiziert…"
                      : `${mb(download.received ?? 0)} / ${mb(download.total ?? MODEL_MB * 1024 * 1024)} MB`}
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
            ) : download?.state === "error" ? (
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
            ) : download?.state === "cancelled" ? (
              <div className="row-line">
                <span className="row-sub">Abgebrochen — Teilstand bleibt erhalten.</span>
                <button className="btn primary" onClick={startDownload}>
                  Fortsetzen
                </button>
              </div>
            ) : (
              <div className="row-line">
                <span className="row-sub">Ohne Modell kann Private Wispr nicht transkribieren.</span>
                <button className="btn primary" onClick={startDownload}>
                  Modell herunterladen
                </button>
              </div>
            )}

            <div className="row-line">
              <button className="btn quiet" onClick={() => setStep(1)}>
                Zurück
              </button>
              {!busyDownload && (
                <button className="btn quiet" onClick={() => onFinished()}>
                  Später
                </button>
              )}
              <button
                className="btn primary"
                disabled={!modelPresent && download?.state !== "done"}
                onClick={() => setStep(3)}
              >
                Weiter
              </button>
            </div>
          </>
        )}

        {step === 3 && (
          <>
            <div className="row-line">
              <div className="row-text">
                <span className="row-title">Erster Diktat-Test</span>
                <span className="row-sub">
                  {hookReady && engineReady ? (
                    <>
                      Halte <strong>{cursorKey}</strong>, sag einen kurzen Satz, lasse los —
                      der erkannte Text erscheint hier. Das ist der Echtest der ganzen
                      Kette: Taste, Mikrofon, Sprachmodell.
                    </>
                  ) : (
                    <>Warte auf die Anbindungen — danach halte {cursorKey} und sprich.</>
                  )}
                </span>
              </div>
            </div>
            <div className="row-line">
              <span className="capsule" data-tone={hookReady ? "ok" : "busy"}>
                <span className="dot" />
                {hookReady ? "Tastatur-Anbindung OK" : "Tastatur-Anbindung wird hergestellt…"}
              </span>
              <span className="capsule" data-tone={engineReady ? "ok" : "busy"}>
                <span className="dot" />
                {engineReady ? "Sprachmodell bereit" : "Sprachmodell lädt…"}
              </span>
            </div>
            {dictated !== null && (
              <div className="notice">
                <span className="notice-icon">✓</span>
                <div>Erkannt: „{dictated}"</div>
              </div>
            )}
            <div className="row-line">
              <button className="btn quiet" onClick={() => setStep(2)}>
                Zurück
              </button>
              <button className="btn quiet" onClick={() => finish()}>
                Ohne Test fertigstellen
              </button>
              {dictated !== null && (
                <button className="btn primary" onClick={() => setStep(4)}>
                  Sieht gut aus — weiter
                </button>
              )}
            </div>
          </>
        )}

        {step === 4 && (
          <>
            <div className="row-line">
              <div className="row-text">
                <span className="row-title">Einrichtung abgeschlossen 🎉</span>
                <span className="row-sub">
                  Mikrofon: {mic ?? "System-Standard"} · Modell: {status?.engine.model || "—"}{" "}
                  · Diktattaste: {cursorKey} halten.
                </span>
              </div>
            </div>
            <div className="row-line">
              <span className="row-sub">
                Private Wispr läuft im Hintergrund weiter (Tray). Escape bricht eine
                Aufnahme ab, X minimiert das Fenster — Beenden nur übers Tray-Menü.
              </span>
              <button className="btn primary" onClick={() => finish()}>
                Fertig
              </button>
            </div>
          </>
        )}
      </div>
    </section>
  );
}
