import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/public-sans";
import "./App.css";
import App from "./App";

// Zuletzt gewähltes Farbschema sofort anwenden (vor dem ersten Render), damit
// es beim Start nicht kurz dunkel aufblitzt. Der Cache wird in App.tsx bei
// jeder Änderung aktualisiert; die Wahrheit steht in den Settings.
try {
  const cached = localStorage.getItem("pw-theme");
  if (cached === "light" || cached === "dark") {
    document.documentElement.dataset.theme = cached;
  }
} catch {
  // localStorage nicht verfügbar → Default (dunkel) bleibt, kein Problem.
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
