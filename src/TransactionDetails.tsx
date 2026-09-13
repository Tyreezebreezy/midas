import { useEffect, useRef } from "react";
import type { Transaction } from "./api/midas";
import { formatDate } from "./lib/dateFormat";

interface TransactionDetailsProps {
  transaction: Transaction;
  accountName: string;
  payeeName: string | null;
  importFilename: string | null;
  onClose: () => void;
}

/**
 * A compact read-only surface for the provenance fields that don't earn a
 * column in the main table (product handoff section 40) - raw imported
 * text, external/import identifiers, and timestamps.
 */
function TransactionDetails({
  transaction,
  accountName,
  payeeName,
  importFilename,
  onClose,
}: TransactionDetailsProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  return (
    <dialog
      ref={dialogRef}
      className="account-dialog"
      aria-labelledby="txn-details-title"
      onClose={onClose}
    >
      <h2 id="txn-details-title">Transaction details</h2>
      <dl className="txn-detail-list">
        <dt>Account</dt>
        <dd>{accountName}</dd>

        <dt>Raw description</dt>
        <dd>{transaction.raw_description}</dd>

        <dt>Payee</dt>
        <dd>{payeeName ?? <span className="muted">Not set</span>}</dd>

        {transaction.notes && (
          <>
            <dt>Notes</dt>
            <dd>{transaction.notes}</dd>
          </>
        )}

        <dt>Import source</dt>
        <dd>
          {importFilename ?? <span className="muted">Entered manually</span>}
        </dd>

        {transaction.external_transaction_id && (
          <>
            <dt>External ID</dt>
            <dd>{transaction.external_transaction_id}</dd>
          </>
        )}

        <dt>Added</dt>
        <dd>{formatDate(transaction.created_at.slice(0, 10))}</dd>

        <dt>Last updated</dt>
        <dd>{formatDate(transaction.updated_at.slice(0, 10))}</dd>
      </dl>
      <div className="dialog-actions">
        <button type="button" className="btn-primary" onClick={onClose}>
          Close
        </button>
      </div>
    </dialog>
  );
}

export default TransactionDetails;
