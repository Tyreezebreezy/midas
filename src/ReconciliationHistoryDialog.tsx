import { useEffect, useRef, useState } from "react";
import type { Account, Reconciliation } from "./api/midas";
import { reconciliationApi } from "./api/midas";
import { formatCents, useMasked } from "./lib/currency";
import { formatDate } from "./lib/dateFormat";

interface ReconciliationHistoryDialogProps {
  account: Account;
  onClose: () => void;
}

const STATUS_LABELS: Record<Reconciliation["status"], string> = {
  completed: "Reconciled",
  needs_review: "Needs review",
  reopened: "Reopened",
};

function ReconciliationHistoryDialog({
  account,
  onClose,
}: ReconciliationHistoryDialogProps) {
  useMasked();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [records, setRecords] = useState<Reconciliation[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [reopening, setReopening] = useState<Reconciliation | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      setRecords(await reconciliationApi.list(account.id));
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [account.id]);

  async function confirmReopen() {
    if (!reopening) return;
    setBusy(true);
    setError(null);
    try {
      await reconciliationApi.reopen(reopening.id);
      setReopening(null);
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <dialog
      ref={dialogRef}
      className="account-dialog import-history-dialog"
      aria-labelledby="reconciliation-history-title"
      onClose={onClose}
    >
      <h2 id="reconciliation-history-title">
        {account.name} - Reconciliation History
      </h2>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {loading ? (
        <p className="muted">Loading…</p>
      ) : records.length === 0 ? (
        <p className="muted">
          No statements reconciled yet. Reconcile your first statement to verify
          MIDAS against your bank.
        </p>
      ) : (
        <ul className="import-history-list">
          {records.map((r) => (
            <li key={r.id}>
              <div className="import-history-item">
                <span className="import-history-filename">
                  {formatDate(r.statement_start_date)} -{" "}
                  {formatDate(r.statement_end_date)}
                  {r.status !== "completed" && (
                    <span
                      className={
                        r.status === "needs_review"
                          ? "badge-review"
                          : "badge-archived"
                      }
                    >
                      {STATUS_LABELS[r.status]}
                    </span>
                  )}
                </span>
                <span className="muted">
                  {formatCents(r.statement_ending_balance_cents)} · Reconciled{" "}
                  {formatDate(r.reconciled_at)}
                </span>
                {r.note && <span className="muted">{r.note}</span>}

                {reopening?.id === r.id ? (
                  <>
                    <p className="import-recognition-banner" role="alert">
                      Reopen this period? Its transactions return to Cleared and
                      it no longer counts as verified - the record stays in your
                      history, marked Reopened.
                    </p>
                    <div className="dialog-actions">
                      <button
                        type="button"
                        className="btn-secondary"
                        onClick={() => setReopening(null)}
                        disabled={busy}
                      >
                        Cancel
                      </button>
                      <button
                        type="button"
                        className="btn-danger"
                        onClick={confirmReopen}
                        disabled={busy}
                      >
                        {busy ? "Reopening…" : "Reopen Reconciliation"}
                      </button>
                    </div>
                  </>
                ) : (
                  (r.status === "completed" || r.status === "needs_review") && (
                    <div className="dialog-actions">
                      <button
                        type="button"
                        className="btn-link btn-link-danger"
                        onClick={() => setReopening(r)}
                      >
                        Reopen
                      </button>
                    </div>
                  )
                )}
              </div>
            </li>
          ))}
        </ul>
      )}

      <div className="dialog-actions">
        <button type="button" className="btn-primary" onClick={onClose}>
          Close
        </button>
      </div>
    </dialog>
  );
}

export default ReconciliationHistoryDialog;
