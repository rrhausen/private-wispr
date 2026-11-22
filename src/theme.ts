// Farbschema (K5): „system" wird über die prefers-color-scheme der Windows-
// App-Darstellung aufgelöst, das effektive Schema am <html> gesetzt und im
// localStorage gegen den Start-Flash gepuffert (main.tsx liest ihn vor dem
// ersten Render; die Wahrheit steht in den Settings).
//
// useTheme montiert den OS-Wechsel-Listener genau EINMAL — früher lief die
// Montage in einem [theme]-Effekt in App.tsx und wurde bei jedem Schema-
// Wechsel neu aufgesetzt (und die Settings jedes Mal neu geholt).
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type Theme = "dark" | "light" | "system";

const THEMES: readonly Theme[] = ["dark", "light", "system"];

/** Effektives Schema am <html> setzen und zurückliefern. */
export function applyTheme(theme: Theme): "dark" | "light" {
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

/** Schema-Zustand der App: beim Start aus den Settings laden, OS-Wechseln
 *  live folgen (nur im Modus „system" sichtbar wirkend), Änderungen via
 *  set_theme zurückschreiben. */
export function useTheme() {
  const [theme, setThemeState] = useState<Theme>("dark");
  /** Aktuelles Schema für den einmal montierten OS-Listener (statt [theme]-Effekt). */
  const themeRef = useRef<Theme>("dark");
  /** Effektives Schema („system" aufgelöst) — steuert Sonne/Mond-Knopf. */
  const [isLight, setIsLight] = useState(false);

  function setTheme(t: Theme) {
    themeRef.current = t;
    setThemeState(t);
    setIsLight(applyTheme(t) === "light");
    invoke("set_theme", { theme: t }).catch(console.error);
  }

  useEffect(() => {
    invoke<{ theme: string }>("get_settings")
      .then((s) => {
        const t = THEMES.includes(s.theme as Theme) ? (s.theme as Theme) : "dark";
        themeRef.current = t;
        setThemeState(t);
        setIsLight(applyTheme(t) === "light");
      })
      .catch(console.error);
    const mq = window.matchMedia("(prefers-color-scheme: light)");
    const onChange = () => setIsLight(applyTheme(themeRef.current) === "light");
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);

  /** Schnell-Umschalter im Header: Icon zeigt das Ziel des Klicks — im
   *  dunklen Modus eine Sonne (Klick → hell), im hellen ein Mond. */
  function toggleTheme() {
    setTheme(isLight ? "dark" : "light");
  }

  return { theme, isLight, setTheme, toggleTheme };
}
