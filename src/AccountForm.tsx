import { useEffect, useRef, useState } from "react";
import type { Account, AccountInput } from "./api/midas";
import { accountingApi, accountsApi } from "./api/midas";
import {
  ACCOUNT_TYPES,
  ACCOUNT_TYPE_LABELS,
  LIABILITY_TYPES,
  type AccountType,
} from "./lib/accountTypes";
import { parseDollarsToCents } from "./lib/currency";

interface AccountFormProps {
  account: Account | null;
  onClose: () => void;
  onSaved: (savedAccount: Account) => void;
}

function AccountForm({ account, onClose, onSaved }: AccountFormProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [name, setName] = useState(account?.name ?? "");
  const [institution, setInstitution] = useState(account?.institution ?? "");
  const [accountType, setAccountType] = useState<AccountType>(
    account?.account_type ?? "checking",
  );
  const [startingBalanceInput, setStartingBalanceInput] = useState("0.00");
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
    let startingBalanceCents = 0;
    if (!account) {
      const parsed = parseDollarsToCents(startingBalanceInput);
      if (parsed === null) {
        setError("Starting balance must be a number, e.g. 1234.56.");
        return;
      }
      startingBalanceCents = parsed;
    }

    const input: AccountInput = {
      name: name.trim(),
      institution: institution.trim() === "" ? null : institution.trim(),
      account_type: accountType,
      include_in_net_worth: includeInNetWorth,
      notes: notes.trim() === "" ? null : notes.trim(),
    };

    setSaving(true);
    setError(null);
    try {
      let saved: Account;
      if (account) {
        saved = await accountsApi.update(account.id, input);
      } else {
        const created = await accountsApi.create(input);
        if (startingBalanceCents !== 0) {
          await accountingApi.reconcileBalance(
            created.id,
            startingBalanceCents,
            new Date().toISOString().slice(0, 10),
          );
        }
        saved = created;
      }
      onSaved(saved);
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
          <label htmlFor="account-name" className="field-required">
            Name
          </label>
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

        {!account && (
          <div className="field">
            <label htmlFor="account-balance">
              Starting balance (USD), as of today
            </label>
            <input
              id="account-balance"
              inputMode="decimal"
              value={startingBalanceInput}
              onChange={(e) => setStartingBalanceInput(e.target.value)}
            />
            {LIABILITY_TYPES.has(accountType) && (
              <p className="field-hint">
                Enter what you owe as a positive number, e.g. 500.00.
              </p>
            )}
          </div>
        )}

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
