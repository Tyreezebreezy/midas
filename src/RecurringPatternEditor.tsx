import { useEffect, useRef, useState } from "react";
import type {
  Account,
  Cadence,
  Payee,
  PatternEditInput,
  RecurringDirection,
  RecurringKind,
  RecurringSeries,
} from "./api/midas";
import { payeesApi, recurringApi } from "./api/midas";
import {
  LIABILITY_TYPES,
  amountIsMoneyIn,
  signedAmountCents,
  type AccountType,
} from "./lib/accountTypes";
import { SUGGESTED_CATEGORIES } from "./lib/categories";
import { centsToDollarInput, parseDollarsToCents } from "./lib/currency";

export const CADENCE_LABELS: Record<Cadence, string> = {
  weekly: "Weekly",
  biweekly: "Every 2 weeks",
  monthly: "Monthly",
  quarterly: "Quarterly",
  annual: "Annually",
  semimonthly: "Twice a month",
};

const KIND_LABELS: Record<RecurringKind, string> = {
  subscription: "Subscription",
  bill: "Bill",
  membership: "Membership",
  recurring_income: "Recurring income",
  other: "Other",
};

const KIND_OPTIONS: RecurringKind[] = [
  "subscription",
  "bill",
  "membership",
  "recurring_income",
  "other",
];

const CADENCE_OPTIONS: Cadence[] = [
  "weekly",
  "biweekly",
  "monthly",
  "quarterly",
  "annual",
  "semimonthly",
];

