import { useEffect, useRef, useState } from "react";
import type { SavingsGoal, SavingsGoalInput } from "./api/midas";
import { savingsGoalsApi } from "./api/midas";
import {
  centsToDollarInput,
  formatCents,
  parseDollarsToCents,
  useMasked,
} from "./lib/currency";
import { formatDate } from "./lib/dateFormat";

const STATUS_LABELS: Record<SavingsGoal["status"], string> = {
  in_progress: "In progress",
  completed: "Completed",
  overdue: "Overdue",
};

function GoalForm({
  goal,
  onClose,
  onSaved,
}: {
  goal: SavingsGoal | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [name, setName] = useState(goal?.name ?? "");
  const [targetInput, setTargetInput] = useState(
    goal ? centsToDollarInput(goal.target_amount_cents) : "0.00",
  );
  const [currentInput, setCurrentInput] = useState(
    goal ? centsToDollarInput(goal.current_amount_cents) : "0.00",
  );
  const [targetDate, setTargetDate] = useState(goal?.target_date ?? "");
  const [reservedInput, setReservedInput] = useState(
    goal?.reserved_amount_cents != null
      ? centsToDollarInput(goal.reserved_amount_cents)
      : "",
  );
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (name.trim() === "") {
      setError("Name is required.");
      return;
    }
    const target = parseDollarsToCents(targetInput);
    if (target === null || target <= 0) {
      setError("Target amount must be greater than zero.");
      return;
    }
    const current = parseDollarsToCents(currentInput);
    if (current === null || current < 0) {
      setError("Current amount must be a non-negative number.");
      return;
    }
    let reserved: number | null = null;
    if (reservedInput.trim() !== "") {
      const parsed = parseDollarsToCents(reservedInput);
      if (parsed === null || parsed < 0) {
        setError("Reserved amount must be a non-negative number.");
        return;
      }
      reserved = parsed;
    }

    const input: SavingsGoalInput = {
      name: name.trim(),
      target_amount_cents: target,
      current_amount_cents: current,
      target_date: targetDate.trim() === "" ? null : targetDate,
      reserved_amount_cents: reserved,
    };

    setSaving(true);
    setError(null);
    try {
      if (goal) {
        await savingsGoalsApi.update(goal.id, input);
      } else {
        await savingsGoalsApi.create(input);
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
      aria-labelledby="goal-form-title"
      onClose={onClose}
    >
      <form method="dialog" onSubmit={handleSubmit}>
        <h2 id="goal-form-title">{goal ? "Edit goal" : "Add goal"}</h2>

        <div className="field">
          <label htmlFor="goal-name" className="field-required">
            Name
          </label>
          <input
            id="goal-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            autoFocus
          />
        </div>

        <div className="field">
          <label htmlFor="goal-target">Target amount (USD)</label>
          <input
            id="goal-target"
            inputMode="decimal"
            value={targetInput}
            onChange={(e) => setTargetInput(e.target.value)}
          />
        </div>

        <div className="field">
          <label htmlFor="goal-current">Current amount (USD)</label>
          <input
            id="goal-current"
            inputMode="decimal"
            value={currentInput}
            onChange={(e) => setCurrentInput(e.target.value)}
          />
        </div>

        <div className="field">
          <label htmlFor="goal-target-date">Target date</label>
          <input
            id="goal-target-date"
            type="date"
            value={targetDate}
            onChange={(e) => setTargetDate(e.target.value)}
          />
          <p className="field-hint">Optional.</p>
        </div>

        <div className="field">
          <label htmlFor="goal-reserved">
            Protect from Safe to Spend (USD)
          </label>
          <input
            id="goal-reserved"
            inputMode="decimal"
            value={reservedInput}
            onChange={(e) => setReservedInput(e.target.value)}
            placeholder="Not reserved"
          />
          <p className="field-hint">
            Optional. Leave blank unless you want this amount excluded from Safe
            to Spend.
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

function SavingsGoalsPage() {
  useMasked();
  const [goals, setGoals] = useState<SavingsGoal[]>([]);
  const [showArchived, setShowArchived] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<SavingsGoal | null | "new">(null);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      setGoals(await savingsGoalsApi.list(showArchived));
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showArchived]);

  async function toggleArchived(goal: SavingsGoal) {
    try {
      await savingsGoalsApi.setArchived(goal.id, !goal.is_archived);
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <section aria-labelledby="goals-heading">
      <div className="page-toolbar">
        <h1 id="goals-heading">Savings Goals</h1>
        <div className="page-toolbar-actions">
          <label className="show-archived-toggle">
            <input
              type="checkbox"
              checked={showArchived}
              onChange={(e) => setShowArchived(e.target.checked)}
            />
            Show archived
          </label>
          <button
            type="button"
            className="btn-primary"
            onClick={() => setEditing("new")}
          >
            Add goal
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
      ) : goals.length === 0 ? (
        <p className="muted">No savings goals yet.</p>
      ) : (
        <ul className="review-list">
          {goals.map((g) => {
            const pct = Math.min(
              100,
              Math.round(
                (g.current_amount_cents / g.target_amount_cents) * 100,
              ),
            );
            return (
              <li key={g.id} className="review-item">
                <div className="review-item-header">
                  <strong>{g.name}</strong>
                  <span className="muted">{STATUS_LABELS[g.status]}</span>
                  <span>
                    {formatCents(g.current_amount_cents)} of{" "}
                    {formatCents(g.target_amount_cents)}
                  </span>
                </div>
                <div className="goal-progress-track">
                  <div
                    className="goal-progress-fill"
                    style={{ width: `${pct}%` }}
                  />
                </div>
                <p className="muted">
                  {g.target_date
                    ? `Target date ${formatDate(g.target_date)}`
                    : "No target date"}
                  {g.reserved_amount_cents != null &&
                    ` · ${formatCents(g.reserved_amount_cents)} protected from Safe to Spend`}
                </p>
                <div className="review-item-actions">
                  <button
                    type="button"
                    className="btn-link"
                    onClick={() => setEditing(g)}
                  >
                    Edit
                  </button>
                  <button
                    type="button"
                    className="btn-link"
                    onClick={() => toggleArchived(g)}
                  >
                    {g.is_archived ? "Unarchive" : "Archive"}
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}

      {editing !== null && (
        <GoalForm
          goal={editing === "new" ? null : editing}
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

export default SavingsGoalsPage;
