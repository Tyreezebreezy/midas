import { useEffect, useState } from "react";
import "./App.css";
import AccountsPage from "./AccountsPage";
import BudgetsPage from "./BudgetsPage";
import NeedsReviewPage from "./NeedsReviewPage";
import NetWorthPage from "./NetWorthPage";
import OverviewPage from "./OverviewPage";
import PayCyclePage from "./PayCyclePage";
import RecurringPage from "./RecurringPage";
import ReportsPage from "./ReportsPage";
import RulesPage from "./RulesPage";
import SafeToSpendPage from "./SafeToSpendPage";
import SavingsGoalsPage from "./SavingsGoalsPage";
import TransactionsPage from "./TransactionsPage";
import {
  parseThemePreference,
  resolveTheme,
  type ThemePreference,
} from "./theme";

const themeOptions: ThemePreference[] = ["system", "light", "dark"];

type Tab =
  | "overview"
  | "accounts"
  | "transactions"
  | "budgets"
  | "goals"
  | "paycycle"
  | "safetospend"
  | "networth"
  | "recurring"
  | "rules"
  | "review"
  | "reports";

const tabs: { id: Tab; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "accounts", label: "Accounts" },
  { id: "transactions", label: "Transactions" },
  { id: "budgets", label: "Budgets" },
  { id: "goals", label: "Savings Goals" },
  { id: "paycycle", label: "Pay Cycle" },
  { id: "safetospend", label: "Safe to Spend" },
  { id: "networth", label: "Net Worth" },
  { id: "recurring", label: "Recurring & Bills" },
  { id: "rules", label: "Rules" },
  { id: "review", label: "Needs Review" },
  { id: "reports", label: "Reports" },
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

      <nav className="main-nav" aria-label="Main">
        {tabs.map((t) => (
          <button
            key={t.id}
            type="button"
            className={t.id === tab ? "nav-tab nav-tab-active" : "nav-tab"}
            aria-current={t.id === tab ? "page" : undefined}
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </nav>

      <main>
        {tab === "overview" && <OverviewPage />}
        {tab === "accounts" && <AccountsPage />}
        {tab === "transactions" && <TransactionsPage />}
        {tab === "budgets" && <BudgetsPage />}
        {tab === "goals" && <SavingsGoalsPage />}
        {tab === "paycycle" && <PayCyclePage />}
        {tab === "safetospend" && <SafeToSpendPage />}
        {tab === "networth" && <NetWorthPage />}
        {tab === "recurring" && <RecurringPage />}
        {tab === "rules" && <RulesPage />}
        {tab === "review" && <NeedsReviewPage />}
        {tab === "reports" && <ReportsPage />}
      </main>

      <footer>
        <span>MIDAS · Tauri 2 · React · TypeScript · Vite</span>
        <span>Local only</span>
      </footer>
    </div>
  );
}

export default App;
