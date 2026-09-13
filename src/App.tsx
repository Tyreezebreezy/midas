import { useEffect, useState } from "react";
import "./App.css";
import AccountsPage from "./AccountsPage";
import BudgetsPage from "./BudgetsPage";
import ForecastPage from "./ForecastPage";
import InsightsPage from "./InsightsPage";
import NeedsReviewPage from "./NeedsReviewPage";
import OverviewPage from "./OverviewPage";
import RecurringPage from "./RecurringPage";
import SavingsGoalsPage from "./SavingsGoalsPage";
import SettingsPage from "./SettingsPage";
import TransactionsPage from "./TransactionsPage";
import {
  parseThemePreference,
  resolveTheme,
  type ThemePreference,
} from "./theme";

export type Tab =
  | "overview"
  | "transactions"
  | "accounts"
  | "budgets"
  | "bills"
  | "goals"
  | "insights"
  | "forecast"
  | "review"
  | "settings";

const navGroups: {
  label: string | null;
  items: { id: Tab; label: string }[];
}[] = [
  { label: null, items: [{ id: "overview", label: "Overview" }] },
  {
    label: "Money",
    items: [
      { id: "transactions", label: "Transactions" },
      { id: "accounts", label: "Accounts" },
    ],
  },
  {
    label: "Planning",
    items: [
      { id: "budgets", label: "Budget" },
      { id: "bills", label: "Recurring" },
      { id: "goals", label: "Goals" },
    ],
  },
  {
    label: "Insights",
    items: [
      { id: "insights", label: "Insights" },
      { id: "forecast", label: "Forecast" },
    ],
  },
  { label: "Review", items: [{ id: "review", label: "Needs Review" }] },
];

function loadTheme(): ThemePreference {
  try {
    return parseThemePreference(localStorage.getItem("midas-theme"));
  } catch {
    return "system";
  }
}

function loadDisplayName(): string {
  try {
    return localStorage.getItem("midas-display-name") ?? "";
  } catch {
    return "";
  }
}

function App() {
  const [theme, setTheme] = useState(loadTheme);
  const [tab, setTab] = useState<Tab>("overview");
  const [displayName, setDisplayName] = useState(loadDisplayName);

  useEffect(() => {
    try {
      localStorage.setItem("midas-display-name", displayName);
    } catch {
      // The name still works for this session when storage is unavailable.
    }
  }, [displayName]);

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
      <aside className="sidebar">
        <span className="brand">MIDAS</span>
        <nav className="sidebar-nav" aria-label="Main">
          {navGroups.map((group, i) => (
            <div className="sidebar-group" key={group.label ?? `g${i}`}>
              {group.label && (
                <span className="sidebar-group-label">{group.label}</span>
              )}
              {group.items.map((item) => (
                <button
                  key={item.id}
                  type="button"
                  className={
                    item.id === tab
                      ? "sidebar-link sidebar-link-active"
                      : "sidebar-link"
                  }
                  aria-current={item.id === tab ? "page" : undefined}
                  onClick={() => setTab(item.id)}
                >
                  {item.label}
                </button>
              ))}
            </div>
          ))}
        </nav>
        <div className="sidebar-footer">
          <button
            type="button"
            className={
              tab === "settings"
                ? "sidebar-link sidebar-link-active"
                : "sidebar-link"
            }
            aria-current={tab === "settings" ? "page" : undefined}
            onClick={() => setTab("settings")}
          >
            Settings
          </button>
        </div>
      </aside>

      <div className="main-column">
        <main>
          {tab === "overview" && (
            <OverviewPage onNavigate={setTab} displayName={displayName} />
          )}
          {tab === "transactions" && <TransactionsPage onNavigate={setTab} />}
          {tab === "accounts" && <AccountsPage />}
          {tab === "budgets" && <BudgetsPage />}
          {tab === "bills" && <RecurringPage onNavigate={setTab} />}
          {tab === "goals" && <SavingsGoalsPage />}
          {tab === "insights" && <InsightsPage />}
          {tab === "forecast" && <ForecastPage />}
          {tab === "review" && <NeedsReviewPage />}
          {tab === "settings" && (
            <SettingsPage
              theme={theme}
              onThemeChange={setTheme}
              displayName={displayName}
              onDisplayNameChange={setDisplayName}
            />
          )}
        </main>

        <footer>
          <span>MIDAS · Tauri 2 · React · TypeScript · Vite</span>
          <span>Local only</span>
        </footer>
      </div>
    </div>
  );
}

export default App;
