import { useEffect, useRef, useState } from "react";
import type { Budget, BudgetClassification, BudgetInput } from "./api/midas";
import { accountingApi, budgetsApi } from "./api/midas";
import { SUGGESTED_CATEGORIES } from "./lib/categories";
import {
  centsToDollarInput,
  formatCents,
  parseDollarsToCents,
} from "./lib/currency";

const CLASSIFICATION_LABELS: Record<BudgetClassification, string> = {
  fixed: "Fixed",
  flexible: "Flexible",
};

const STATUS_LABELS: Record<Budget["status"], string> = {
  under: "On track",
  near: "Near limit",
  over: "Over budget",
};

function currentMonthValue(): string {
  return new Date().toISOString().slice(0, 7); // YYYY-MM
}

function monthToIso(monthValue: string): string {
  return `${monthValue}-01`;
}

function isoToMonthValue(monthIso: string): string {
  return monthIso.slice(0, 7);
}

function shiftMonth(monthValue: string, delta: number): string {
  const [year, month] = monthValue.split("-").map(Number);
  const d = new Date(Date.UTC(year, month - 1 + delta, 1));
  return d.toISOString().slice(0, 7);
}

function BudgetForm({
  monthValue,
  budget,
  onClose,
  onSaved,
}: {
  monthValue: string;
  budget: Budget | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [category, setCategory] = useState(budget?.category ?? "");
  const [month, setMonth] = useState(
    budget ? isoToMonthValue(budget.month) : monthValue,
  );
  const [plannedInput, setPlannedInput] = useState(
    budget ? centsToDollarInput(budget.planned_amount_cents) : "0.00",
  );
  const [classification, setClassification] = useState<BudgetClassification>(
    budget?.classification ?? "flexible",
  );
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (category.trim() === "") {
      setError("Category is required.");
      return;
    }
    const cents = parseDollarsToCents(plannedInput);
    if (cents === null || cents < 0) {
      setError("Planned amount must be a non-negative number, e.g. 400.00.");
      return;
    }
    const input: BudgetInput = {
      category: category.trim(),
      month: monthToIso(month),
      planned_amount_cents: cents,
      classification,
    };
    setSaving(true);
    setError(null);
    try {
      if (budget) {
        await budgetsApi.update(budget.id, input);
      } else {
        await budgetsApi.create(input);
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
      aria-labelledby="budget-form-title"
      onClose={onClose}
    >
      <form method="dialog" onSubmit={handleSubmit}>
        <h2 id="budget-form-title">{budget ? "Edit budget" : "Add budget"}</h2>

        <div className="field">
          <label htmlFor="budget-category">Category</label>
          <input
            id="budget-category"
            list="budget-category-options"
            value={category}
            onChange={(e) => setCategory(e.target.value)}
            required
            autoFocus
          />
          <datalist id="budget-category-options">
            {SUGGESTED_CATEGORIES.map((c) => (
              <option key={c} value={c} />
            ))}
          </datalist>
        </div>

        <div className="field">
          <label htmlFor="budget-month">Month</label>
          <input
            id="budget-month"
            type="month"
            value={month}
            onChange={(e) => setMonth(e.target.value)}
            required
          />
        </div>

        <div className="field">
          <label htmlFor="budget-planned">Planned amount (USD)</label>
          <input
            id="budget-planned"
            inputMode="decimal"
            value={plannedInput}
            onChange={(e) => setPlannedInput(e.target.value)}
          />
        </div>

        <div className="field">
          <label htmlFor="budget-classification">Classification</label>
          <select
            id="budget-classification"
            value={classification}
            onChange={(e) =>
              setClassification(e.target.value as BudgetClassification)
            }
          >
            <option value="flexible">Flexible</option>
            <option value="fixed">Fixed</option>
          </select>
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

function monthRange(monthValue: string): { from: string; to: string } {
  const [year, month] = monthValue.split("-").map(Number);
  const lastDay = new Date(Date.UTC(year, month, 0)).getUTCDate();
  return {
    from: monthToIso(monthValue),
    to: `${monthValue}-${String(lastDay).padStart(2, "0")}`,
  };
}

function BudgetsPage() {
  const [monthValue, setMonthValue] = useState(currentMonthValue);
  const [budgets, setBudgets] = useState<Budget[]>([]);
  // actualsByMonth[category] = [last month's actual, 2 months ago actual]
  const [actualsByCategory, setActualsByCategory] = useState<
    Record<string, [number, number]>
  >({});
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<Budget | null | "new">(null);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      const current = await budgetsApi.list(monthToIso(monthValue));
      setBudgets(current);

      // Historical comparison compares actual spending, not whether a
      // budget happened to be set in a prior month too - reuses
      // accounting::total_spending (via getTotalSpending) directly.
      const priorMonths = [
        shiftMonth(monthValue, -1),
        shiftMonth(monthValue, -2),
      ];
      const entries = await Promise.all(
        current.map(async (b) => {
          const [last, twoAgo] = await Promise.all(
            priorMonths.map((m) => {
              const { from, to } = monthRange(m);
              return accountingApi.getTotalSpending({
                category: b.category,
                date_from: from,
                date_to: to,
              });
            }),
          );
          return [b.category, [last, twoAgo]] as [string, [number, number]];
        }),
      );
      setActualsByCategory(Object.fromEntries(entries));
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [monthValue]);

  async function handleDelete(id: number) {
    try {
      await budgetsApi.delete(id);
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  const hasHistory = Object.values(actualsByCategory).some(
    ([last, twoAgo]) => last !== 0 || twoAgo !== 0,
  );

  return (
    <section aria-labelledby="budgets-heading">
      <div className="accounts-toolbar">
        <h1 id="budgets-heading">Budgets</h1>
        <div className="accounts-toolbar-actions">
          <input
            type="month"
            value={monthValue}
            onChange={(e) => setMonthValue(e.target.value)}
            aria-label="Month"
          />
          <button
            type="button"
            className="btn-primary"
            onClick={() => setEditing("new")}
          >
            Add budget
          </button>
        </div>
      </div>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {loading ? (
        <p className="muted">Loading…</p>
      ) : budgets.length === 0 ? (
        <p className="muted">No budgets set for this month yet.</p>
      ) : (
        <div className="table-scroll">
          <table className="accounts-table">
            <thead>
              <tr>
                <th scope="col">Category</th>
                <th scope="col">Classification</th>
                <th scope="col" className="col-balance">
                  Planned
                </th>
                <th scope="col" className="col-balance">
                  Actual
                </th>
                <th scope="col">Status</th>
                <th scope="col">
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {budgets.map((b) => (
                <tr key={b.id}>
                  <td>{b.category}</td>
                  <td>{CLASSIFICATION_LABELS[b.classification]}</td>
                  <td className="col-balance">
                    {formatCents(b.planned_amount_cents)}
                  </td>
                  <td className="col-balance">
                    {formatCents(b.actual_amount_cents)}
                  </td>
                  <td>
                    <span
                      className={
                        b.status === "over"
                          ? "amount-owed"
                          : b.status === "near"
                            ? "badge-review"
                            : "badge-valid"
                      }
                    >
                      {STATUS_LABELS[b.status]}
                    </span>
                  </td>
                  <td className="col-actions">
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => setEditing(b)}
                    >
                      Edit
                    </button>
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => handleDelete(b.id)}
                    >
                      Delete
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {!loading && hasHistory && (
        <>
          <h2 className="section-heading">Recent months</h2>
          <div className="table-scroll">
            <table className="accounts-table">
              <thead>
                <tr>
                  <th scope="col">Category</th>
                  <th scope="col" className="col-balance">
                    {shiftMonth(monthValue, -1)}
                  </th>
                  <th scope="col" className="col-balance">
                    {shiftMonth(monthValue, -2)}
                  </th>
                </tr>
              </thead>
              <tbody>
                {budgets.map((b) => {
                  const [last, twoAgo] = actualsByCategory[b.category] ?? [
                    0, 0,
                  ];
                  return (
                    <tr key={b.category}>
                      <td>{b.category}</td>
                      <td className="col-balance">
                        {last === 0 ? (
                          <span className="muted">—</span>
                        ) : (
                          formatCents(last)
                        )}
                      </td>
                      <td className="col-balance">
                        {twoAgo === 0 ? (
                          <span className="muted">—</span>
                        ) : (
                          formatCents(twoAgo)
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </>
      )}

      {editing !== null && (
        <BudgetForm
          monthValue={monthValue}
          budget={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            refresh();
          }}
        />
      )}
    </section>
  );
}

export default BudgetsPage;
