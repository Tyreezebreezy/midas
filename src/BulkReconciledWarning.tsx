import { useEffect, useRef } from "react";

interface BulkReconciledWarningProps {
  count: number;
  actionLabel: string;
  onCancel: () => void;
  onConfirm: () => void;
}

/**
 * The bulk equivalent of ReconciledEditWarning - a selection can span many
 * different reconciled statements, so unlike the single-transaction warning
 * this never names a specific period, only how many of the selected
 * transactions are reconciled. The backend still detects and marks each
 * affected reconciliation Needs Review regardless of this dialog.
 */
function BulkReconciledWarning({
  count,
  actionLabel,
  onCancel,
  onConfirm,
}: BulkReconciledWarningProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  return (
    <dialog
      ref={dialogRef}
      className="account-dialog"
      aria-labelledby="bulk-reconciled-warning-title"
      onClose={onCancel}
    >
      <h2 id="bulk-reconciled-warning-title">
        {count} selected transaction{count === 1 ? "" : "s"} belong
        {count === 1 ? "s" : ""} to a reconciled statement
      </h2>
      <p className="muted">
        {actionLabel} them may invalidate those verified balances, marking them
        Needs Review.
      </p>
      <div className="dialog-actions">
        <button type="button" className="btn-secondary" onClick={onCancel}>
          Cancel
        </button>
        <button type="button" className="btn-primary" onClick={onConfirm}>
          Continue Anyway
        </button>
      </div>
    </dialog>
  );
}

export default BulkReconciledWarning;
