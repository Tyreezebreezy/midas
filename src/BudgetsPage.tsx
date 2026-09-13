import { useEffect, useRef, useState } from "react";
import type {
  Budget,
  BudgetClassification,
  BudgetInput,
  BudgetSuggestion,
  CategoryTotal,
  PaceStatus,
} from "./api/midas";
import { budgetsApi } from "./api/midas";
import CategoryDrilldown from "./CategoryDrilldown";
import { SUGGESTED_CATEGORIES } from "./lib/categories";
import {
  centsToDollarInput,
  formatCents,
  parseDollarsToCents,
  useMasked,
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
  discretionary: "Discretionary",
};

const PACE_STATUS_LABELS: Record<PaceStatus, string> = {
  on_track: "On track",
  faster: "Spending faster than planned",
  slower: "Spending slower than planned",
};

const monthLabelFormatter = new Intl.DateTimeFormat("en-US", {
  month: "long",
  year: "numeric",
  timeZone: "UTC",
});

function monthLabel(monthValue: string): string {
  return monthLabelFormatter.format(new Date(monthToIso(monthValue)));
}

function monthCompare(a: string, b: string): number {
  return a.localeCompare(b);
}

// Direct, human status language (sections 16/49/74) - never a raw ratio or
// jargon. "Over" always wins regardless of pace (once exceeded, pace is
// moot); a future month gets no status language at all (nothing has
// happened yet); a completed month gets a final verdict, never a stale
// pace reading; only the real current month gets live pace text.
function statusLine(b: Budget, monthValue: string): string {
  if (b.status === "over") {
    return `Over by ${formatCents(b.actual_amount_cents - b.effective_amount_cents)}`;
  }
  const cmp = monthCompare(monthValue, currentMonthValue());
  if (cmp > 0) return "Not started yet";
  if (cmp === 0) {
    return b.pace ? PACE_STATUS_LABELS[b.pace.status] : "On track";
  }
  return b.status === "near" ? "On budget" : "Under budget";
}

function BudgetProgressBar({
  percentUsed,
  percentElapsed,
  over,
}: {
  percentUsed: number;
  percentElapsed: number | null;
  over: boolean;
}) {
  const label =
    percentElapsed !== null
      ? `${percentUsed}% of budget used, ${percentElapsed}% of month elapsed`
      : `${percentUsed}% of budget used`;
  return (
    <div className="budget-progress" role="img" aria-label={label}>
      <div className="budget-progress-track">
        <div
          className={
            over
              ? "budget-progress-fill budget-progress-fill-over"
              : "budget-progress-fill"
          }
          style={{ width: `${Math.min(100, Math.max(0, percentUsed))}%` }}
        />
        {percentElapsed !== null && (
          <div
            className="budget-progress-marker"
            style={{ left: `${Math.min(100, Math.max(0, percentElapsed))}%` }}
          />
        )}
      </div>
    </div>
  );
}

