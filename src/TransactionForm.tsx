import { useEffect, useRef, useState } from "react";
import type { Account, Transaction, TransactionInput } from "./api/midas";
import { transactionsApi } from "./api/midas";
import { SUGGESTED_CATEGORIES } from "./lib/categories";
import { centsToDollarInput, parseDollarsToCents } from "./lib/currency";
import {
  TRANSACTION_TYPES,
  TRANSACTION_TYPE_LABELS,
} from "./lib/transactionTypes";

interface TransactionFormProps {
  accounts: Account[];
  defaultAccountId: number | null;
  transaction: Transaction | null;
  onClose: () => void;
  onSaved: () => void;
}

function TransactionForm({
  accounts,
  defaultAccountId,
  transaction,
  onClose,
  onSaved,
}: TransactionFormProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [accountId, setAccountId] = useState<number>(
    transaction?.account_id ?? defaultAccountId ?? accounts[0]?.id ?? 0,
  );
  const [date, setDate] = useState(
    transaction?.transaction_date ?? new Date().toISOString().slice(0, 10),
  );
  const [moneyIn, setMoneyIn] = useState(
    transaction ? transaction.amount_cents >= 0 : false,
  );
  const [amountInput, setAmountInput] = useState(
    transaction
      ? centsToDollarInput(Math.abs(transaction.amount_cents))
      : "0.00",
  );
  const [transactionType, setTransactionType] = useState(
    transaction?.transaction_type ?? "purchase",
  );
  const [description, setDescription] = useState(transaction?.merchant ?? "");
  const [category, setCategory] = useState(transaction?.category ?? "");
  const [notes, setNotes] = useState(transaction?.notes ?? "");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (description.trim() === "") {
      setError("Description is required.");
      return;
    }
    if (!accountId) {
      setError("Choose an account.");
      return;
    }
    const magnitude = parseDollarsToCents(amountInput);
    if (magnitude === null || magnitude < 0) {
      setError("Amount must be a positive number, e.g. 42.50.");
      return;
    }

    const input: TransactionInput = {
      account_id: accountId,
      transaction_date: date,
      posted_date: null,
      amount_cents: moneyIn ? magnitude : -magnitude,
      transaction_type: transactionType,
      // Preserve the original raw CSV text on edits (traceability); a new
      // manual entry has no separate raw text yet, so it starts equal to
      // what the user typed - the backend does the same when merchant is
      // omitted on create.
      raw_description: transaction?.raw_description ?? description.trim(),
      merchant: description.trim(),
      category: category.trim() === "" ? null : category.trim(),
      subcategory: null,
      notes: notes.trim() === "" ? null : notes.trim(),
      tags: null,
      is_pending: null,
    };

    setSaving(true);
    setError(null);
    try {
      if (transaction) {
        await transactionsApi.update(transaction.id, input);
      } else {
        await transactionsApi.create(input);
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
      aria-labelledby="transaction-form-title"
      onClose={onClose}
    >
      <form method="dialog" onSubmit={handleSubmit}>
        <h2 id="transaction-form-title">
          {transaction ? "Edit transaction" : "Add transaction"}
        </h2>

        <div className="field">
          <label htmlFor="txn-account">Account</label>
          <select
            id="txn-account"
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

        <div className="field">
          <label htmlFor="txn-date">Date</label>
          <input
            id="txn-date"
            type="date"
            value={date}
            onChange={(e) => setDate(e.target.value)}
            required
          />
        </div>

        <div className="field field-checkbox amount-direction">
          <span className="field-legend">Direction</span>
          <label>
            <input
              type="radio"
              name="direction"
              checked={!moneyIn}
              onChange={() => setMoneyIn(false)}
            />
            Money out (spending, transfer out)
          </label>
          <label>
            <input
              type="radio"
              name="direction"
              checked={moneyIn}
              onChange={() => setMoneyIn(true)}
            />
            Money in (income, refund, transfer in)
          </label>
        </div>

        <div className="field">
          <label htmlFor="txn-amount">Amount (USD)</label>
          <input
            id="txn-amount"
            inputMode="decimal"
            value={amountInput}
            onChange={(e) => setAmountInput(e.target.value)}
          />
        </div>

        <div className="field">
          <label htmlFor="txn-type">Type</label>
          <select
            id="txn-type"
            value={transactionType}
            onChange={(e) =>
              setTransactionType(
                e.target.value as Transaction["transaction_type"],
              )
            }
          >
            {TRANSACTION_TYPES.map((t) => (
              <option key={t} value={t}>
                {TRANSACTION_TYPE_LABELS[t]}
              </option>
            ))}
          </select>
        </div>

        <div className="field">
          <label htmlFor="txn-description">Merchant / description</label>
          <input
            id="txn-description"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            required
            autoFocus
          />
          {transaction &&
            transaction.merchant !== transaction.raw_description && (
              <p className="field-hint">
                Originally imported as: "{transaction.raw_description}"
              </p>
            )}
        </div>

        <div className="field">
          <label htmlFor="txn-category">Category</label>
          <input
            id="txn-category"
            list="txn-category-options"
            value={category}
            onChange={(e) => setCategory(e.target.value)}
            placeholder="Optional"
          />
          <datalist id="txn-category-options">
            {SUGGESTED_CATEGORIES.map((c) => (
              <option key={c} value={c} />
            ))}
          </datalist>
        </div>

        <div className="field">
          <label htmlFor="txn-notes">Notes</label>
          <textarea
            id="txn-notes"
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

export default TransactionForm;
