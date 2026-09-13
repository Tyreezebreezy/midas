import { useEffect, useRef, useState } from "react";
import type { CycleSummary, PaySchedule, RecurringSeries } from "./api/midas";
import { payCycleApi } from "./api/midas";
import { formatCents } from "./lib/currency";
import { formatDate } from "./lib/dateFormat";

export const SCHEDULE_TYPE_LABELS: Record<
  PaySchedule["schedule_type"],
  string
> = {
  weekly: "Weekly",
  biweekly: "Every 2 weeks",
  monthly: "Monthly",
  semimonthly: "Twice a month",
};

function suggestScheduleFromSeries(s: RecurringSeries): PaySchedule | null {
  const anchor = s.last_occurrence_date ?? s.next_expected_date;
  switch (s.cadence) {
    case "weekly":
      return { schedule_type: "weekly", anchor_date: anchor };
    case "biweekly":
      return { schedule_type: "biweekly", anchor_date: anchor };
    case "monthly":
      return {
        schedule_type: "monthly",
        day_of_month: Number(anchor.slice(8, 10)),
      };
    case "semimonthly":
      if (s.semimonthly_day_1 == null || s.semimonthly_day_2 == null) {
        return null;
      }
      return {
        schedule_type: "semimonthly",
        day_of_month_1: s.semimonthly_day_1,
        day_of_month_2: s.semimonthly_day_2,
      };
    default:
      return null; // quarterly/annual patterns don't map to a pay schedule
  }
}

