import { useEffect, useRef, useState } from "react";
import type { Account } from "./api/midas";
import { accountingApi } from "./api/midas";
import { LIABILITY_TYPES } from "./lib/accountTypes";
import { formatCents, parseDollarsToCents, useMasked } from "./lib/currency";

interface ReconcileBalanceFormProps {
  account: Account;
  onClose: () => void;
  onSaved: () => void;
}

function ReconcileBalanceForm({
  account,
  onClose,
  onSaved,
}: ReconcileBalanceFormProps) {
  useMasked();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [balanceInput, setBalanceInput] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const cents = parseDollarsToCents(balanceInput);
    if (cents === null) {
      setError("Balance must be a number, e.g. 1234.56.");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await accountingApi.reconcileBalance(
        account.id,
        cents,
        new Date().toISOString().slice(0, 10),
      );
      onSaved();
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
      aria-labelledby="reconcile-form-title"
      onClose={onClose}
    >
      <form method="dialog" onSubmit={handleSubmit}>
        <h2 id="reconcile-form-title">Update {account.name}'s balance</h2>
        <p className="muted">
          MIDAS currently shows {formatCents(account.balance_cents)}. Enter what
          your bank actually shows today - MIDAS will record the difference as a
          dated adjustment, visible in the transaction list.
        </p>

        <div className="field">
          <label htmlFor="reconcile-balance">Current balance (USD)</label>
          <input
            id="reconcile-balance"
            inputMode="decimal"
            value={balanceInput}
            onChange={(e) => setBalanceInput(e.target.value)}
            autoFocus
            required
          />
          {LIABILITY_TYPES.has(account.account_type) && (
            <p className="field-hint">
              Enter what you owe as a positive number, e.g. 500.00.
            </p>
          )}
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
          <button type="submit" className="btn-primary" disabled={saving}>
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </form>
    </dialog>
  );
}

export default ReconcileBalanceForm;
