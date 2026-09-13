import { useState } from "react";
import type { Account } from "./api/midas";
import AccountForm from "./AccountForm";
import ImportWizard from "./ImportWizard";
import TransactionForm from "./TransactionForm";

type Step = "welcome" | "privacy" | "account" | "transactions" | "done";

// First-run onboarding (Batch 10). Shown only when the database has never
// had a single account (see App.tsx's isFirstRun) - not for "all accounts
// archived" or "no transactions this month," which are normal states for
// an experienced user, not a fresh install (section 30). No bank
// credentials, no cloud signup, no fabricated demo data - every step
// reuses the real account/transaction/import forms, never a second
// implementation of any of them.
function OnboardingFlow({
  onFinish,
  onSkip,
}: {
  onFinish: () => void;
  onSkip: () => void;
}) {
  const [step, setStep] = useState<Step>("welcome");
  const [account, setAccount] = useState<Account | null>(null);
  const [showAccountForm, setShowAccountForm] = useState(false);
  const [showTransactionForm, setShowTransactionForm] = useState(false);
  const [showImport, setShowImport] = useState(false);

  return (
    <div className="onboarding-shell">
      <div
        className="onboarding-card"
        role="region"
        aria-label="Get started with MIDAS"
      >
        {step === "welcome" && (
          <>
            <h1>Welcome to MIDAS</h1>
            <p>Your personal finances, kept on your computer.</p>
            <p className="muted">
              Track accounts, spending, budgets, recurring expenses, net worth,
              and upcoming cash flow without giving MIDAS your banking password.
            </p>
            <div className="onboarding-actions">
              <button type="button" className="btn-link" onClick={onSkip}>
                Skip setup
              </button>
              <button
                type="button"
                className="btn-primary"
                autoFocus
                onClick={() => setStep("privacy")}
              >
                Get Started
              </button>
            </div>
          </>
        )}

        {step === "privacy" && (
          <>
            <h1>Your data stays local</h1>
            <p>MIDAS stores your financial information on this computer.</p>
            <ul className="onboarding-list">
              <li>No bank credentials.</li>
              <li>No cloud account required.</li>
              <li>You control backups (Settings → Backup &amp; Restore).</li>
            </ul>
            <div className="onboarding-actions">
              <button
                type="button"
                className="btn-secondary"
                onClick={() => setStep("welcome")}
              >
                Back
              </button>
              <button
                type="button"
                className="btn-primary"
                autoFocus
                onClick={() => setStep("account")}
              >
                Continue
              </button>
            </div>
          </>
        )}

        {step === "account" && (
          <>
            <h1>Add your first account</h1>
            <p className="muted">
              Checking, savings, credit card, cash - whatever you track day to
              day. You can add more later from Accounts.
            </p>
            <p className="muted">
              For the starting balance, enter what this account contained when
              you want MIDAS history to begin - you don't need to enter anything
              from before that date.
            </p>
            <div className="onboarding-actions">
              <button
                type="button"
                className="btn-secondary"
                onClick={() => setStep("privacy")}
              >
                Back
              </button>
              <button
                type="button"
                className="btn-primary"
                autoFocus
                onClick={() => setShowAccountForm(true)}
              >
                Add account
              </button>
            </div>
            {showAccountForm && (
              <AccountForm
                account={null}
                onClose={() => setShowAccountForm(false)}
                onSaved={(created) => {
                  setShowAccountForm(false);
                  setAccount(created);
                  setStep("transactions");
                }}
              />
            )}
          </>
        )}

        {step === "transactions" && (
          <>
            <h1>Add your first transactions</h1>
            <p className="muted">
              Import a statement, add one manually, or skip this for now - MIDAS
              works either way.
            </p>
            <div className="onboarding-actions">
              <button
                type="button"
                className="btn-link"
                onClick={() => setStep("done")}
              >
                Skip for now
              </button>
              <button
                type="button"
                className="btn-secondary"
                onClick={() => setShowTransactionForm(true)}
              >
                Add manually
              </button>
              <button
                type="button"
                className="btn-primary"
                autoFocus
                onClick={() => setShowImport(true)}
              >
                Import (CSV/OFX/QFX)
              </button>
            </div>
            {showTransactionForm && account && (
              <TransactionForm
                accounts={[account]}
                defaultAccountId={account.id}
                transaction={null}
                onClose={() => setShowTransactionForm(false)}
                onSaved={() => {
                  setShowTransactionForm(false);
                  setStep("done");
                }}
              />
            )}
            {showImport && account && (
              <ImportWizard
                accounts={[account]}
                onClose={() => setShowImport(false)}
                onImported={() => {
                  setShowImport(false);
                  setStep("done");
                }}
              />
            )}
          </>
        )}

        {step === "done" && (
          <>
            <h1>You're ready</h1>
            <p>
              MIDAS will keep building your picture as you add more accounts and
              transactions.
            </p>
            <div className="onboarding-actions">
              <button
                type="button"
                className="btn-primary"
                autoFocus
                onClick={onFinish}
              >
                Go to Overview
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

export default OnboardingFlow;
