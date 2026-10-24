// Overlay-Pille (M0): zeigt Aufnahme/Transkription/Fertig/Fehler ohne
// Fokusraub. Das Fenster selbst ist transparent, always-on-top und wird vom
// Rust-Backend positioniert und gezeigt/versteckt.
import { createRoot } from "react-dom/client";
import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import "@fontsource-variable/public-sans";
import "./App.css";

type Phase = "recording" | "transcribing" | "done" | "error";

interface OverlayEvent {
  phase: Phase;
  message?: string;
  level?: number;
}

function Pill() {
  const [phase, setPhase] = useState<Phase>("recording");
  const [message, setMessage] = useState<string | null>(null);
  const [level, setLevel] = useState(0);
  const [elapsed, setElapsed] = useState(0);

  useEffect(() => {
    const un1 = listen<OverlayEvent>("pw://overlay", (e) => {
      setPhase(e.payload.phase);
      setMessage(e.payload.message ?? null);
      if (e.payload.phase === "recording") setElapsed(0);
    });
    const un2 = listen<number>("pw://level", (e) => setLevel(e.payload));
    const timer =
      phase === "recording"
        ? setInterval(() => setElapsed((s) => s + 0.1), 100)
        : null;
    return () => {
      void un1.then((f) => f());
      void un2.then((f) => f());
      if (timer) clearInterval(timer);
    };
  }, [phase]);

  const bars = Array.from({ length: 7 }, (_, i) => {
    const base = phase === "recording" ? level : 0.06;
    const wobble = Math.sin(Date.now() / 120 + i * 1.3) * 0.04;
    return Math.max(0.05, Math.min(1, base * (0.6 + 0.4 * Math.sin(i * 0.9)) + wobble));
  });

  const label =
    phase === "recording"
      ? `${elapsed.toFixed(1)} s`
      : phase === "transcribing"
        ? "schreibt…"
        : phase === "done"
          ? (message ?? "fertig ✓")
          : (message ?? "Fehler");

  return (
    <div className="pill">
      {phase === "transcribing" ? (
        <span className="spinner" />
      ) : (
        <span className="mic">{phase === "recording" ? "🎙" : phase === "done" ? "✓" : "⚠"}</span>
      )}
      <span className="label">{label}</span>
      <span className="bars">
        {bars.map((v, i) => (
          <span key={i} className="bar" style={{ height: `${6 + v * 18}px` }} />
        ))}
      </span>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Pill />);
