import { useEffect, useState } from "react";
import "./App.css";
import { accountsApi } from "./api/midas";
import AccountsPage from "./AccountsPage";
import BudgetsPage from "./BudgetsPage";
import ForecastPage from "./ForecastPage";
import InsightsPage from "./InsightsPage";
import OnboardingFlow from "./OnboardingFlow";
import OverviewPage from "./OverviewPage";
import RecurringPage from "./RecurringPage";
import ReviewPage from "./ReviewPage";
import SavingsGoalsPage from "./SavingsGoalsPage";
import SettingsPage from "./SettingsPage";
import TransactionsPage from "./TransactionsPage";
import { isPrivacyModeOn, setPrivacyMode, useMasked } from "./lib/currency";
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
      { id: "budgets", label: "Budgets" },
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
  { label: "Review", items: [{ id: "review", label: "Review" }] },
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

// Privacy Mode (Batch 10): a display-only mask over monetary amounts, not
// database encryption - see docs/architecture. Deliberately session-only
// (never persisted, matching the recommendation in the product handoff),
// so a fresh launch never shows numbers unexpectedly hidden.
function PrivacyToggle() {
  useMasked();
  const on = isPrivacyModeOn();
  const fullLabel = on ? "Show financial amounts" : "Hide financial amounts";
  return (
    <button
      type="button"
      className="sidebar-link privacy-toggle"
      aria-pressed={on}
      title={fullLabel}
      onClick={() => setPrivacyMode(!on)}
    >
      <svg
        viewBox="0 0 24 24"
        width="16"
        height="16"
        aria-hidden="true"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M1.5 12S5 5 12 5s10.5 7 10.5 7-3.5 7-10.5 7S1.5 12 1.5 12Z" />
        <circle cx="12" cy="12" r="3" />
        {on && <line x1="3" y1="21" x2="21" y2="3" />}
      </svg>
      <span className="sr-only">{fullLabel}</span>
      <span aria-hidden="true">{on ? "Show amounts" : "Hide amounts"}</span>
    </button>
  );
}

function App() {
  const [theme, setTheme] = useState(loadTheme);
  const [tab, setTab] = useState<Tab>("overview");
  const [displayName, setDisplayName] = useState(loadDisplayName);
  // First-run state (section 30): needs no persisted flag at all - a
  // database that has never had a single account (archived ones count;
  // this is about "has anyone ever set this up," not "is anything active
  // right now") is already an unambiguous, self-clearing signal. The
  // moment onboarding's own account step succeeds, this recomputes to
  // false on its own. "Skip setup" only suppresses it for this session
  // (never persisted) so a backup-restore-first user isn't blocked from
  // reaching Settings, while a still-empty database is gently offered
  // onboarding again next launch rather than treated as a dead end.
  const [needsOnboarding, setNeedsOnboarding] = useState<boolean | null>(null);
  const [onboardingSkipped, setOnboardingSkipped] = useState(false);

  useEffect(() => {
    accountsApi
      .list(true)
      .then((accounts) => setNeedsOnboarding(accounts.length === 0))
      .catch(() => setNeedsOnboarding(false));
  }, []);

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

  if (needsOnboarding === null) {
    return <div className="app-shell" />;
  }

  if (needsOnboarding && !onboardingSkipped) {
    return (
      <OnboardingFlow
        onFinish={() => setNeedsOnboarding(false)}
        onSkip={() => setOnboardingSkipped(true)}
      />
    );
  }

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
          <PrivacyToggle />
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
          {tab === "review" && <ReviewPage onNavigate={setTab} />}
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
