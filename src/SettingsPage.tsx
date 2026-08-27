import BackupPage from "./BackupPage";
import PayCycleSettings from "./PayCycleSettings";
import RulesPage from "./RulesPage";
import SafeToSpendReserveForm from "./SafeToSpendReserveForm";
import type { ThemePreference } from "./theme";

const themeOptions: ThemePreference[] = ["system", "light", "dark"];

function SettingsPage({
  theme,
  onThemeChange,
}: {
  theme: ThemePreference;
  onThemeChange: (preference: ThemePreference) => void;
}) {
  return (
    <div>
      <h1>Settings</h1>
      <div className="settings-sections">
        <section
          aria-labelledby="appearance-heading"
          className="settings-section"
        >
          <h2 id="appearance-heading">Appearance</h2>
          <fieldset className="theme-control">
            <legend>Theme</legend>
            {themeOptions.map((option) => (
              <label key={option}>
                <input
                  type="radio"
                  name="theme"
                  value={option}
                  checked={theme === option}
                  onChange={() => onThemeChange(option)}
                />
                <span>{option[0].toUpperCase() + option.slice(1)}</span>
              </label>
            ))}
          </fieldset>
        </section>

        <section
          aria-labelledby="financial-heading"
          className="settings-section"
        >
          <h2 id="financial-heading">Financial Preferences</h2>
          <PayCycleSettings />
          <SafeToSpendReserveForm />
        </section>

        <div className="settings-section">
          <RulesPage />
        </div>

        <div className="settings-section">
          <BackupPage />
        </div>

        <section aria-labelledby="about-heading" className="settings-section">
          <h2 id="about-heading">About</h2>
          {/* ponytail: version is a manual sync point alongside Cargo.toml/package.json/tauri.conf.json - no Tauri capability exists to read it at runtime, and adding one just for this label isn't worth the new permission surface. */}
          <p>MIDAS version 0.1.0</p>
          <p className="muted">
            Private, local-first personal finance. No cloud account, no bank
            sync, no telemetry.
          </p>
          <p className="muted">
            Data file:{" "}
            <code className="settings-data-path">
              %APPDATA%\com.breezy.midas\midas.sqlite3
            </code>
          </p>
        </section>
      </div>
    </div>
  );
}

export default SettingsPage;