function ScheduleForm({
  initial,
  onClose,
  onSaved,
}: {
  initial: PaySchedule | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [scheduleType, setScheduleType] = useState<
    PaySchedule["schedule_type"]
  >(initial?.schedule_type ?? "biweekly");
  const [anchorDate, setAnchorDate] = useState(
    initial && "anchor_date" in initial ? initial.anchor_date : "",
  );
  const [dayOfMonth, setDayOfMonth] = useState(
    initial && "day_of_month" in initial ? initial.day_of_month : 1,
  );
  const [dayOfMonth1, setDayOfMonth1] = useState(
    initial && "day_of_month_1" in initial ? initial.day_of_month_1 : 1,
  );
  const [dayOfMonth2, setDayOfMonth2] = useState(
    initial && "day_of_month_2" in initial ? initial.day_of_month_2 : 15,
  );
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    let schedule: PaySchedule;
    if (scheduleType === "weekly" || scheduleType === "biweekly") {
      if (anchorDate.trim() === "") {
        setError("Pick a date you were actually paid, to anchor the schedule.");
        return;
      }
      schedule = { schedule_type: scheduleType, anchor_date: anchorDate };
    } else if (scheduleType === "monthly") {
      schedule = { schedule_type: "monthly", day_of_month: dayOfMonth };
    } else {
      if (dayOfMonth1 === dayOfMonth2) {
        setError("The two paydays must be different days.");
        return;
      }
      schedule = {
        schedule_type: "semimonthly",
        day_of_month_1: dayOfMonth1,
        day_of_month_2: dayOfMonth2,
      };
    }

    setSaving(true);
    setError(null);
    try {
      await payCycleApi.setSchedule(schedule);
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
      aria-labelledby="schedule-form-title"
      onClose={onClose}
    >
      <form method="dialog" onSubmit={handleSubmit}>
        <h2 id="schedule-form-title">
          {initial ? "Edit pay schedule" : "Set up your pay schedule"}
        </h2>

        <div className="field">
          <label htmlFor="schedule-type">Pay frequency</label>
          <select
            id="schedule-type"
            value={scheduleType}
            onChange={(e) =>
              setScheduleType(e.target.value as PaySchedule["schedule_type"])
            }
          >
            {(
              Object.keys(
                SCHEDULE_TYPE_LABELS,
              ) as PaySchedule["schedule_type"][]
            ).map((t) => (
              <option key={t} value={t}>
                {SCHEDULE_TYPE_LABELS[t]}
              </option>
            ))}
          </select>
        </div>

        {(scheduleType === "weekly" || scheduleType === "biweekly") && (
          <div className="field">
            <label htmlFor="schedule-anchor">A date you were paid</label>
            <input
              id="schedule-anchor"
              type="date"
              value={anchorDate}
              onChange={(e) => setAnchorDate(e.target.value)}
              required
            />
            <p className="field-hint">
              Any real payday works - MIDAS counts forward/backward from it.
            </p>
          </div>
        )}

        {scheduleType === "monthly" && (
          <div className="field">
            <label htmlFor="schedule-day">Day of month</label>
            <input
              id="schedule-day"
              type="number"
              min={1}
              max={31}
              value={dayOfMonth}
              onChange={(e) => setDayOfMonth(Number(e.target.value))}
            />
            <p className="field-hint">
              Days past a short month's end (e.g. 31 in February) land on that
              month's last day.
            </p>
          </div>
        )}

        {scheduleType === "semimonthly" && (
          <div className="field mapping-row">
            <label htmlFor="schedule-day-1">First payday</label>
            <input
              id="schedule-day-1"
              type="number"
              min={1}
              max={31}
              value={dayOfMonth1}
              onChange={(e) => setDayOfMonth1(Number(e.target.value))}
            />
            <label htmlFor="schedule-day-2">Second payday</label>
            <input
              id="schedule-day-2"
              type="number"
              min={1}
              max={31}
              value={dayOfMonth2}
              onChange={(e) => setDayOfMonth2(Number(e.target.value))}
            />
          </div>
        )}

        <p className="field-hint">
          Setting or editing this schedule never changes past transactions -
          only future cycle calculations.
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
          <button type="submit" className="btn-primary" disabled={saving}>
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </form>
    </dialog>
  );
}

function PayCycleSettings() {
  const [summary, setSummary] = useState<CycleSummary | null>(null);
  const [suggestions, setSuggestions] = useState<RecurringSeries[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<PaySchedule | null | "new">(null);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      const [cycleSummary, income] = await Promise.all([
        payCycleApi.getSummary(),
        payCycleApi.getLikelyIncomeSeries(),
      ]);
      setSummary(cycleSummary);
      setSuggestions(income);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
  }, []);

  async function useSuggestion(s: RecurringSeries) {
    const schedule = suggestScheduleFromSeries(s);
    if (!schedule) return;
    try {
      await payCycleApi.setSchedule(schedule);
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <div>
      <h3 className="settings-subheading">Pay schedule</h3>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {loading ? (
        <p className="muted">Loading…</p>
      ) : !summary ? (
        <>
          <p className="muted">
            No pay schedule set yet. MIDAS needs one to compute your current
            cycle, next payday, and Safe to Spend.
          </p>
          <button
            type="button"
            className="btn-primary"
            onClick={() => setEditing("new")}
          >
            Set up your pay schedule
          </button>

          {suggestions.length > 0 && (
            <>
              <h4 className="settings-subheading">Detected income patterns</h4>
              <ul className="review-list">
                {suggestions.map((s) => {
                  const schedule = suggestScheduleFromSeries(s);
                  return (
                    <li key={s.id} className="review-item">
                      <div className="review-item-header">
                        <strong>{s.merchant}</strong>
                        <span className="muted">
                          {formatCents(s.expected_amount_cents)}
                        </span>
                      </div>
                      {schedule ? (
                        <div className="review-item-actions">
                          <button
                            type="button"
                            className="btn-link"
                            onClick={() => useSuggestion(s)}
                          >
                            Use this as my pay schedule
                          </button>
                        </div>
                      ) : (
                        <p className="muted">
                          Cadence doesn't map to a pay schedule.
                        </p>
                      )}
                    </li>
                  );
                })}
              </ul>
            </>
          )}
        </>
      ) : (
        <p className="muted">
          Current schedule:{" "}
          <strong>
            {SCHEDULE_TYPE_LABELS[summary.schedule.schedule_type]}
          </strong>
          , next payday {formatDate(summary.next_payday)}.{" "}
          <button
            type="button"
            className="btn-link"
            onClick={() => setEditing(summary.schedule)}
          >
            Edit schedule
          </button>
        </p>
      )}

      {editing !== null && (
        <ScheduleForm
          initial={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            refresh();
          }}
        />
      )}
    </div>
  );
}

export default PayCycleSettings;