function MoveAccountDialog({
  series,
  accounts,
  onClose,
  onMoved,
}: {
  series: RecurringSeries;
  accounts: Account[];
  onClose: () => void;
  onMoved: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [accountId, setAccountId] = useState(series.account_id);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  async function handleConfirm() {
    setSaving(true);
    setError(null);
    try {
      await recurringApi.moveAccount(series.id, accountId);
      onMoved();
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <dialog
      ref={dialogRef}
      className="account-dialog"
      aria-labelledby="move-account-title"
      onClose={onClose}
    >
      <h2 id="move-account-title">Move to a different account</h2>
      <p className="muted">
        Future expected occurrences will belong to the new account. Past
        transactions already linked to this pattern are not moved.
      </p>
      <div className="field">
        <label htmlFor="move-account-select">Account</label>
        <select
          id="move-account-select"
          value={accountId}
          onChange={(e) => setAccountId(Number(e.target.value))}
        >
          {accounts.map((a) => (
            <option key={a.id} value={a.id}>
              {a.name}
            </option>
          ))}
        </select>
      </div>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      <div className="dialog-actions">
        <button
          type="button"
          className="btn-secondary"
          onClick={onClose}
          disabled={saving}
        >
          Cancel
        </button>
        <button
          type="button"
          className="btn-primary"
          onClick={handleConfirm}
          disabled={saving || accountId === series.account_id}
        >
          {saving ? "Moving…" : "Move"}
        </button>
      </div>
    </dialog>
  );
}

interface RecurringPatternEditorProps {
  accounts: Account[];
  series: RecurringSeries | "new";
  defaultAccountId: number | null;
  onClose: () => void;
  onSaved: () => void;
}

function RecurringPatternEditor({
  accounts,
  series,
  defaultAccountId,
  onClose,
  onSaved,
}: RecurringPatternEditorProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const existing = series === "new" ? null : series;
  const isNew = existing === null;

  const [accountId, setAccountId] = useState(
    existing?.account_id ?? defaultAccountId ?? accounts[0]?.id ?? 0,
  );
  // Recomputed every render, never stored, so switching the account while
  // creating a new pattern immediately re-labels the direction options
  // (ADR-0004: liability accounts invert what "money in/out" means) -
  // mirrors TransactionForm's own account-aware direction handling.
  const selectedAccountType: AccountType =
    accounts.find((a) => a.id === accountId)?.account_type ?? "checking";

  const [direction, setDirection] = useState<RecurringDirection>(
    existing?.direction ?? "expense",
  );
  const [merchant, setMerchant] = useState(existing?.merchant ?? "");
  const [payeeId, setPayeeId] = useState<number | "">(existing?.payee_id ?? "");
  const [kind, setKind] = useState<RecurringKind>(existing?.kind ?? "other");
  const [category, setCategory] = useState(existing?.category ?? "");
  const [cadence, setCadence] = useState<Cadence>(
    existing?.cadence ?? "monthly",
  );
  const [semiDay1, setSemiDay1] = useState(existing?.semimonthly_day_1 ?? 1);
  const [semiDay2, setSemiDay2] = useState(existing?.semimonthly_day_2 ?? 15);
  const [moneyIn, setMoneyIn] = useState(
    existing
      ? amountIsMoneyIn(existing.expected_amount_cents, selectedAccountType)
      : false,
  );
  const [amountInput, setAmountInput] = useState(
    existing
      ? centsToDollarInput(Math.abs(existing.expected_amount_cents))
      : "0.00",
  );
  const [nextDate, setNextDate] = useState(
    existing?.next_expected_date ?? new Date().toISOString().slice(0, 10),
  );
  const [notes, setNotes] = useState(existing?.notes ?? "");
  const [payees, setPayees] = useState<Payee[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [busyAction, setBusyAction] = useState(false);
  const [movingAccount, setMovingAccount] = useState(false);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  useEffect(() => {
    payeesApi
      .list(false)
      .then(setPayees)
      .catch(() => {
        // The payee picker is a convenience; the form still works without it.
      });
  }, []);

  // For an existing pattern, direction was fixed at creation (changing it
  // would silently rewrite what "this amount" has meant historically) - the
  // radio only appears for a brand-new pattern; edits re-sign the same
  // direction the pattern already has.
  const effectiveDirection = isNew ? direction : existing.direction;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (merchant.trim() === "") {
      setError("Name is required.");
      return;
    }
    const magnitude = parseDollarsToCents(amountInput);
    if (magnitude === null || magnitude < 0) {
      setError("Amount must be a positive number, e.g. 9.99.");
      return;
    }
    if (cadence === "semimonthly") {
      if (
        semiDay1 < 1 ||
        semiDay1 > 31 ||
        semiDay2 < 1 ||
        semiDay2 > 31 ||
        semiDay1 === semiDay2
      ) {
        setError(
          "Semi-monthly needs two different days of the month, each 1-31.",
        );
        return;
      }
    }
    if (!nextDate) {
      setError("Next expected date is required.");
      return;
    }

    const input: PatternEditInput = {
      merchant: merchant.trim(),
      payee_id: payeeId === "" ? null : payeeId,
      kind,
      category: category.trim() === "" ? null : category.trim(),
      cadence,
      semimonthly_day_1: cadence === "semimonthly" ? semiDay1 : null,
      semimonthly_day_2: cadence === "semimonthly" ? semiDay2 : null,
      expected_amount_cents: signedAmountCents(
        magnitude,
        isNew ? moneyIn : effectiveDirection === "income",
        selectedAccountType,
      ),
      next_expected_date: nextDate,
      notes: notes.trim() === "" ? null : notes.trim(),
    };

    setSaving(true);
    setError(null);
    try {
      if (existing) {
        await recurringApi.updatePattern(existing.id, input);
      } else {
        await recurringApi.createManual(accountId, direction, input);
      }
      onSaved();
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  async function runLifecycleAction(action: () => Promise<unknown>) {
    setBusyAction(true);
    setError(null);
    try {
      await action();
      onSaved();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusyAction(false);
    }
  }

  return (
    <>
      <dialog
        ref={dialogRef}
        className="account-dialog"
        aria-labelledby="pattern-form-title"
        onClose={onClose}
      >
        <form method="dialog" onSubmit={handleSubmit}>
          <h2 id="pattern-form-title">
            {existing ? "Edit recurring pattern" : "Add recurring pattern"}
          </h2>

          <div className="field">
            <label htmlFor="pattern-account">Account</label>
            {existing ? (
              <p>
                {accounts.find((a) => a.id === existing.account_id)?.name ??
                  "—"}
              </p>
            ) : (
              <select
                id="pattern-account"
                value={accountId}
                onChange={(e) => setAccountId(Number(e.target.value))}
              >
                {accounts.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.name}
                  </option>
                ))}
              </select>
            )}
          </div>

          {isNew && (
            <fieldset className="field field-checkbox amount-direction">
              <legend className="field-legend">Direction</legend>
              <label>
                <input
                  type="radio"
                  name="pattern-direction"
                  checked={!moneyIn}
                  onChange={() => {
                    setMoneyIn(false);
                    setDirection("expense");
                  }}
                />
                {LIABILITY_TYPES.has(selectedAccountType)
                  ? "Charge (increases what you owe)"
                  : "Expense (money out)"}
              </label>
              <label>
                <input
                  type="radio"
                  name="pattern-direction"
                  checked={moneyIn}
                  onChange={() => {
                    setMoneyIn(true);
                    setDirection("income");
                  }}
                />
                {LIABILITY_TYPES.has(selectedAccountType)
                  ? "Payment or credit (decreases what you owe)"
                  : "Income (money in)"}
              </label>
            </fieldset>
          )}

          <div className="field">
            <label htmlFor="pattern-amount">
              {effectiveDirection === "income"
                ? "Expected amount received (USD)"
                : "Expected amount (USD)"}
            </label>
            <input
              id="pattern-amount"
              inputMode="decimal"
              value={amountInput}
              onChange={(e) => setAmountInput(e.target.value)}
            />
          </div>

          <div className="field">
            <label htmlFor="pattern-cadence">Frequency</label>
            <select
              id="pattern-cadence"
              value={cadence}
              onChange={(e) => setCadence(e.target.value as Cadence)}
            >
              {CADENCE_OPTIONS.map((c) => (
                <option key={c} value={c}>
                  {CADENCE_LABELS[c]}
                </option>
              ))}
            </select>
          </div>

          {cadence === "semimonthly" && (
            <div className="field mapping-row">
              <label htmlFor="pattern-semi-1">First day of month</label>
              <input
                id="pattern-semi-1"
                type="number"
                min={1}
                max={31}
                value={semiDay1}
                onChange={(e) => setSemiDay1(Number(e.target.value))}
              />
              <label htmlFor="pattern-semi-2">Second day of month</label>
              <input
                id="pattern-semi-2"
                type="number"
                min={1}
                max={31}
                value={semiDay2}
                onChange={(e) => setSemiDay2(Number(e.target.value))}
              />
            </div>
          )}

          <div className="field">
            <label htmlFor="pattern-next-date">Next expected date</label>
            <input
              id="pattern-next-date"
              type="date"
              value={nextDate}
              onChange={(e) => setNextDate(e.target.value)}
              required
            />
          </div>

          <div className="field">
            <label htmlFor="pattern-merchant">Name</label>
            <input
              id="pattern-merchant"
              value={merchant}
              onChange={(e) => setMerchant(e.target.value)}
              required
              autoFocus
            />
          </div>

          <div className="field">
            <label htmlFor="pattern-payee">Payee</label>
            <select
              id="pattern-payee"
              value={payeeId}
              onChange={(e) =>
                setPayeeId(e.target.value === "" ? "" : Number(e.target.value))
              }
            >
              <option value="">No payee</option>
              {payees.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          </div>

          <div className="field">
            <label htmlFor="pattern-kind">Kind</label>
            <select
              id="pattern-kind"
              value={kind}
              onChange={(e) => setKind(e.target.value as RecurringKind)}
            >
              {KIND_OPTIONS.map((k) => (
                <option key={k} value={k}>
                  {KIND_LABELS[k]}
                </option>
              ))}
            </select>
          </div>

          <div className="field">
            <label htmlFor="pattern-category">Category</label>
            <input
              id="pattern-category"
              list="pattern-category-options"
              value={category}
              onChange={(e) => setCategory(e.target.value)}
              placeholder="Optional"
            />
            <datalist id="pattern-category-options">
              {SUGGESTED_CATEGORIES.map((c) => (
                <option key={c} value={c} />
              ))}
            </datalist>
          </div>

          <div className="field">
            <label htmlFor="pattern-notes">Notes</label>
            <textarea
              id="pattern-notes"
              value={notes}
              onChange={(e) => setNotes(e.target.value)}
              rows={2}
            />
          </div>

          {existing && (
            <div className="field pattern-lifecycle">
              <span className="field-legend">Status</span>
              <p className="muted">
                {existing.status === "inactive"
                  ? "Inactive - no longer generating upcoming occurrences."
                  : existing.planning_to_cancel
                    ? "Confirmed - planning to cancel."
                    : "Confirmed - active."}
              </p>
              <div className="review-item-actions">
                {existing.status === "confirmed" && (
                  <button
                    type="button"
                    className="btn-link"
                    disabled={busyAction}
                    onClick={() =>
                      runLifecycleAction(() =>
                        recurringApi.setInactive(existing.id),
                      )
                    }
                  >
                    Set inactive
                  </button>
                )}
                {existing.status === "inactive" && (
                  <button
                    type="button"
                    className="btn-link"
                    disabled={busyAction}
                    onClick={() =>
                      runLifecycleAction(() =>
                        recurringApi.reactivate(existing.id),
                      )
                    }
                  >
                    Reactivate
                  </button>
                )}
                {existing.status === "confirmed" &&
                  (existing.kind === "subscription" ||
                    existing.kind === "membership") && (
                    <button
                      type="button"
                      className="btn-link"
                      disabled={busyAction}
                      onClick={() =>
                        runLifecycleAction(() =>
                          recurringApi.setPlanningToCancel(
                            existing.id,
                            !existing.planning_to_cancel,
                          ),
                        )
                      }
                    >
                      {existing.planning_to_cancel
                        ? "No longer planning to cancel"
                        : "Mark planning to cancel"}
                    </button>
                  )}
                <button
                  type="button"
                  className="btn-link"
                  disabled={busyAction}
                  onClick={() => setMovingAccount(true)}
                >
                  Move to a different account
                </button>
              </div>
            </div>
          )}

          {error && (
            <p className="form-error" role="alert">
              {error}
            </p>
          )}

          <div className="dialog-actions">
            <button
              type="button"
              className="btn-secondary"
              onClick={onClose}
              disabled={saving}
            >
              Cancel
            </button>
            <button type="submit" className="btn-primary" disabled={saving}>
              {saving ? "Saving…" : "Save"}
            </button>
          </div>
        </form>
      </dialog>

      {movingAccount && existing && (
        <MoveAccountDialog
          series={existing}
          accounts={accounts}
          onClose={() => setMovingAccount(false)}
          onMoved={() => {
            setMovingAccount(false);
            onSaved();
          }}
        />
      )}
    </>
  );
}

export default RecurringPatternEditor;
