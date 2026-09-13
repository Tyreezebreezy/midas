import { useEffect, useRef, useState } from "react";
import type { Split, Transaction } from "./api/midas";
import { splitsApi } from "./api/midas";
import { SUGGESTED_CATEGORIES } from "./lib/categories";
import { formatCents, parseDollarsToCents } from "./lib/currency";

interface SplitEditorProps {
  transaction: Transaction;
  onClose: () => void;
  onSaved: () => void;
}

interface SplitRow {
  key: number;
  category: string;
  amountInput: string;
}

let nextKey = 1;

function rowFromSplit(s: Split): SplitRow {
  return {
    key: nextKey++,
    category: s.category ?? "",
    amountInput: (Math.abs(s.amount_cents) / 100).toFixed(2),
  };
}

function emptyRow(): SplitRow {
  return { key: nextKey++, category: "", amountInput: "" };
}

// amountInput always holds a magnitude (never a sign) - a row loaded from an
// existing split, a freshly typed value, and "Assign remainder" must all be
// read the same way here, with the parent's own sign applied exactly once.
function magnitudeCents(amountInput: string): number {
  return Math.abs(parseDollarsToCents(amountInput) ?? 0);
}

/**
 * Splits are a reporting-layer breakdown of one ledger movement, never a
 * second set of ledger rows (product handoff section 14) - this dialog
 * only ever calls splitsApi.set, which replaces transaction_splits for
 * this transaction; it never touches the parent transaction's own amount,
 * account, or balance.
 */
function SplitEditor({ transaction, onClose, onSaved }: SplitEditorProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [rows, setRows] = useState<SplitRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  useEffect(() => {
    splitsApi
      .list(transaction.id)
      .then((existing) => {
        setRows(
          existing.length > 0
            ? existing.map(rowFromSplit)
            : [emptyRow(), emptyRow()],
        );
      })
      .catch((err) => setError(String(err)))
      .finally(() => setLoading(false));
  }, [transaction.id]);

  const parentAmount = transaction.amount_cents;
  const assignedCents = rows.reduce((sum, r) => {
    const magnitude = magnitudeCents(r.amountInput);
    return sum + (transaction.amount_cents < 0 ? -magnitude : magnitude);
  }, 0);
  const remainingCents = parentAmount - assignedCents;
  const isBalanced = remainingCents === 0;

  function updateRow(key: number, patch: Partial<SplitRow>) {
    setRows((prev) =>
      prev.map((r) => (r.key === key ? { ...r, ...patch } : r)),
    );
  }

  function removeRow(key: number) {
    setRows((prev) => prev.filter((r) => r.key !== key));
  }

  function addRow() {
    setRows((prev) => [...prev, emptyRow()]);
  }

  function assignRemainderTo(key: number) {
    const magnitude = Math.abs(remainingCents) / 100;
    updateRow(key, { amountInput: magnitude.toFixed(2) });
  }

  async function handleSave() {
    setError(null);
    if (rows.length < 2) {
      setError("A split needs at least two parts.");
      return;
    }
    if (!isBalanced) {
      setError("Splits must sum exactly to the transaction's amount.");
      return;
    }
    setSaving(true);
    try {
      await splitsApi.set(
        transaction.id,
        rows.map((r) => {
          const magnitude = magnitudeCents(r.amountInput);
          return {
            category: r.category.trim() === "" ? null : r.category.trim(),
            subcategory: null,
            amount_cents: transaction.amount_cents < 0 ? -magnitude : magnitude,
            notes: null,
          };
        }),
      );
      onSaved();
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  async function handleUnsplit() {
    setSaving(true);
    setError(null);
    try {
      await splitsApi.set(transaction.id, []);
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
      aria-labelledby="split-editor-title"
      onClose={onClose}
    >
      <h2 id="split-editor-title">
        Split {transaction.merchant} - {formatCents(parentAmount)}
      </h2>

      {transaction.reconciliation_id !== null && (
        <p className="field-hint">
          This transaction belongs to a reconciled statement. Splitting only
          affects category reporting, not the account balance, so it will not
          invalidate that reconciliation.
        </p>
      )}

      {loading ? (
        <p className="muted">Loading…</p>
      ) : (
        <>
          <div className="split-rows">
            {rows.map((row) => (
              <div key={row.key} className="split-row">
                <input
                  aria-label="Split category"
                  value={row.category}
                  onChange={(e) =>
                    updateRow(row.key, { category: e.target.value })
                  }
                  placeholder="Category"
                  list="split-category-options"
                />
                <input
                  aria-label="Split amount"
                  inputMode="decimal"
                  value={row.amountInput}
                  onChange={(e) =>
                    updateRow(row.key, { amountInput: e.target.value })
                  }
                  placeholder="0.00"
                />
                <button
                  type="button"
                  className="btn-link"
                  onClick={() => assignRemainderTo(row.key)}
                  disabled={remainingCents === 0}
                >
                  Assign remainder
                </button>
                <button
                  type="button"
                  className="btn-link"
                  onClick={() => removeRow(row.key)}
                  disabled={rows.length <= 2}
                >
                  Remove
                </button>
              </div>
            ))}
          </div>

          <datalist id="split-category-options">
            {SUGGESTED_CATEGORIES.map((c) => (
              <option key={c} value={c} />
            ))}
          </datalist>

          <button type="button" className="btn-secondary" onClick={addRow}>
            Add split
          </button>

          <p className={isBalanced ? "muted" : "form-error"} role="status">
            {isBalanced
              ? "Fully assigned."
              : `Remaining: ${formatCents(remainingCents)}`}
          </p>

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
            {transaction.has_splits && (
              <button
                type="button"
                className="btn-secondary"
                onClick={handleUnsplit}
                disabled={saving}
              >
                Remove split
              </button>
            )}
            <button
              type="button"
              className="btn-primary"
              onClick={handleSave}
              disabled={saving || !isBalanced}
            >
              {saving ? "Saving…" : "Save split"}
            </button>
          </div>
        </>
      )}
    </dialog>
  );
}

export default SplitEditor;
