import { useEffect, useState } from "react";
import type { Tab } from "./App";
import type {
  Account,
  OccurrenceStatus,
  PriceHistoryPoint,
  RecurringOccurrence,
  RecurringSeries,
} from "./api/midas";
import { accountsApi, recurringApi } from "./api/midas";
import { CADENCE_LABELS } from "./RecurringPatternEditor";
import RecurringPatternEditor from "./RecurringPatternEditor";
import { formatCents } from "./lib/currency";
import { formatDate } from "./lib/dateFormat";

type View = "upcoming" | "subscriptions" | "bills" | "income" | "calendar";
type SortKey = "next_date" | "name" | "monthly" | "annual";

const VIEW_LABELS: Record<View, string> = {
  upcoming: "Upcoming",
  subscriptions: "Subscriptions",
  bills: "Bills",
  income: "Income",
  calendar: "Calendar",
};

const OCCURRENCE_STATUS_LABELS: Record<OccurrenceStatus, string> = {
  upcoming: "Upcoming",
  due: "Due",
  matched: "Posted",
  missed: "Missed",
  skipped: "Skipped",
  needs_review: "Needs review",
};

function addDaysIso(days: number): string {
  const d = new Date();
  d.setUTCDate(d.getUTCDate() + days);
  return d.toISOString().slice(0, 10);
}

function firstOfMonthUtc(d: Date): Date {
  return new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), 1));
}

function addMonthsUtc(d: Date, months: number): Date {
  return new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + months, 1));
}

function isoDate(d: Date): string {
  return d.toISOString().slice(0, 10);
}

function lastOfMonthIso(d: Date): string {
  const next = addMonthsUtc(d, 1).getTime() - 24 * 60 * 60 * 1000;
  return isoDate(new Date(next));
}

const monthLabelFormatter = new Intl.DateTimeFormat("en-US", {
  month: "long",
  year: "numeric",
  timeZone: "UTC",
});

function groupByDate(
  occurrences: RecurringOccurrence[],
): [string, RecurringOccurrence[]][] {
  const map = new Map<string, RecurringOccurrence[]>();
  for (const o of occurrences) {
    const list = map.get(o.expected_date) ?? [];
    list.push(o);
    map.set(o.expected_date, list);
  }
  return [...map.entries()].sort((a, b) => a[0].localeCompare(b[0]));
}

function matchesSearch(merchant: string, search: string): boolean {
  return (
    search.trim() === "" ||
    merchant.toLowerCase().includes(search.trim().toLowerCase())
  );
}

function OccurrenceStatusTag({ status }: { status: OccurrenceStatus }) {
  switch (status) {
    case "upcoming":
      return null;
    case "due":
      return <span className="muted">{OCCURRENCE_STATUS_LABELS.due}</span>;
    case "matched":
      return (
        <span className="badge-valid">{OCCURRENCE_STATUS_LABELS.matched}</span>
      );
    case "skipped":
      return (
        <span className="badge-archived">
          {OCCURRENCE_STATUS_LABELS.skipped}
        </span>
      );
    case "missed":
      return (
        <span className="badge-review">{OCCURRENCE_STATUS_LABELS.missed}</span>
      );
    case "needs_review":
      return (
        <span className="badge-review">
          {OCCURRENCE_STATUS_LABELS.needs_review}
        </span>
      );
  }
}

function OccurrenceRow({
  occurrence,
  onSkip,
  onUnlink,
  onReview,
}: {
  occurrence: RecurringOccurrence;
  onSkip: (id: number) => void;
  onUnlink: (id: number) => void;
  onReview: () => void;
}) {
  const canSkip =
    occurrence.status === "upcoming" || occurrence.status === "due";
  const canUnlink = occurrence.status === "matched";
  return (
    <li className="review-item">
      <div className="review-item-header">
        <strong>{occurrence.merchant}</strong>
        <span className="muted">{formatDate(occurrence.expected_date)}</span>
        <span
          className={
            occurrence.direction === "income"
              ? "amount-positive"
              : "amount-owed"
          }
        >
          {formatCents(Math.abs(occurrence.expected_amount_cents))}
        </span>
        <OccurrenceStatusTag status={occurrence.status} />
      </div>
      {(canSkip || canUnlink || occurrence.status === "needs_review") && (
        <div className="review-item-actions">
          {canSkip && (
            <button
              type="button"
              className="btn-link"
              onClick={() => onSkip(occurrence.id)}
            >
              Skip
            </button>
          )}
          {canUnlink && (
            <button
              type="button"
              className="btn-link"
              onClick={() => onUnlink(occurrence.id)}
            >
              Unlink
            </button>
          )}
          {occurrence.status === "needs_review" && (
            <button type="button" className="btn-link" onClick={onReview}>
              Review in Needs Review →
            </button>
          )}
        </div>
      )}
    </li>
  );
}

