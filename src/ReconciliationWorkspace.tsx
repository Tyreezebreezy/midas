import { useEffect, useRef, useState } from "react";
import type {
  Account,
  ReconciliationPreview,
  StatementHint,
  Transaction,
} from "./api/midas";
import { reconciliationApi, transactionsApi } from "./api/midas";
import TransactionForm from "./TransactionForm";
import { LIABILITY_TYPES } from "./lib/accountTypes";
import { formatCents, parseDollarsToCents } from "./lib/currency";
import { formatDate } from "./lib/dateFormat";

interface ReconciliationWorkspaceProps {
  account: Account;
  onClose: () => void;
  onCompleted: () => void;
}

function todayIso(): string {
  return new Date().toISOString().slice(0, 10);
}

function firstOfMonthIso(): string {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-01`;
}

function ReconciliationWorkspace({
  account,
  onClose,
  onCompleted,
}: ReconciliationWorkspaceProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [startDate, setStartDate] = useState(firstOfMonthIso());
  const [endDate, setEndDate] = useState(todayIso());
  const [balanceInput, setBalanceInput] = useState("");
  const [note, setNote] = useState("");
  const [hint, setHint] = useState<StatementHint | null>(null);
  const [hintDismissed, setHintDismissed] = useState(false);
  const [preview, setPreview] = useState<ReconciliationPreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<Transaction | "new" | null>(null);
  const [completed, setCompleted] = useState(false);

  const isLiability = LIABILITY_TYPES.has(account.account_type);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  useEffect(() => {
    reconciliationApi
      .getStatementHint(account.id)
      .then(setHint)
      .catch(() => {
        // The hint is a convenience prefill, not required to reconcile.
      });
  }, [account.id]);

  async function refreshPreview() {
    const cents = parseDollarsToCents(balanceInput);
    if (cents === null) {
      setPreview(null);
      return;
    }
    setError(null);
    try {
      setPreview(
        await reconciliationApi.preview(account.id, startDate, endDate, cents),
      );
    } catch (err) {
      setPreview(null);
      setError(String(err));
    }
  }

  useEffect(() => {
    refreshPreview();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [account.id, startDate, endDate, balanceInput]);

  function useStatementHint() {
    if (!hint) return;
    if (hint.statement_start_date) setStartDate(hint.statement_start_date);
    if (hint.statement_end_date) setEndDate(hint.statement_end_date);
    setBalanceInput(
      (hint.statement_ending_balance_cents / 100)
        .toFixed(2)
        .replace("-0.00", "0.00"),
    );
    setHintDismissed(true);
  }

  async function toggleCleared(t: Transaction) {
    setError(null);
    try {
      await transactionsApi.setReconciliationStatus(
        t.id,
        t.reconciliation_status === "cleared" ? "unreconciled" : "cleared",
      );
      await refreshPreview();
    } catch (err) {
      setError(String(err));
    }
  }

  async function handleComplete() {
    if (!preview || preview.difference_cents !== 0) return;
    setBusy(true);
    setError(null);
    try {
      await reconciliationApi.complete(
        account.id,
        startDate,
        endDate,
        preview.statement_ending_balance_cents,
        note.trim() === "" ? null : note.trim(),
        hint && hintDismissed ? hint.source_import_id : null,
      );
      setCompleted(true);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  const differenceLabel = preview
    ? formatCents(Math.abs(preview.difference_cents))
    : null;
  const isBalanced = preview?.difference_cents === 0;

  return (
    <dialog
      ref={dialogRef}
      className="account-dialog import-dialog"
      aria-labelledby="reconcile-workspace-title"
      onClose={onClose}
    >
      <h2 id="reconcile-workspace-title">Reconcile {account.name}</h2>

      {completed ? (
        <div className="import-step">
          <p className="import-result-headline">
            {formatDate(startDate)} - {formatDate(endDate)} reconciled.
          </p>
          <p className="muted">
            The verified balance is now part of {account.name}'s reconciliation
            history.
          </p>
          <div className="dialog-actions">
            <button type="button" className="btn-primary" onClick={onCompleted}>
              Done
            </button>
          </div>
        </div>
      ) : (
        <div className="import-step">
          {hint && !hintDismissed && (
            <p className="import-recognition-banner">
              Statement information found in{" "}
              <strong>{hint.source_filename}</strong>
              {hint.statement_start_date && hint.statement_end_date && (
                <>
                  {" "}
                  (period {formatDate(hint.statement_start_date)} -{" "}
                  {formatDate(hint.statement_end_date)})
                </>
              )}
              , ending balance{" "}
              {formatCents(hint.statement_ending_balance_cents)}.{" "}
              <button
                type="button"
                className="btn-link"
                onClick={useStatementHint}
              >
                Use for reconciliation
              </button>{" "}
              <button
                type="button"
                className="btn-link"
                onClick={() => setHintDismissed(true)}
              >
                Dismiss
              </button>
            </p>
          )}

          <div className="reconcile-period-row">
            <div className="field">
              <label htmlFor="reconcile-start">Statement start</label>
              <input
                id="reconcile-start"
                type="date"
                value={startDate}
                onChange={(e) => setStartDate(e.target.value)}
              />
            </div>
            <div className="field">
              <label htmlFor="reconcile-end">Statement end</label>
              <input
                id="reconcile-end"
                type="date"
                value={endDate}
                onChange={(e) => setEndDate(e.target.value)}
              />
            </div>
            <div className="field">
              <label htmlFor="reconcile-balance">
                Statement ending balance
              </label>
              <input
                id="reconcile-balance"
                inputMode="decimal"
                placeholder="0.00"
                value={balanceInput}
                onChange={(e) => setBalanceInput(e.target.value)}
              />
              {isLiability && (
                <p className="field-hint">
                  Enter what you owe as a positive number, e.g. 1842.73.
                </p>
              )}
            </div>
          </div>

          {error && (
            <p className="form-error" role="alert">
              {error}
            </p>
          )}

          {preview && (
            <>
              <div
                className={
                  isBalanced
                    ? "reconcile-difference reconcile-difference-zero"
                    : "reconcile-difference"
                }
              >
                <span className="reconcile-difference-label">Difference</span>
                <span className="reconcile-difference-amount">
                  {isBalanced ? "$0.00" : differenceLabel}
                </span>
                {isBalanced ? (
                  <span className="reconcile-difference-status">
                    Ready to reconcile
                  </span>
                ) : (
                  <span className="reconcile-difference-status">
                    MIDAS is {differenceLabel}{" "}
                    {preview.difference_cents > 0 ? "below" : "above"} the
                    statement.
                  </span>
                )}
              </div>
              <ul className="import-summary-counts">
                <li>
                  Statement{" "}
                  {formatCents(preview.statement_ending_balance_cents)}
                </li>
                <li>
                  MIDAS cleared {formatCents(preview.cleared_balance_cents)}
                </li>
              </ul>

              <div className="table-scroll import-preview-scroll">
                <table className="accounts-table">
                  <thead>
                    <tr>
                      <th scope="col">Date</th>
                      <th scope="col">Payee</th>
                      <th scope="col" className="col-balance">
                        Amount
                      </th>
                      <th scope="col" className="col-balance">
                        Running balance
                      </th>
                      <th scope="col">Cleared</th>
                      <th scope="col">
                        <span className="sr-only">Edit</span>
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {preview.rows.length === 0 && (
                      <tr>
                        <td colSpan={6} className="muted">
                          No transactions in this period yet.
                        </td>
                      </tr>
                    )}
                    {preview.rows.map((row) => (
                      <tr key={row.id}>
                        <td>{formatDate(row.transaction_date)}</td>
                        <td>{row.merchant}</td>
                        <td className="col-balance">
                          {formatCents(row.amount_cents)}
                        </td>
                        <td className="col-balance">
                          {formatCents(row.running_balance_cents)}
                        </td>
                        <td>
                          <label className="force-import-toggle">
                            <input
                              type="checkbox"
                              checked={row.reconciliation_status === "cleared"}
                              onChange={() => toggleCleared(row)}
                            />
                            Cleared
                          </label>
                        </td>
                        <td>
                          <button
                            type="button"
                            className="btn-link"
                            onClick={() => setEditing(row)}
                          >
                            Edit
                          </button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>

              <div className="field">
                <label htmlFor="reconcile-note">Note (optional)</label>
                <input
                  id="reconcile-note"
                  value={note}
                  onChange={(e) => setNote(e.target.value)}
                  placeholder="e.g. Confirmed against the August statement"
                />
              </div>
            </>
          )}

          <div className="dialog-actions">
            <button
              type="button"
              className="btn-secondary"
              onClick={() => setEditing("new")}
            >
              Add transaction
            </button>
            <button type="button" className="btn-secondary" onClick={onClose}>
              Close
            </button>
            <button
              type="button"
              className="btn-primary"
              onClick={handleComplete}
              disabled={busy || !isBalanced}
            >
              {busy ? "Reconciling…" : "Reconcile Statement"}
            </button>
          </div>
        </div>
      )}

      {editing !== null && (
        <TransactionForm
          accounts={[account]}
          defaultAccountId={account.id}
          transaction={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            refreshPreview();
          }}
        />
      )}
    </dialog>
  );
}

export default ReconciliationWorkspace;