function BudgetForm({
  monthValue,
  budget,
  defaultCategory,
  onClose,
  onSaved,
}: {
  monthValue: string;
  budget: Budget | null;
  defaultCategory: string;
  onClose: () => void;
  onSaved: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [category, setCategory] = useState(budget?.category ?? defaultCategory);
  const [month, setMonth] = useState(
    budget ? isoToMonthValue(budget.month) : monthValue,
  );
  const [plannedInput, setPlannedInput] = useState(
    budget ? centsToDollarInput(budget.planned_amount_cents) : "0.00",
  );
  const [classification, setClassification] = useState<BudgetClassification>(
    budget?.classification ?? "flexible",
  );
  const [rolloverEnabled, setRolloverEnabled] = useState(
    budget?.rollover_enabled ?? false,
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
      rollover_enabled: rolloverEnabled,
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
          <label htmlFor="budget-category" className="field-required">
            Category
          </label>
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
          <label htmlFor="budget-month" className="field-required">
            Month
          </label>
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
            <option value="fixed">Fixed - hard to change short-term</option>
            <option value="flexible">
              Flexible - necessary but adjustable
            </option>
            <option value="discretionary">Discretionary - optional</option>
          </select>
        </div>

        <div className="field field-checkbox">
          <label htmlFor="budget-rollover">
            <input
              id="budget-rollover"
              type="checkbox"
              checked={rolloverEnabled}
              onChange={(e) => setRolloverEnabled(e.target.checked)}
            />
            Carry unused budget (or overspending) into next month
          </label>
          <p className="field-hint">
            Good for irregular categories like Travel or Car Repair. Leave off
            for steady categories like Dining or Groceries.
          </p>
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

function BudgetCard({
  budget,
  monthValue,
  expanded,
  onToggleExpanded,
  onEdit,
  onDelete,
}: {
  budget: Budget;
  monthValue: string;
  expanded: boolean;
  onToggleExpanded: () => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const b = budget;
  const percentUsed =
    b.effective_amount_cents > 0
      ? Math.round((b.actual_amount_cents / b.effective_amount_cents) * 100)
      : 0;
  const { from, to } = monthRange(monthValue);

  return (
    <li className="review-item">
      <div className="review-item-header">
        <strong>{b.category}</strong>
        <span className="muted">{CLASSIFICATION_LABELS[b.classification]}</span>
        <span>
          {formatCents(b.actual_amount_cents)} /{" "}
          {formatCents(b.effective_amount_cents)}
        </span>
      </div>

      <BudgetProgressBar
        percentUsed={percentUsed}
        percentElapsed={b.pace?.percent_elapsed ?? null}
        over={b.status === "over"}
      />
      <p className="budget-status-line">
        <span className="muted">
          {percentUsed}% used
          {b.pace ? ` · ${b.pace.percent_elapsed}% of month elapsed` : ""}
        </span>{" "}
        <strong className={b.status === "over" ? "amount-owed" : undefined}>
          {statusLine(b, monthValue)}
        </strong>
      </p>

      {b.typical_amount_cents !== null && (
        <p className="muted">Typical: {formatCents(b.typical_amount_cents)}</p>
      )}

      {b.status !== "over" && (
        <p className="muted">
          Remaining:{" "}
          {formatCents(b.effective_amount_cents - b.actual_amount_cents)}
        </p>
      )}

      {b.rollover_enabled && b.rollover_in_cents !== 0 && (
        <p className="muted">
          {b.rollover_in_cents > 0
            ? "Carried from last month: +"
            : "Carried overspending: "}
          {formatCents(b.rollover_in_cents)}
        </p>
      )}

      <div className="review-item-actions">
        <button type="button" className="btn-link" onClick={onEdit}>
          Edit
        </button>
        <button
          type="button"
          className="btn-link btn-link-danger"
          onClick={onDelete}
        >
          Delete
        </button>
        <button type="button" className="btn-link" onClick={onToggleExpanded}>
          {expanded ? "Hide transactions" : "View transactions"}
        </button>
      </div>

      {expanded && (
        <CategoryDrilldown category={b.category} from={from} to={to} />
      )}
    </li>
  );
}

function BudgetsPage() {
  useMasked();
  const [monthValue, setMonthValue] = useState(currentMonthValue);
  const [budgets, setBudgets] = useState<Budget[]>([]);
  const [previousMonthBudgets, setPreviousMonthBudgets] = useState<Budget[]>(
    [],
  );
  const [unbudgeted, setUnbudgeted] = useState<CategoryTotal[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<Budget | null | "new">(null);
  const [newBudgetCategory, setNewBudgetCategory] = useState("");
  const [expandedCategory, setExpandedCategory] = useState<string | null>(null);
  const [copying, setCopying] = useState(false);

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
      const [current, previous, unbudgetedSpending] = await Promise.all([
        budgetsApi.list(monthToIso(monthValue)),
        budgetsApi.list(monthToIso(shiftMonth(monthValue, -1))),
        budgetsApi.unbudgetedSpending(monthToIso(monthValue)),
      ]);
      setBudgets(current);
      setPreviousMonthBudgets(previous);
      setUnbudgeted(unbudgetedSpending);
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

  async function handleCopyPreviousMonth() {
    setCopying(true);
    setError(null);
    try {
      await budgetsApi.copyFromMonth(
        monthToIso(shiftMonth(monthValue, -1)),
        monthToIso(monthValue),
      );
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setCopying(false);
    }
  }

  async function handleSuggest() {
    setSuggesting(true);
    setError(null);
    try {
      const result = await budgetsApi.suggest(monthToIso(monthValue), 6);
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
        const existing = budgets.find((b) => b.category === s.category);
        const input: BudgetInput = {
          category: s.category,
          month: monthToIso(monthValue),
          planned_amount_cents: cents,
          classification: existing?.classification ?? "flexible",
          rollover_enabled: existing?.rollover_enabled ?? false,
        };
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

  const totalEffective = budgets.reduce(
    (sum, b) => sum + b.effective_amount_cents,
    0,
  );
  const totalActual = budgets.reduce(
    (sum, b) => sum + b.actual_amount_cents,
    0,
  );
  const isCurrentMonth = monthCompare(monthValue, currentMonthValue()) === 0;
  const monthPace = budgets.find((b) => b.pace)?.pace ?? null;

  const missingFromPreviousMonth = previousMonthBudgets.filter(
    (pb) => !budgets.some((b) => b.category === pb.category),
  );

  return (
    <section aria-labelledby="budgets-heading">
      <div className="page-toolbar">
        <h1 id="budgets-heading">Budgets</h1>
        <div className="page-toolbar-actions">
          <button
            type="button"
            className="btn-secondary"
            onClick={() => setMonthValue((m) => shiftMonth(m, -1))}
            aria-label="Previous month"
          >
            ← Prev
          </button>
          <strong>{monthLabel(monthValue)}</strong>
          <button
            type="button"
            className="btn-secondary"
            onClick={() => setMonthValue((m) => shiftMonth(m, 1))}
            aria-label="Next month"
          >
            Next →
          </button>
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
            onClick={() => {
              setNewBudgetCategory("");
              setEditing("new");
            }}
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

      {!loading && budgets.length > 0 && (
        <div className="budget-summary-strip">
          <div className="stat-strip-item">
            <dt>Budgeted</dt>
            <dd>{formatCents(totalEffective)}</dd>
          </div>
          <div className="stat-strip-item">
            <dt>Spent</dt>
            <dd>{formatCents(totalActual)}</dd>
          </div>
          <div className="stat-strip-item">
            <dt>Remaining</dt>
            <dd>{formatCents(totalEffective - totalActual)}</dd>
          </div>
          {isCurrentMonth && monthPace && (
            <div className="stat-strip-item">
              <dt>Month elapsed</dt>
              <dd>{monthPace.percent_elapsed}%</dd>
            </div>
          )}
        </div>
      )}

      {!loading && missingFromPreviousMonth.length > 0 && (
        <p className="muted budget-copy-prompt">
          {missingFromPreviousMonth.length} categor
          {missingFromPreviousMonth.length === 1 ? "y" : "ies"} from{" "}
          {monthLabel(shiftMonth(monthValue, -1))} not set up here yet.{" "}
          <button
            type="button"
            className="btn-link"
            onClick={handleCopyPreviousMonth}
            disabled={copying}
          >
            {copying ? "Copying…" : "Copy last month's budgets"}
          </button>
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
                Based on the median of the last{" "}
                {Math.max(...suggestions.map((s) => s.months_of_history))} month
                {Math.max(...suggestions.map((s) => s.months_of_history)) === 1
                  ? ""
                  : "s"}{" "}
                of categorized spending, excluding anything marked one-time.
                Nothing is applied until you select categories and confirm
                below.
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
                        Typical
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
                          {formatCents(s.typical_amount_cents)}
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
        <ul className="review-list">
          {budgets.map((b) => (
            <BudgetCard
              key={b.id}
              budget={b}
              monthValue={monthValue}
              expanded={expandedCategory === b.category}
              onToggleExpanded={() =>
                setExpandedCategory((prev) =>
                  prev === b.category ? null : b.category,
                )
              }
              onEdit={() => setEditing(b)}
              onDelete={() => handleDelete(b.id)}
            />
          ))}
        </ul>
      )}

      {!loading && unbudgeted.length > 0 && (
        <>
          <h2 className="section-heading">Unbudgeted spending</h2>
          <p className="muted">Spending happening outside the plan above.</p>
          <ul className="review-list">
            {unbudgeted.map((c) => (
              <li key={c.category} className="review-item">
                <div className="review-item-header">
                  <strong>{c.category}</strong>
                  <span>{formatCents(c.spent_cents)}</span>
                </div>
                <div className="review-item-actions">
                  {c.category === "Uncategorized" ? (
                    <span className="muted">
                      Can't be budgeted until categorized.
                    </span>
                  ) : (
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => {
                        setNewBudgetCategory(c.category);
                        setEditing("new");
                      }}
                    >
                      Set budget
                    </button>
                  )}
                </div>
              </li>
            ))}
          </ul>
        </>
      )}

      {editing !== null && (
        <BudgetForm
          monthValue={monthValue}
          budget={editing === "new" ? null : editing}
          defaultCategory={editing === "new" ? newBudgetCategory : ""}
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
