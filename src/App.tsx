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

interface AppliedRule {
  canonical: string;
  matched: string;
  count: number;
}

interface HistoryEntry {
  id: number;
  ts: string;
  mode: "cursor" | "handoff" | "clipboard";
  raw_transcript: string;
  final_text: string;
  applied_rules: { dictionary_version?: string; rules?: AppliedRule[] } | null;
  target_app: string | null;
  duration_s: number;
  model: string;
  engine: string;
}

const MODE_LABELS: Record<HistoryEntry["mode"], string> = {
  cursor: "Cursor",
  handoff: "Hand-off",
  clipboard: "Zwischenablage",
};

function histTime(ts: string): string {
  const d = new Date(ts);
  const today = new Date();
  const sameDay =
    d.getDate() === today.getDate() &&
    d.getMonth() === today.getMonth() &&
    d.getFullYear() === today.getFullYear();
  return sameDay
    ? d.toLocaleTimeString("de-DE", { hour: "2-digit", minute: "2-digit" })
    : d.toLocaleString("de-DE", {
        day: "2-digit",
        month: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
      });
}

interface LastEvent {
  /** Endtext nach der Regel-Pipeline. */
  text: string;
  /** Rohtranskript vor den Regeln. */
  raw_text?: string;
  applied_rules?: AppliedRule[];
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

export type Theme = "dark" | "light" | "system";

/** Setzt das effektive Schema am <html> („system" wird über die
 *  prefers-color-scheme der Windows-App-Darstellung aufgelöst) und
 *  liefert es zurück — Grundlage für den Sonne/Mond-Knopf. */
function applyTheme(theme: Theme): "dark" | "light" {
  const effective =
    theme === "system" && window.matchMedia("(prefers-color-scheme: light)").matches
      ? "light"
      : theme === "system"
        ? "dark"
        : theme;
  document.documentElement.dataset.theme = effective;
  try {
    localStorage.setItem("pw-theme", effective);
  } catch {
    /* Cache ist optional (Flash-Vermeidung beim Start). */
  }
  return effective;
}

export default function App() {
  const [tab, setTab] = useState<"overview" | "settings">("overview");
  const [status, setStatus] = useState<StatusInfo | null>(null);
  const [last, setLast] = useState<LastEvent | null>(null);
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [confirmClear, setConfirmClear] = useState(false);
  const [theme, setTheme] = useState<Theme>("dark");
  /** Effektives Schema („system" aufgelöst) — steuert Sonne/Mond-Knopf. */
  const [isLight, setIsLight] = useState(false);
  const [devices, setDevices] = useState<string[]>([]);
  const [mic, setMic] = useState<string | null>(null);
  const [handoffDir, setHandoffDir] = useState("");
  const [hotkeys, setHotkeys] = useState<StatusInfo["hotkeys"]>({
    cursor: "ctrl+alt+space",
    handoff: "ctrl+alt+h",
  });

  function loadHistory() {
    invoke<HistoryEntry[]>("history_list", { limit: 10 })
      .then(setHistory)
      .catch(console.error);
  }

  function deleteEntry(id: number) {
    invoke("history_delete", { id })
      .then(loadHistory)
      .catch(console.error);
  }

  function clearHistory() {
    if (!confirmClear) {
      setConfirmClear(true);
      window.setTimeout(() => setConfirmClear(false), 4000);
      return;
    }
    setConfirmClear(false);
    invoke("history_clear")
      .then(loadHistory)
      .catch(console.error);
  }

  function copyEntry(text: string) {
    invoke("copy_text", { text }).catch(console.error);
  }

  useEffect(() => {
    loadHistory();
    const un3 = listen("pw://history-changed", () => loadHistory());
    return () => {
      void un3.then((f) => f());
    };
  }, []);

  // Farbschema aus den Settings laden; im Modus „System" auf OS-Wechsel
  // reagieren (auch zur Laufzeit, z. B. automatischer Wechsel abends).
  useEffect(() => {
    invoke<{ theme: string }>("get_settings")
      .then((s) => {
        const t = (["dark", "light", "system"] as const).includes(s.theme as Theme)
          ? (s.theme as Theme)
          : "dark";
        setTheme(t);
        setIsLight(applyTheme(t) === "light");
      })
      .catch(console.error);
    const mq = window.matchMedia("(prefers-color-scheme: light)");
    const onChange = () => setIsLight(applyTheme(theme) === "light");
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [theme]);

  function chooseTheme(t: Theme) {
    setTheme(t);
    setIsLight(applyTheme(t) === "light");
    invoke("set_theme", { theme: t }).catch(console.error);
  }

  /** Schnell-Umschalter im Header: Icon zeigt das Ziel des Klicks — im
   *  dunklen Modus eine Sonne (Klick → hell), im hellen ein Mond.
   *  „System" wird dabei aufgelöst und zum konkreten Schema. */
  function toggleTheme() {
    chooseTheme(isLight ? "dark" : "light");
  }

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
        <div className="header-side">
          <span className="capsule" data-tone={capsule.tone}>
            <span className="dot" />
            {capsule.label}
          </span>
          <button
            className="theme-toggle"
            onClick={toggleTheme}
            title={isLight ? "Zum dunklen Modus wechseln" : "Zum hellen Modus wechseln"}
            aria-label="Farbschema umschalten"
          >
            {isLight ? "🌙" : "☀️"}
          </button>
        </div>
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
            <div className="section-head">Letzte Diktate</div>
            <div className="card">
              {last ? (
                <>
                  <blockquote className="quote">{last.text}</blockquote>
                  <span className="meta-line">
                    {last.audio_secs.toFixed(1)} s Audio · {last.transcription_ms} ms
                    Transkription · Ziel: {last.target}
                    {last.applied_rules && last.applied_rules.length > 0 &&
                      ` · ${last.applied_rules.reduce((n, r) => n + r.count, 0)}× Wörterbuch`}
                  </span>
                </>
              ) : (
                <span className="row-sub">
                  {history.length > 0 ? "Noch nichts diktiert in dieser Sitzung." : "Noch nichts diktiert."}
                </span>
              )}
              {history.map((h) => (
                <div className="hist-row" key={h.id}>
                  <button
                    className="hist-main"
                    title="Klicken zum Kopieren"
                    onClick={() => copyEntry(h.final_text)}
                  >
                    <span className="hist-snippet">{h.final_text}</span>
                    <span className="hist-meta">
                      {histTime(h.ts)} · {MODE_LABELS[h.mode]}
                      {h.target_app ? ` · ${h.target_app}` : ""} · {h.duration_s.toFixed(1)} s
                      {h.applied_rules?.rules && h.applied_rules.rules.length > 0 &&
                        ` · ${h.applied_rules.rules.reduce((n, r) => n + r.count, 0)}× Wörterbuch`}
                    </span>
                  </button>
                  <button
                    className="btn quiet hist-del"
                    title="Eintrag löschen"
                    onClick={() => deleteEntry(h.id)}
                  >
                    ✕
                  </button>
                </div>
              ))}
              {history.length > 0 && (
                <div className="row-line">
                  <span className="row-sub">
                    Klick auf einen Eintrag kopiert den Text. Aufbewahrung: 90 Tage
                    (Einstellungen).
                  </span>
                  <button className="btn quiet" onClick={clearHistory}>
                    {confirmClear ? "Wirklich alles löschen?" : "Alles löschen"}
                  </button>
                </div>
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
          theme={theme}
          onThemeChange={chooseTheme}
        />
      )}

      <footer className="foot">Privat und lokal — Audio verlässt diesen Rechner nicht.</footer>
    </main>
  );
}
