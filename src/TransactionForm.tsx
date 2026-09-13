import { useEffect, useRef, useState } from "react";
import type {
  Account,
  CorrectionScope,
  Payee,
  Tag,
  Transaction,
  TransactionInput,
} from "./api/midas";
import { payeesApi, rulesApi, tagsApi, transactionsApi } from "./api/midas";
import SplitEditor from "./SplitEditor";
import {
  LIABILITY_TYPES,
  amountIsMoneyIn,
  signedAmountCents,
} from "./lib/accountTypes";
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
  // Recomputed every render (not stored state) so switching the account
  // mid-edit - e.g. checking to a credit card - never leaves the direction
  // toggle or a subsequent save using a stale account's sign semantics
  // (ADR-0004: liability accounts invert what "money in/out" means).
  const selectedAccountType =
    accounts.find((a) => a.id === accountId)?.account_type ?? "checking";
  const [date, setDate] = useState(
    transaction?.transaction_date ?? new Date().toISOString().slice(0, 10),
  );
  const [moneyIn, setMoneyIn] = useState(
    transaction
      ? amountIsMoneyIn(transaction.amount_cents, selectedAccountType)
      : false,
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
  const [payeeId, setPayeeId] = useState<number | "">(
    transaction?.payee_id ?? "",
  );
  const [isOneTime, setIsOneTime] = useState(transaction?.is_one_time ?? false);
  const [payees, setPayees] = useState<Payee[]>([]);
  const [allTags, setAllTags] = useState<Tag[]>([]);
  const [selectedTagIds, setSelectedTagIds] = useState<Set<number>>(new Set());
  const originalTagIdsRef = useRef<Set<number>>(new Set());
  const [splitting, setSplitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [correctionOffer, setCorrectionOffer] = useState<{
    merchantChanged: boolean;
    categoryChanged: boolean;
  } | null>(null);
  const [correctionMatchText, setCorrectionMatchText] = useState("");
  const [applyingCorrection, setApplyingCorrection] = useState(false);

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
    tagsApi
      .list()
      .then(setAllTags)
      .catch(() => {
        // Same for tags.
      });
    if (transaction) {
      tagsApi
        .listForTransaction(transaction.id)
        .then((ts) => {
          const ids = new Set(ts.map((t) => t.id));
          originalTagIdsRef.current = ids;
          setSelectedTagIds(ids);
        })
        .catch(() => {
          // Non-critical.
        });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function toggleTag(tagId: number) {
    setSelectedTagIds((prev) => {
      const next = new Set(prev);
      if (next.has(tagId)) next.delete(tagId);
      else next.add(tagId);
      return next;
    });
  }

  async function syncTags(transactionId: number, previousTagIds: Set<number>) {
    const toAttach = [...selectedTagIds].filter(
      (id) => !previousTagIds.has(id),
    );
    const toDetach = [...previousTagIds].filter(
      (id) => !selectedTagIds.has(id),
    );
    await Promise.all([
      ...toAttach.map((tagId) => tagsApi.attach(transactionId, tagId)),
      ...toDetach.map((tagId) => tagsApi.detach(transactionId, tagId)),
    ]);
  }

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
    const trimmedDescription = description.trim();
    const trimmedCategory = category.trim() === "" ? null : category.trim();

    const input: TransactionInput = {
      account_id: accountId,
      transaction_date: date,
      posted_date: null,
      amount_cents: signedAmountCents(magnitude, moneyIn, selectedAccountType),
      transaction_type: transactionType,
      // Preserve the original raw CSV text on edits (traceability); a new
      // manual entry has no separate raw text yet, so it starts equal to
      // what the user typed - the backend does the same when merchant is
      // omitted on create.
      raw_description: transaction?.raw_description ?? trimmedDescription,
      merchant: trimmedDescription,
      category: trimmedCategory,
      subcategory: null,
      notes: notes.trim() === "" ? null : notes.trim(),
      tags: null,
      is_pending: null,
      payee_id: payeeId === "" ? null : payeeId,
      is_one_time: isOneTime,
    };

    setSaving(true);
    setError(null);
    try {
      const previousTagIds = originalTagIdsRef.current;
      let savedId: number;
      if (transaction) {
        await transactionsApi.update(transaction.id, input);
        savedId = transaction.id;
        await syncTags(savedId, previousTagIds);
      } else {
        const created = await transactionsApi.create(input);
        savedId = created.id;
        await syncTags(savedId, new Set());
      }

      const merchantChanged =
        transaction !== null && transaction.merchant !== trimmedDescription;
      const categoryChanged =
        transaction !== null && transaction.category !== trimmedCategory;
      if (merchantChanged || categoryChanged) {
        setCorrectionMatchText(
          transaction?.raw_description ?? trimmedDescription,
        );
        setCorrectionOffer({ merchantChanged, categoryChanged });
        return;
      }
      onSaved();
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  async function handleApplyCorrection(scope: CorrectionScope) {
    if (!correctionOffer) return;
    setApplyingCorrection(true);
    setError(null);
    try {
      await rulesApi.applyCorrection(
        {
          match_type: "contains",
          match_value: correctionMatchText.trim(),
          account_id: null,
          normalized_merchant: description.trim(),
          category: category.trim() === "" ? null : category.trim(),
          subcategory: null,
          priority: 0,
        },
        scope,
      );
      onSaved();
    } catch (err) {
      setError(String(err));
    } finally {
      setApplyingCorrection(false);
    }
  }

  if (correctionOffer) {
    return (
      <dialog
        ref={dialogRef}
        className="account-dialog"
        aria-labelledby="correction-offer-title"
        onClose={onClose}
      >
        <h2 id="correction-offer-title">Reuse this correction?</h2>
        <p className="muted">
          You changed this transaction's{" "}
          {correctionOffer.merchantChanged && correctionOffer.categoryChanged
            ? "merchant and category"
            : correctionOffer.merchantChanged
              ? "merchant"
              : "category"}
          . Apply the same correction automatically to other transactions
          matching this description?
        </p>

        <div className="field">
          <label htmlFor="correction-match-text">
            When description contains
          </label>
          <input
            id="correction-match-text"
            value={correctionMatchText}
            onChange={(e) => setCorrectionMatchText(e.target.value)}
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
            onClick={onSaved}
            disabled={applyingCorrection}
          >
            This transaction only
          </button>
          <button
            type="button"
            className="btn-secondary"
            onClick={() => handleApplyCorrection("past_only")}
            disabled={applyingCorrection || correctionMatchText.trim() === ""}
          >
            Matching past transactions
          </button>
          <button
            type="button"
            className="btn-secondary"
            onClick={() => handleApplyCorrection("future_only")}
            disabled={applyingCorrection || correctionMatchText.trim() === ""}
          >
            Matching future transactions
          </button>
          <button
            type="button"
            className="btn-primary"
            onClick={() => handleApplyCorrection("past_and_future")}
            disabled={applyingCorrection || correctionMatchText.trim() === ""}
          >
            {applyingCorrection ? "Applying…" : "Past + future"}
          </button>
        </div>
      </dialog>
    );
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
            {LIABILITY_TYPES.has(selectedAccountType)
              ? "Charge or purchase (increases what you owe)"
              : "Money out (spending, transfer out)"}
          </label>
          <label>
            <input
              type="radio"
              name="direction"
              checked={moneyIn}
              onChange={() => setMoneyIn(true)}
            />
            {LIABILITY_TYPES.has(selectedAccountType)
              ? "Payment or credit (decreases what you owe)"
              : "Money in (income, refund, transfer in)"}
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
          <label htmlFor="txn-payee">Payee</label>
          <select
            id="txn-payee"
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
          <p className="field-hint">
            Manage payees (merge duplicates, archive) from Settings.
          </p>
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

        {allTags.length > 0 && (
          <fieldset className="field txn-tags-field">
            <legend className="field-legend">Tags</legend>
            {allTags.map((t) => (
              <label key={t.id} className="txn-tag-option">
                <input
                  type="checkbox"
                  checked={selectedTagIds.has(t.id)}
                  onChange={() => toggleTag(t.id)}
                />
                {t.name}
              </label>
            ))}
          </fieldset>
        )}

        <div className="field field-checkbox">
          <label htmlFor="txn-one-time">
            <input
              id="txn-one-time"
              type="checkbox"
              checked={isOneTime}
              onChange={(e) => setIsOneTime(e.target.checked)}
            />
            One-time expense
          </label>
          <p className="field-hint">
            Legitimate but unusual spending (a repair, a large one-off purchase)
            - still counts as real spending now, but future averages can exclude
            it so it doesn't skew your normal monthly baseline.
          </p>
        </div>

        {transaction && (
          <div className="field">
            <button
              type="button"
              className="btn-link"
              onClick={() => setSplitting(true)}
            >
              {transaction.has_splits ? "Edit split" : "Split this transaction"}
            </button>
          </div>
        )}

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

      {splitting && transaction && (
        <SplitEditor
          transaction={transaction}
          onClose={() => setSplitting(false)}
          onSaved={() => {
            setSplitting(false);
            onSaved();
          }}
        />
      )}
    </dialog>
  );
}

export default TransactionForm;
