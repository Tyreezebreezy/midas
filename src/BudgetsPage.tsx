import { useEffect, useRef, useState } from "react";
import type {
  Budget,
  BudgetClassification,
  BudgetInput,
  BudgetSuggestion,
} from "./api/midas";
import { accountingApi, budgetsApi } from "./api/midas";
import { SUGGESTED_CATEGORIES } from "./lib/categories";
import {
  centsToDollarInput,
  formatCents,
  parseDollarsToCents,
} from "./lib/currency";
import {
  currentMonthValue,
  isoToMonthValue,
  monthRange,
  monthToIso,
  shiftMonth,
} from "./lib/monthValue";

const CLASSIFICATION_LABELS: Record<BudgetClassification, string> = {
  fixed: "Fixed",
  flexible: "Flexible",
};

const STATUS_LABELS: Record<Budget["status"], string> = {
  under: "On track",
  near: "Near limit",
  over: "Over budget",
};

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

  const [suggestions, setSuggestions] = useState<BudgetSuggestion[] | null>(
    null,
  );
  const [suggesting, setSuggesting] = useState(false);
  const [selectedCategories, setSelectedCategories] = useState<Set<string>>(
    new Set(),
  );
  const [suggestionInputs, setSuggestionInputs] = useState<
    Record<string, string>
  >({});
  const [applying, setApplying] = useState(false);

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

  async function handleSuggest() {
    setSuggesting(true);
    setError(null);
    try {
      const result = await budgetsApi.suggest(monthToIso(monthValue), 3);
      setSuggestions(result);
      // Default-select only categories with no existing budget, so a first
      // click on "Apply selected" can never silently change a number the
      // owner already set on purpose.
      setSelectedCategories(
        new Set(
          result
            .filter((s) => s.existing_planned_cents === null)
            .map((s) => s.category),
        ),
      );
      setSuggestionInputs(
        Object.fromEntries(
          result.map((s) => [
            s.category,
            centsToDollarInput(s.suggested_cents),
          ]),
        ),
      );
    } catch (err) {
      setError(String(err));
    } finally {
      setSuggesting(false);
    }
  }

  function toggleSuggestion(category: string) {
    setSelectedCategories((prev) => {
      const next = new Set(prev);
      if (next.has(category)) next.delete(category);
      else next.add(category);
      return next;
    });
  }

  async function handleApplySuggestions() {
    if (!suggestions) return;
    setApplying(true);
    setError(null);
    try {
      for (const s of suggestions) {
        if (!selectedCategories.has(s.category)) continue;
        const cents = parseDollarsToCents(suggestionInputs[s.category] ?? "");
        if (cents === null || cents < 0) continue;
        const input: BudgetInput = {
          category: s.category,
          month: monthToIso(monthValue),
          planned_amount_cents: cents,
          classification: "flexible",
        };
        const existing = budgets.find((b) => b.category === s.category);
        if (existing) {
          await budgetsApi.update(existing.id, input);
        } else {
          await budgetsApi.create(input);
        }
      }
      setSuggestions(null);
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setApplying(false);
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
            className="btn-secondary"
            onClick={handleSuggest}
            disabled={suggesting}
          >
            {suggesting ? "Suggesting…" : "Suggest from spending"}
          </button>
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

      {suggestions && (
        <div className="budget-suggestions">
          <h2 className="section-heading">Suggested budgets</h2>
          {suggestions.length === 0 ? (
            <p className="muted">
              MIDAS needs more categorized spending history before it can
              suggest a budget.
            </p>
          ) : (
            <>
              <p className="muted">
                Based on the last{" "}
                {Math.max(...suggestions.map((s) => s.months_of_history))} month
                {Math.max(...suggestions.map((s) => s.months_of_history)) === 1
                  ? ""
                  : "s"}{" "}
                of categorized spending. Nothing is applied until you select
                categories and confirm below.
              </p>
              <div className="table-scroll">
                <table className="accounts-table">
                  <thead>
                    <tr>
                      <th scope="col">
                        <span className="sr-only">Include</span>
                      </th>
                      <th scope="col">Category</th>
                      <th scope="col" className="col-balance">
                        Recent average
                      </th>
                      <th scope="col" className="col-balance">
                        Current
                      </th>
                      <th scope="col" className="col-balance">
                        Suggested
                      </th>
                      <th scope="col">History</th>
                    </tr>
                  </thead>
                  <tbody>
                    {suggestions.map((s) => (
                      <tr key={s.category}>
                        <td>
                          <input
                            type="checkbox"
                            checked={selectedCategories.has(s.category)}
                            onChange={() => toggleSuggestion(s.category)}
                            aria-label={`Include ${s.category}`}
                          />
                        </td>
                        <td>{s.category}</td>
                        <td className="col-balance">
                          {formatCents(s.average_actual_cents)}
                        </td>
                        <td className="col-balance">
                          {s.existing_planned_cents !== null ? (
                            formatCents(s.existing_planned_cents)
                          ) : (
                            <span className="muted">—</span>
                          )}
                        </td>
                        <td className="col-balance">
                          <input
                            inputMode="decimal"
                            className="suggestion-amount"
                            value={suggestionInputs[s.category] ?? ""}
                            onChange={(e) =>
                              setSuggestionInputs((prev) => ({
                                ...prev,
                                [s.category]: e.target.value,
                              }))
                            }
                            aria-label={`Suggested amount for ${s.category}`}
                          />
                        </td>
                        <td className="muted">
                          {s.months_of_history} month
                          {s.months_of_history === 1 ? "" : "s"}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <div className="dialog-actions">
                <button
                  type="button"
                  className="btn-secondary"
                  onClick={() => setSuggestions(null)}
                  disabled={applying}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  className="btn-primary"
                  onClick={handleApplySuggestions}
                  disabled={applying || selectedCategories.size === 0}
                >
                  {applying
                    ? "Applying…"
                    : `Apply ${selectedCategories.size} selected`}
                </button>
              </div>
            </>
          )}
        </div>
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
