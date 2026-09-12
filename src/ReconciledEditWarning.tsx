import { useEffect, useRef, useState } from "react";
import type { Transaction } from "./api/midas";
import { reconciliationApi } from "./api/midas";
import { formatDate } from "./lib/dateFormat";

interface ReconciledEditWarningProps {
  transaction: Transaction;
  /** What the confirmed action actually does, e.g. "Archiving" or "Deleting" - defaults to "Changing" (a plain edit). */
  actionLabel?: string;
  onCancel: () => void;
  onConfirm: () => void;
}

/**
 * Product handoff section 15: reconciled does NOT mean immutable, but
 * changing a transaction that belongs to a verified statement must show a
 * clear warning first - the backend (reconciliation.rs) always detects and
 * marks the affected reconciliation Needs Review regardless of whether the
 * owner sees this warning, so this dialog is a courtesy that prevents a
 * surprising after-the-fact status change, not the actual safety mechanism.
 */
function ReconciledEditWarning({
  transaction,
  actionLabel = "Changing",
  onCancel,
  onConfirm,
}: ReconciledEditWarningProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [periodLabel, setPeriodLabel] = useState<string | null>(null);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  useEffect(() => {
    if (transaction.reconciliation_id === null) return;
    reconciliationApi
      .list(transaction.account_id)
      .then((all) => {
        const match = all.find((r) => r.id === transaction.reconciliation_id);
        if (match) {
          setPeriodLabel(
            `${formatDate(match.statement_start_date)} - ${formatDate(match.statement_end_date)}`,
          );
        }
      })
      .catch(() => {
        // The warning still makes sense without the exact period label.
      });
  }, [transaction.account_id, transaction.reconciliation_id]);

  return (
    <dialog
      ref={dialogRef}
      className="account-dialog"
      aria-labelledby="reconciled-warning-title"
      onClose={onCancel}
    >
      <h2 id="reconciled-warning-title">
        This transaction belongs to a reconciled statement
      </h2>
      <p className="muted">
        {actionLabel} it may invalidate the verified balance
        {periodLabel ? ` for ${periodLabel}` : ""}.
      </p>
      <div className="dialog-actions">
        <button type="button" className="btn-secondary" onClick={onCancel}>
          Cancel
        </button>
        <button type="button" className="btn-primary" onClick={onConfirm}>
          Edit Anyway
        </button>
      </div>
    </dialog>
  );
}

export default ReconciledEditWarning;
