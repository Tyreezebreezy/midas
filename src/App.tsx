import { useEffect, useState } from "react";
import "./App.css";
import {
  parseThemePreference,
  resolveTheme,
  type ThemePreference,
} from "./theme";

const themeOptions: ThemePreference[] = ["system", "light", "dark"];

function loadTheme(): ThemePreference {
  try {
    return parseThemePreference(localStorage.getItem("midas-theme"));
  } catch {
    return "system";
  }
}

function App() {
  const [theme, setTheme] = useState(loadTheme);

  useEffect(() => {
    const systemDark = window.matchMedia("(prefers-color-scheme: dark)");
    const applyTheme = () => {
      document.documentElement.dataset.theme = resolveTheme(
        theme,
        systemDark.matches,
      );
    };

    applyTheme();
    try {
      localStorage.setItem("midas-theme", theme);
    } catch {
      // The theme still works when storage is unavailable.
    }

    if (theme === "system") {
      systemDark.addEventListener("change", applyTheme);
      return () => systemDark.removeEventListener("change", applyTheme);
    }
  }, [theme]);

  return (
    <div className="app-shell">
      <header className="app-header">
        <span className="brand">MIDAS</span>
        <fieldset className="theme-control">
          <legend>Theme</legend>
          {themeOptions.map((option) => (
            <label key={option}>
              <input
                type="radio"
                name="theme"
                value={option}
                checked={theme === option}
                onChange={() => setTheme(option)}
              />
              <span>{option[0].toUpperCase() + option.slice(1)}</span>
            </label>
          ))}
        </fieldset>
      </header>

      <main>
        <section aria-labelledby="ready-heading">
          <p className="eyebrow">
            Private &middot; Local-first &middot; Explainable
          </p>
          <h1 id="ready-heading">Where do you stand financially?</h1>
          <p className="lede">
            MIDAS is your local personal finance command center. This is the
            foundation shell &mdash; accounts, imports, and the dashboard arrive
            in the phases ahead.
          </p>
          <ul className="status-list">
            {[
              "Local-first: no cloud account required",
              "Theme preference saved on this device",
              "No network authority",
            ].map((item) => (
              <li key={item}>
                <span aria-hidden="true">✓</span>
                {item}
              </li>
            ))}
          </ul>
        </section>
      </main>

      <footer>
        <span>MIDAS · Tauri 2 · React · TypeScript · Vite</span>
        <span>Local only</span>
      </footer>
    </div>
  );
}

export default App;
