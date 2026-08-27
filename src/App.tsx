import { useEffect, useState } from "react";
import "./App.css";
import AccountsPage from "./AccountsPage";
import BudgetsPage from "./BudgetsPage";
import NeedsReviewPage from "./NeedsReviewPage";
import NetWorthPage from "./NetWorthPage";
import OverviewPage from "./OverviewPage";
import RecurringPage from "./RecurringPage";
import ReportsPage from "./ReportsPage";
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
  | "reports"
  | "networth"
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
      { id: "bills", label: "Bills" },
      { id: "goals", label: "Goals" },
    ],
  },
  {
    label: "Insights",
    items: [
      { id: "reports", label: "Reports" },
      { id: "networth", label: "Net Worth" },
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

function App() {
  const [theme, setTheme] = useState(loadTheme);
  const [tab, setTab] = useState<Tab>("overview");

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
          {tab === "overview" && <OverviewPage onNavigate={setTab} />}
          {tab === "transactions" && <TransactionsPage />}
          {tab === "accounts" && <AccountsPage />}
          {tab === "budgets" && <BudgetsPage />}
          {tab === "bills" && <RecurringPage />}
          {tab === "goals" && <SavingsGoalsPage />}
          {tab === "reports" && <ReportsPage />}
          {tab === "networth" && <NetWorthPage />}
          {tab === "review" && <NeedsReviewPage />}
          {tab === "settings" && (
            <SettingsPage theme={theme} onThemeChange={setTheme} />
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