function PriceHistorySparkline({ points }: { points: PriceHistoryPoint[] }) {
  const amounts = points.map((p) => Math.abs(p.amount_cents));
  const min = Math.min(...amounts);
  const max = Math.max(...amounts);
  const width = 240;
  const height = 40;
  const range = max - min || 1;
  const step = amounts.length > 1 ? width / (amounts.length - 1) : 0;
  const coords = amounts.map((a, i) => {
    const x = i * step;
    const y = height - 4 - ((a - min) / range) * (height - 8);
    return [x, y] as const;
  });

  return (
    <div className="price-history">
      <svg
        viewBox={`0 0 ${width} ${height}`}
        role="img"
        aria-label={`Price history from ${formatCents(amounts[0])} to ${formatCents(amounts[amounts.length - 1])}`}
        className="price-history-chart"
      >
        <polyline
          points={coords
            .map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`)
            .join(" ")}
          fill="none"
          stroke="var(--midas-gold)"
          strokeWidth="2"
        />
        {coords.map(([x, y], i) => (
          <circle key={i} cx={x} cy={y} r="2.5" fill="var(--midas-gold)" />
        ))}
      </svg>
      <ul className="price-history-list">
        {points.map((p) => (
          <li key={p.transaction_id}>
            <span className="muted">{formatDate(p.date)}</span>{" "}
            {formatCents(Math.abs(p.amount_cents))}
          </li>
        ))}
      </ul>
    </div>
  );
}

function SeriesCard({ s, onEdit }: { s: RecurringSeries; onEdit: () => void }) {
  const [showHistory, setShowHistory] = useState(false);
  const [history, setHistory] = useState<PriceHistoryPoint[] | null>(null);
  const variable =
    s.min_amount_cents !== null &&
    s.max_amount_cents !== null &&
    s.min_amount_cents !== s.max_amount_cents;
  const isSubscriptionLike =
    s.kind === "subscription" || s.kind === "membership";

  async function toggleHistory() {
    if (showHistory) {
      setShowHistory(false);
      return;
    }
    if (!history) {
      try {
        setHistory(await recurringApi.priceHistory(s.id));
      } catch {
        setHistory([]);
      }
    }
    setShowHistory(true);
  }

  return (
    <li className="review-item">
      <div className="review-item-header">
        <strong>{s.merchant}</strong>
        <span className="muted">{CADENCE_LABELS[s.cadence]}</span>
        {s.status === "inactive" && (
          <span className="badge-archived">Inactive</span>
        )}
        {s.planning_to_cancel && (
          <span className="badge-review">Planning to cancel</span>
        )}
        {s.is_price_changed && (
          <span className="badge-review">Price changed</span>
        )}
        {s.is_possibly_missed && (
          <span className="badge-review">Possibly missed</span>
        )}
      </div>

      {variable ? (
        <p>
          Typically{" "}
          {formatCents(
            s.median_amount_cents ?? Math.abs(s.expected_amount_cents),
          )}{" "}
          <span className="muted">
            (range {formatCents(s.min_amount_cents!)} –{" "}
            {formatCents(s.max_amount_cents!)})
          </span>
        </p>
      ) : (
        <p>{formatCents(Math.abs(s.expected_amount_cents))}</p>
      )}

      {s.is_price_changed &&
        s.previous_typical_amount_cents !== null &&
        s.last_amount_cents !== null && (
          <p className="muted">
            Was {formatCents(Math.abs(s.previous_typical_amount_cents))}, now{" "}
            {formatCents(Math.abs(s.last_amount_cents))}
          </p>
        )}

      <p className="muted">
        Next expected {formatDate(s.next_expected_date)}
        {s.last_occurrence_date &&
          ` · Last seen ${formatDate(s.last_occurrence_date)}`}
      </p>

      {isSubscriptionLike && (
        <p className="muted">
          {formatCents(s.monthly_equivalent_cents)} / mo ·{" "}
          {formatCents(s.annual_equivalent_cents)} / yr
        </p>
      )}

      <div className="review-item-actions">
        <button type="button" className="btn-link" onClick={onEdit}>
          Edit
        </button>
        {s.occurrence_count > 1 && (
          <button type="button" className="btn-link" onClick={toggleHistory}>
            {showHistory ? "Hide history" : "Price history"}
          </button>
        )}
      </div>

      {showHistory &&
        (history === null ? (
          <p className="muted">Loading…</p>
        ) : history.length < 2 ? (
          <p className="muted">Not enough history yet.</p>
        ) : (
          <PriceHistorySparkline points={history} />
        ))}
    </li>
  );
}

function ProposedCard({
  s,
  accountName,
  onConfirm,
  onReject,
}: {
  s: RecurringSeries;
  accountName: string;
  onConfirm: (id: number) => void;
  onReject: (id: number) => void;
}) {
  return (
    <li className="review-item">
      <div className="review-item-header">
        <strong>{s.merchant}</strong>
        <span className="muted">{CADENCE_LABELS[s.cadence]}</span>
        <span
          className={
            s.direction === "income" ? "amount-positive" : "amount-owed"
          }
        >
          {formatCents(Math.abs(s.expected_amount_cents))}
        </span>
      </div>
      <p className="muted">
        {s.direction === "income"
          ? "Looks like recurring income"
          : "Looks like a recurring expense"}{" "}
        · Next expected {formatDate(s.next_expected_date)} · {accountName}
      </p>
      <div className="review-item-actions">
        <button
          type="button"
          className="btn-link"
          onClick={() => onConfirm(s.id)}
        >
          Confirm
        </button>
        <button
          type="button"
          className="btn-link"
          onClick={() => onReject(s.id)}
        >
          Not recurring
        </button>
      </div>
    </li>
  );
}

function RecurringPage({ onNavigate }: { onNavigate: (tab: Tab) => void }) {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [accountFilter, setAccountFilter] = useState<number | "">("");
  const [allSeries, setAllSeries] = useState<RecurringSeries[]>([]);
  const [loading, setLoading] = useState(true);
  const [detecting, setDetecting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [detectMessage, setDetectMessage] = useState<string | null>(null);
  const [view, setView] = useState<View>("upcoming");
  const [search, setSearch] = useState("");
  const [showInactive, setShowInactive] = useState(false);
  const [sortKey, setSortKey] = useState<SortKey>("next_date");
  const [editingPattern, setEditingPattern] = useState<
    RecurringSeries | "new" | null
  >(null);

  const [upcomingOccurrences, setUpcomingOccurrences] = useState<
    RecurringOccurrence[]
  >([]);
  const [upcomingLoading, setUpcomingLoading] = useState(true);

  const [calendarCursor, setCalendarCursor] = useState(() =>
    firstOfMonthUtc(new Date()),
  );
  const [calendarOccurrences, setCalendarOccurrences] = useState<
    RecurringOccurrence[]
  >([]);
  const [calendarLoading, setCalendarLoading] = useState(true);

  const scopedAccountId = accountFilter === "" ? null : accountFilter;

  useEffect(() => {
    accountsApi
      .list(false)
      .then(setAccounts)
      .catch((err) => setError(String(err)));
  }, []);

  async function refreshSeries() {
    setLoading(true);
    try {
      setAllSeries(await recurringApi.list(null));
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  async function refreshUpcoming() {
    setUpcomingLoading(true);
    try {
      const from = new Date().toISOString().slice(0, 10);
      const to = addDaysIso(30);
      setUpcomingOccurrences(
        await recurringApi.upcoming(from, to, scopedAccountId),
      );
    } catch (err) {
      setError(String(err));
    } finally {
      setUpcomingLoading(false);
    }
  }

  async function refreshCalendar() {
    setCalendarLoading(true);
    try {
      const from = isoDate(calendarCursor);
      const to = lastOfMonthIso(calendarCursor);
      setCalendarOccurrences(
        await recurringApi.upcoming(from, to, scopedAccountId),
      );
    } catch (err) {
      setError(String(err));
    } finally {
      setCalendarLoading(false);
    }
  }

  useEffect(() => {
    refreshSeries();
  }, []);

  useEffect(() => {
    refreshUpcoming();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accountFilter]);

  useEffect(() => {
    refreshCalendar();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accountFilter, calendarCursor]);

  async function refreshAll() {
    await Promise.all([refreshSeries(), refreshUpcoming(), refreshCalendar()]);
  }

  async function handleDetect() {
    setDetecting(true);
    setDetectMessage(null);
    setError(null);
    try {
      const targets =
        accountFilter === "" ? accounts.map((a) => a.id) : [accountFilter];
      const results = await Promise.all(
        targets.map((id) => recurringApi.detect(id)),
      );
      const total = results.reduce((sum, r) => sum + r.length, 0);
      setDetectMessage(
        total === 0
          ? "No new recurring patterns found."
          : `Found ${total} new recurring pattern${total === 1 ? "" : "s"} to review below.`,
      );
      await refreshSeries();
    } catch (err) {
      setError(String(err));
    } finally {
      setDetecting(false);
    }
  }

  async function handleConfirm(id: number) {
    try {
      await recurringApi.confirm(id);
      await refreshAll();
    } catch (err) {
      setError(String(err));
    }
  }

  async function handleReject(id: number) {
    try {
      await recurringApi.reject(id);
      await refreshSeries();
    } catch (err) {
      setError(String(err));
    }
  }

  async function handleSkip(occurrenceId: number) {
    try {
      await recurringApi.skipOccurrence(occurrenceId);
      await refreshAll();
    } catch (err) {
      setError(String(err));
    }
  }

  async function handleUnlink(occurrenceId: number) {
    try {
      await recurringApi.unlinkOccurrence(occurrenceId);
      await refreshAll();
    } catch (err) {
      setError(String(err));
    }
  }

  function accountName(id: number): string {
    return accounts.find((a) => a.id === id)?.name ?? "";
  }

  const visibleSeries = allSeries.filter((s) => {
    if (accountFilter !== "" && s.account_id !== accountFilter) return false;
    return matchesSearch(s.merchant, search);
  });

  const proposed = visibleSeries.filter((s) => s.status === "proposed");

  function lifecycleOk(s: RecurringSeries): boolean {
    return (
      s.status === "confirmed" || (showInactive && s.status === "inactive")
    );
  }

  const subscriptions = visibleSeries.filter(
    (s) =>
      lifecycleOk(s) &&
      s.direction === "expense" &&
      (s.kind === "subscription" || s.kind === "membership"),
  );
  const bills = visibleSeries.filter(
    (s) =>
      lifecycleOk(s) &&
      s.direction === "expense" &&
      (s.kind === "bill" || s.kind === "other"),
  );
  const income = visibleSeries.filter(
    (s) => lifecycleOk(s) && s.direction === "income",
  );

  function sortSeries(list: RecurringSeries[]): RecurringSeries[] {
    const copy = [...list];
    switch (sortKey) {
      case "name":
        copy.sort((a, b) => a.merchant.localeCompare(b.merchant));
        break;
      case "monthly":
        copy.sort(
          (a, b) => b.monthly_equivalent_cents - a.monthly_equivalent_cents,
        );
        break;
      case "annual":
        copy.sort(
          (a, b) => b.annual_equivalent_cents - a.annual_equivalent_cents,
        );
        break;
      default:
        copy.sort((a, b) =>
          a.next_expected_date.localeCompare(b.next_expected_date),
        );
    }
    return copy;
  }

  function renderSeriesList(list: RecurringSeries[], emptyMessage: string) {
    if (list.length === 0) {
      return <p className="muted">{emptyMessage}</p>;
    }
    return (
      <ul className="review-list">
        {sortSeries(list).map((s) => (
          <SeriesCard key={s.id} s={s} onEdit={() => setEditingPattern(s)} />
        ))}
      </ul>
    );
  }

  const filteredUpcoming = upcomingOccurrences.filter((o) =>
    matchesSearch(o.merchant, search),
  );

  return (
    <section aria-labelledby="recurring-heading">
      <div className="accounts-toolbar">
        <h1 id="recurring-heading">Recurring</h1>
        <div className="accounts-toolbar-actions">
          <select
            value={accountFilter}
            onChange={(e) =>
              setAccountFilter(
                e.target.value === "" ? "" : Number(e.target.value),
              )
            }
            aria-label="Account"
          >
            <option value="">All accounts</option>
            {accounts.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name}
              </option>
            ))}
          </select>
          <button
            type="button"
            className="btn-secondary"
            onClick={handleDetect}
            disabled={detecting || accounts.length === 0}
          >
            {detecting ? "Scanning…" : "Detect recurring"}
          </button>
          <button
            type="button"
            className="btn-primary"
            onClick={() => setEditingPattern("new")}
            disabled={accounts.length === 0}
          >
            + Add recurring
          </button>
        </div>
      </div>

      {detectMessage && <p className="muted">{detectMessage}</p>}
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {proposed.length > 0 && (
        <>
          <h2 className="section-heading">New patterns found</h2>
          <ul className="review-list">
            {proposed.map((s) => (
              <ProposedCard
                key={s.id}
                s={s}
                accountName={accountName(s.account_id)}
                onConfirm={handleConfirm}
                onReject={handleReject}
              />
            ))}
          </ul>
        </>
      )}

      <div className="recurring-tabs" role="group" aria-label="Recurring views">
        {(Object.keys(VIEW_LABELS) as View[]).map((v) => (
          <button
            key={v}
            type="button"
            aria-pressed={view === v}
            className={
              view === v
                ? "recurring-tab recurring-tab-active"
                : "recurring-tab"
            }
            onClick={() => setView(v)}
          >
            {VIEW_LABELS[v]}
          </button>
        ))}
      </div>

      <div className="filters-row">
        {view !== "calendar" && (
          <input
            className="search-input"
            type="search"
            placeholder="Search by name…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            aria-label="Search recurring patterns"
          />
        )}
        {(view === "subscriptions" ||
          view === "bills" ||
          view === "income") && (
          <>
            <label className="show-archived-toggle">
              <input
                type="checkbox"
                checked={showInactive}
                onChange={(e) => setShowInactive(e.target.checked)}
              />
              Show inactive
            </label>
            <select
              value={sortKey}
              onChange={(e) => setSortKey(e.target.value as SortKey)}
              aria-label="Sort by"
            >
              <option value="next_date">Sort: Next date</option>
              <option value="name">Sort: Name</option>
              <option value="monthly">Sort: Monthly cost</option>
              <option value="annual">Sort: Annual cost</option>
            </select>
          </>
        )}
      </div>

      {loading ? (
        <p className="muted">Loading…</p>
      ) : (
        <>
          {view === "upcoming" &&
            (upcomingLoading ? (
              <p className="muted">Loading…</p>
            ) : filteredUpcoming.length === 0 ? (
              <p className="muted">Nothing expected in the next 30 days.</p>
            ) : (
              <ul className="review-list">
                {filteredUpcoming.map((o) => (
                  <OccurrenceRow
                    key={o.id}
                    occurrence={o}
                    onSkip={handleSkip}
                    onUnlink={handleUnlink}
                    onReview={() => onNavigate("review")}
                  />
                ))}
              </ul>
            ))}

          {view === "subscriptions" &&
            renderSeriesList(subscriptions, "No subscriptions yet.")}
          {view === "bills" && renderSeriesList(bills, "No bills yet.")}
          {view === "income" &&
            renderSeriesList(income, "No recurring income yet.")}

          {view === "calendar" && (
            <>
              <div className="calendar-nav">
                <button
                  type="button"
                  className="btn-secondary"
                  onClick={() => setCalendarCursor((c) => addMonthsUtc(c, -1))}
                >
                  ← Prev
                </button>
                <strong>{monthLabelFormatter.format(calendarCursor)}</strong>
                <button
                  type="button"
                  className="btn-secondary"
                  onClick={() => setCalendarCursor((c) => addMonthsUtc(c, 1))}
                >
                  Next →
                </button>
              </div>
              {calendarLoading ? (
                <p className="muted">Loading…</p>
              ) : calendarOccurrences.length === 0 ? (
                <p className="muted">Nothing expected this month.</p>
              ) : (
                groupByDate(calendarOccurrences).map(([date, occs]) => (
                  <div key={date} className="calendar-day-group">
                    <h3 className="calendar-day-heading">{formatDate(date)}</h3>
                    <ul className="review-list">
                      {occs.map((o) => (
                        <OccurrenceRow
                          key={o.id}
                          occurrence={o}
                          onSkip={handleSkip}
                          onUnlink={handleUnlink}
                          onReview={() => onNavigate("review")}
                        />
                      ))}
                    </ul>
                  </div>
                ))
              )}
            </>
          )}
        </>
      )}

      {editingPattern !== null && (
        <RecurringPatternEditor
          accounts={accounts}
          series={editingPattern}
          defaultAccountId={scopedAccountId ?? accounts[0]?.id ?? null}
          onClose={() => setEditingPattern(null)}
          onSaved={() => {
            setEditingPattern(null);
            refreshAll();
          }}
        />
      )}
    </section>
  );
}

export default RecurringPage;
