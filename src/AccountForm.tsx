import { useEffect, useRef, useState } from "react";
import type { Account, AccountInput } from "./api/midas";
import { accountsApi } from "./api/midas";
import {
  ACCOUNT_TYPES,
  ACCOUNT_TYPE_LABELS,
  LIABILITY_TYPES,
  type AccountType,
} from "./lib/accountTypes";
import { centsToDollarInput, parseDollarsToCents } from "./lib/currency";

interface AccountFormProps {
  account: Account | null;
  onClose: () => void;
  onSaved: () => void;
}

function AccountForm({ account, onClose, onSaved }: AccountFormProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [name, setName] = useState(account?.name ?? "");
  const [institution, setInstitution] = useState(account?.institution ?? "");
  const [accountType, setAccountType] = useState<AccountType>(
    account?.account_type ?? "checking",
  );
  const [balanceInput, setBalanceInput] = useState(
    account ? centsToDollarInput(account.balance_cents) : "0.00",
  );
  const [includeInNetWorth, setIncludeInNetWorth] = useState(
    account?.include_in_net_worth ?? true,
  );
  const [notes, setNotes] = useState(account?.notes ?? "");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (name.trim() === "") {
      setError("Account name is required.");
      return;
    }
    const cents = parseDollarsToCents(balanceInput);
    if (cents === null) {
      setError("Balance must be a number, e.g. 1234.56.");
      return;
    }

    const input: AccountInput = {
      name: name.trim(),
      institution: institution.trim() === "" ? null : institution.trim(),
      account_type: accountType,
      balance_cents: cents,
      include_in_net_worth: includeInNetWorth,
      notes: notes.trim() === "" ? null : notes.trim(),
    };

    setSaving(true);
    setError(null);
    try {
      if (account) {
        await accountsApi.update(account.id, input);
      } else {
        await accountsApi.create(input);
      }
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
      aria-labelledby="account-form-title"
      onClose={onClose}
    >
      <form method="dialog" onSubmit={handleSubmit}>
        <h2 id="account-form-title">
          {account ? "Edit account" : "Add account"}
        </h2>

        <div className="field">
          <label htmlFor="account-name">Name</label>
          <input
            id="account-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            autoFocus
          />
        </div>

        <div className="field">
          <label htmlFor="account-institution">Institution</label>
          <input
            id="account-institution"
            value={institution}
            onChange={(e) => setInstitution(e.target.value)}
            placeholder="Optional"
          />
        </div>

        <div className="field">
          <label htmlFor="account-type">Account type</label>
          <select
            id="account-type"
            value={accountType}
            onChange={(e) => setAccountType(e.target.value as AccountType)}
          >
            {ACCOUNT_TYPES.map((t) => (
              <option key={t} value={t}>
                {ACCOUNT_TYPE_LABELS[t]}
              </option>
            ))}
          </select>
        </div>

        <div className="field">
          <label htmlFor="account-balance">Current balance (USD)</label>
          <input
            id="account-balance"
            inputMode="decimal"
            value={balanceInput}
            onChange={(e) => setBalanceInput(e.target.value)}
          />
          {LIABILITY_TYPES.has(accountType) && (
            <p className="field-hint">
              Enter what you owe as a positive number, e.g. 500.00.
            </p>
          )}
        </div>

        <div className="field field-checkbox">
          <label htmlFor="account-net-worth">
            <input
              id="account-net-worth"
              type="checkbox"
              checked={includeInNetWorth}
              onChange={(e) => setIncludeInNetWorth(e.target.checked)}
            />
            Include in net worth
          </label>
        </div>

        <div className="field">
          <label htmlFor="account-notes">Notes</label>
          <textarea
            id="account-notes"
            value={notes}
            onChange={(e) => setNotes(e.target.value)}
            rows={2}
          />
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

export default AccountForm;
