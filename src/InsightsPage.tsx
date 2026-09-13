import { useEffect, useState } from "react";
import CategoryDonut, { buildDonutSlices } from "./CategoryDonut";
import CategoryDrilldown from "./CategoryDrilldown";
import MerchantDrilldown from "./MerchantDrilldown";
import {
  accountingApi,
  insightsApi,
  reportsApi,
  type CategoryDetail,
  type IncomeInsights,
  type MerchantDetail,
  type MonthlyFlow,
  type NetWorthBreakdown,
  type NetWorthChange,
  type NetWorthPoint,
  type PayeeTotal,
  type SpendingInsights,
} from "./api/midas";
import { formatCents } from "./lib/currency";
import {
  currentPeriodValue,
  periodComparison,
  periodLabel,
  shiftPeriod,
  type PeriodMode,
} from "./lib/period";

type InsightsTab =
  "spending" | "income" | "merchants" | "cashflow" | "networth";

const TABS: { id: InsightsTab; label: string }[] = [
  { id: "spending", label: "Spending" },
  { id: "income", label: "Income" },
  { id: "merchants", label: "Merchants" },
  { id: "cashflow", label: "Cash Flow" },
  { id: "networth", label: "Net Worth" },
];

const MODES: { id: PeriodMode; label: string }[] = [
  { id: "month", label: "Month" },
  { id: "quarter", label: "Quarter" },
  { id: "year", label: "Year" },
];

function comparisonSuffix(
  mode: PeriodMode,
  previousValue: string,
  isPartial: boolean,
): string {
  return isPartial
    ? `vs same point last ${mode}`
    : `vs ${periodLabel(mode, previousValue)}`;
}

// A plain-text, never color-only comparison line (sections 6, 64): the
// direction is spelled out in words, an arrow is purely decorative on top.
function ComparisonLine({
  currentCents,
  previousCents,
  suffix,
}: {
  currentCents: number;
  previousCents: number;
  suffix: string;
}) {
  if (previousCents === 0) {
    return <p className="muted">No prior spending to compare {suffix}.</p>;
  }
  const delta = currentCents - previousCents;
  const pct =
    Math.round((Math.abs(delta) / Math.abs(previousCents)) * 1000) / 10;
  if (delta === 0) {
    return <p className="muted">Unchanged {suffix}.</p>;
  }
  const arrow = delta > 0 ? "↑" : "↓";
  const word = delta > 0 ? "more" : "less";
  return (
    <p className="insights-comparison muted">
      <span aria-hidden="true">{arrow} </span>
      {formatCents(Math.abs(delta))} ({pct}%) {word} {suffix}
    </p>
  );
}

function MiniBarTrend({
  trend,
  showIncome,
}: {
  trend: MonthlyFlow[];
  showIncome: boolean;
}) {
  const max = Math.max(
    1,
    ...trend.map((t) => Math.max(t.income_cents, t.spending_cents)),
  );
  const monthShort = (iso: string) =>
    new Intl.DateTimeFormat("en-US", {
      month: "short",
      timeZone: "UTC",
    }).format(new Date(`${iso}T00:00:00Z`));
  const summary = trend
    .map(
      (t) =>
        `${monthShort(t.month)}: ${formatCents(t.spending_cents)} spent${showIncome ? `, ${formatCents(t.income_cents)} income` : ""}`,
    )
    .join("; ");

  return (
    <div
      className="insights-trend"
      role="img"
      aria-label={`Trend for the last ${trend.length} months. ${summary}.`}
    >
      {trend.map((t) => (
        <div className="insights-trend-col" key={t.month}>
          <div className="insights-trend-bars">
            {showIncome && (
              <div
                className="insights-trend-bar insights-trend-bar-income"
                style={{ height: `${(t.income_cents / max) * 100}%` }}
              />
            )}
            <div
              className="insights-trend-bar insights-trend-bar-spending"
              style={{ height: `${(t.spending_cents / max) * 100}%` }}
            />
          </div>
          <span className="muted insights-trend-label">
            {monthShort(t.month)}
          </span>
        </div>
      ))}
    </div>
  );
}

function PeriodNav({
  mode,
  value,
  onModeChange,
  onValueChange,
}: {
  mode: PeriodMode;
  value: string;
  onModeChange: (mode: PeriodMode) => void;
  onValueChange: (value: string) => void;
}) {
  const isCurrent = value === currentPeriodValue(mode);
  return (
    <div className="insights-period-nav">
      <div className="accounts-toolbar-actions">
        <button
          type="button"
          className="btn-secondary"
          onClick={() => onValueChange(shiftPeriod(mode, value, -1))}
        >
          ‹ Prior
        </button>
        <strong>{periodLabel(mode, value)}</strong>
        <button
          type="button"
          className="btn-secondary"
          onClick={() => onValueChange(shiftPeriod(mode, value, 1))}
          disabled={isCurrent}
        >
          Next ›
        </button>
        {!isCurrent && (
          <button
            type="button"
            className="btn-link"
            onClick={() => onValueChange(currentPeriodValue(mode))}
          >
            Current {mode}
          </button>
        )}
      </div>
      <div
        className="insights-mode-switch"
        role="group"
        aria-label="Period mode"
      >
        {MODES.map((m) => (
          <button
            key={m.id}
            type="button"
            aria-pressed={m.id === mode}
            className={
              m.id === mode ? "btn-secondary btn-mode-active" : "btn-secondary"
            }
            onClick={() => onModeChange(m.id)}
          >
            {m.label}
          </button>
        ))}
      </div>
    </div>
  );
}

// ---------- Spending ----------

function SpendingTab({
  from,
  to,
  previousFrom,
  previousTo,
  compareSuffix,
  mode,
}: {
  from: string;
  to: string;
  previousFrom: string;
  previousTo: string;
  compareSuffix: string;
  mode: PeriodMode;
}) {
  const [insights, setInsights] = useState<SpendingInsights | null>(null);
  const [previousTotal, setPreviousTotal] = useState<number | null>(null);
  const [merchants, setMerchants] = useState<PayeeTotal[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [detail, setDetail] = useState<CategoryDetail | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setSelected(null);
    setDetail(null);
    setError(null);
    setInsights(null);
    Promise.all([
      insightsApi.getSpendingInsights(from, to),
      insightsApi.getSpendingInsights(previousFrom, previousTo),
      insightsApi.getTopMerchants(from, to),
    ])
      .then(([current, previous, top]) => {
        setInsights(current);
        setPreviousTotal(previous.total_cents);
        setMerchants(top.slice(0, 5));
      })
      .catch((err) => setError(String(err)));
  }, [from, to, previousFrom, previousTo]);

  useEffect(() => {
    if (!selected) {
      setDetail(null);
      return;
    }
    insightsApi
      .getCategoryDetail(selected, from, to, previousFrom, previousTo)
      .then(setDetail)
      .catch((err) => setError(String(err)));
  }, [selected, from, to, previousFrom, previousTo]);

  if (error) {
    return (
      <p className="form-error" role="alert">
        {error}
      </p>
    );
  }
  if (!insights) return <p className="muted">Loading…</p>;

  const donutSlices = buildDonutSlices(insights.categories);
  const uncategorized = insights.categories.find(
    (c) => c.category === "Uncategorized",
  );

  return (
    <div>
      <p className="headline-number">{formatCents(insights.total_cents)}</p>
      <p className="muted">Spent this {mode}</p>
      {previousTotal !== null && (
        <ComparisonLine
          currentCents={insights.total_cents}
          previousCents={previousTotal}
          suffix={compareSuffix}
        />
      )}

      <div className="insights-one-time-strip">
        <span>
          One-time: <strong>{formatCents(insights.one_time_cents)}</strong>
        </span>
        <span>
          Typical/ordinary:{" "}
          <strong>
            {formatCents(insights.total_cents - insights.one_time_cents)}
          </strong>
        </span>
        <span>
          Recurring: <strong>{formatCents(insights.recurring_cents)}</strong>
        </span>
        <span>
          Non-recurring:{" "}
          <strong>{formatCents(insights.non_recurring_cents)}</strong>
        </span>
      </div>

      <h2 className="section-heading">Category breakdown</h2>
      {insights.categories.length === 0 ? (
        <p className="muted">No spending recorded this {mode}.</p>
      ) : (
        <div className="reports-category-layout">
          <div className="category-donut-wrap">
            <CategoryDonut
              slices={donutSlices}
              total={insights.total_cents}
              selected={selected}
              onSelect={(category) =>
                setSelected(selected === category ? null : category)
              }
            />
          </div>
          <ul className="review-list">
            {insights.categories.map((c) => {
              const color =
                donutSlices.find((s) => s.category === c.category)?.color ??
                "var(--midas-border)";
              return (
                <li key={c.category} className="review-item">
                  <button
                    type="button"
                    className="category-row"
                    onClick={() =>
                      setSelected(selected === c.category ? null : c.category)
                    }
                    aria-expanded={selected === c.category}
                  >
                    <span className="category-row-label">
                      <span
                        className="category-legend-swatch"
                        style={{ background: color }}
                        aria-hidden="true"
                      />
                      {c.category}
                    </span>
                    <span>
                      {formatCents(c.spent_cents)}
                      <span className="muted">
                        {" "}
                        (
                        {Math.round(
                          (c.spent_cents / (insights.total_cents || 1)) * 100,
                        )}
                        %)
                      </span>
                    </span>
                  </button>
                  {selected === c.category && detail && (
                    <div className="category-detail-panel">
                      <div className="insights-one-time-strip">
                        <span>
                          Previous:{" "}
                          <strong>
                            {formatCents(detail.previous_spent_cents)}
                          </strong>
                        </span>
                        <span>
                          Change:{" "}
                          <strong>
                            {formatCents(
                              detail.spent_cents - detail.previous_spent_cents,
                            )}
                          </strong>
                        </span>
                        {detail.typical_cents !== null ? (
                          <span>
                            Typical:{" "}
                            <strong>{formatCents(detail.typical_cents)}</strong>
                          </span>
                        ) : (
                          <span className="muted">Limited history</span>
                        )}
                        <span>
                          Transactions:{" "}
                          <strong>{detail.transaction_count}</strong>
                        </span>
                        <span>
                          One-time:{" "}
                          <strong>{formatCents(detail.one_time_cents)}</strong>
                        </span>
                      </div>
                      <CategoryDrilldown
                        category={c.category}
                        from={from}
                        to={to}
                      />
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        </div>
      )}

      {uncategorized && uncategorized.spent_cents > 0 && (
        <p className="muted">
          Uncategorized spending: {formatCents(uncategorized.spent_cents)} -
          review it on the Needs Review page.
        </p>
      )}

      {merchants.length > 0 && (
        <>
          <h2 className="section-heading">Top merchants</h2>
          <ul className="review-list">
            {merchants.map((m) => (
              <li
                key={`${m.payee_id ?? m.name}`}
                className="review-item review-item-compact"
              >
                <span>{m.name}</span>
                <span>
                  {formatCents(m.amount_cents)}{" "}
                  <span className="muted">
                    ({m.transaction_count}{" "}
                    {m.transaction_count === 1 ? "transaction" : "transactions"}
                    )
                  </span>
                </span>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

// ---------- Income ----------

function IncomeTab({
  from,
  to,
  previousFrom,
  previousTo,
  compareSuffix,
  mode,
}: {
  from: string;
  to: string;
  previousFrom: string;
  previousTo: string;
  compareSuffix: string;
  mode: PeriodMode;
}) {
  const [insights, setInsights] = useState<IncomeInsights | null>(null);
  const [previousTotal, setPreviousTotal] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setError(null);
    setInsights(null);
    Promise.all([
      insightsApi.getIncomeInsights(from, to),
      insightsApi.getIncomeInsights(previousFrom, previousTo),
    ])
      .then(([current, previous]) => {
        setInsights(current);
        setPreviousTotal(previous.total_cents);
      })
      .catch((err) => setError(String(err)));
  }, [from, to, previousFrom, previousTo]);

  if (error) {
    return (
      <p className="form-error" role="alert">
        {error}
      </p>
    );
  }
  if (!insights) return <p className="muted">Loading…</p>;

  return (
    <div>
      <p className="headline-number">{formatCents(insights.total_cents)}</p>
      <p className="muted">Income this {mode}</p>
      {previousTotal !== null && (
        <ComparisonLine
          currentCents={insights.total_cents}
          previousCents={previousTotal}
          suffix={compareSuffix}
        />
      )}

      <div className="insights-one-time-strip">
        <span>
          Recurring: <strong>{formatCents(insights.recurring_cents)}</strong>
        </span>
        <span>
          Other income:{" "}
          <strong>{formatCents(insights.non_recurring_cents)}</strong>
        </span>
      </div>

      <h2 className="section-heading">Sources</h2>
      {insights.sources.length === 0 ? (
        <p className="muted">No income recorded this {mode}.</p>
      ) : (
        <ul className="review-list">
          {insights.sources.map((s) => (
            <li
              key={`${s.payee_id ?? s.name}`}
              className="review-item review-item-compact"
            >
              <span>{s.name}</span>
              <span>
                {formatCents(s.amount_cents)}{" "}
                <span className="muted">
                  ({s.transaction_count}{" "}
                  {s.transaction_count === 1 ? "payment" : "payments"})
                </span>
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// ---------- Merchants ----------

function MerchantsTab({
  from,
  to,
  previousFrom,
  previousTo,
}: {
  from: string;
  to: string;
  previousFrom: string;
  previousTo: string;
}) {
  const [merchants, setMerchants] = useState<PayeeTotal[] | null>(null);
  const [selected, setSelected] = useState<PayeeTotal | null>(null);
  const [detail, setDetail] = useState<MerchantDetail | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setSelected(null);
    setDetail(null);
    setError(null);
    insightsApi
      .getTopMerchants(from, to)
      .then(setMerchants)
      .catch((err) => setError(String(err)));
  }, [from, to]);

  useEffect(() => {
    if (!selected) {
      setDetail(null);
      return;
    }
    insightsApi
      .getMerchantDetail(
        selected.payee_id,
        selected.payee_id ? null : selected.name,
        from,
        to,
        previousFrom,
        previousTo,
      )
      .then(setDetail)
      .catch((err) => setError(String(err)));
  }, [selected, from, to, previousFrom, previousTo]);

  if (error) {
    return (
      <p className="form-error" role="alert">
        {error}
      </p>
    );
  }
  if (!merchants) return <p className="muted">Loading…</p>;
  if (merchants.length === 0) {
    return <p className="muted">No merchant spending recorded this period.</p>;
  }

  return (
    <div>
      <h2 className="section-heading">Top merchants this period</h2>
      <ul className="review-list">
        {merchants.map((m) => {
          const isSelected =
            selected?.payee_id === m.payee_id && selected?.name === m.name;
          return (
            <li key={`${m.payee_id ?? m.name}`} className="review-item">
              <button
                type="button"
                className="category-row"
                onClick={() => setSelected(isSelected ? null : m)}
                aria-expanded={isSelected}
              >
                <span className="category-row-label">{m.name}</span>
                <span>
                  {formatCents(m.amount_cents)}{" "}
                  <span className="muted">
                    ({m.transaction_count}{" "}
                    {m.transaction_count === 1 ? "transaction" : "transactions"}
                    )
                  </span>
                </span>
              </button>
              {isSelected && detail && (
                <div className="category-detail-panel">
                  <div className="insights-one-time-strip">
                    <span>
                      Previous period:{" "}
                      <strong>
                        {formatCents(detail.previous_spent_cents)}
                      </strong>
                    </span>
                    <span>
                      Average:{" "}
                      <strong>{formatCents(detail.average_cents)}</strong>
                    </span>
                    {detail.is_recurring && (
                      <span className="muted">Has a recurring pattern</span>
                    )}
                  </div>
                  {detail.categories.length > 0 && (
                    <p className="muted">
                      {detail.categories
                        .map(
                          (c) => `${c.category} ${formatCents(c.amount_cents)}`,
                        )
                        .join(" · ")}
                    </p>
                  )}
                  <MerchantDrilldown
                    payeeId={m.payee_id}
                    merchant={m.name}
                    from={from}
                    to={to}
                  />
                </div>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

// ---------- Cash Flow ----------

function CashFlowTab({
  from,
  to,
  mode,
}: {
  from: string;
  to: string;
  mode: PeriodMode;
}) {
  const [flow, setFlow] = useState<{
    income_cents: number;
    spending_cents: number;
    net_cents: number;
  } | null>(null);
  const [trend, setTrend] = useState<MonthlyFlow[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setError(null);
    setFlow(null);
    Promise.all([
      insightsApi.getCashFlowInsights(from, to),
      reportsApi.getIncomeVsSpendingTrend(),
    ])
      .then(([f, t]) => {
        setFlow(f);
        setTrend(t);
      })
      .catch((err) => setError(String(err)));
  }, [from, to]);

  if (error) {
    return (
      <p className="form-error" role="alert">
        {error}
      </p>
    );
  }
  if (!flow) return <p className="muted">Loading…</p>;

  return (
    <div>
      <div className="insights-cashflow-summary">
        <div>
          <p className="muted">Income</p>
          <p className="headline-number amount-positive">
            {formatCents(flow.income_cents)}
          </p>
        </div>
        <div>
          <p className="muted">Spending</p>
          <p className="headline-number amount-owed">
            -{formatCents(flow.spending_cents)}
          </p>
        </div>
        <div>
          <p className="muted">Net cash flow</p>
          <p className="headline-number">{formatCents(flow.net_cents)}</p>
        </div>
      </div>
      <p className="muted">
        Net cash flow is income minus spending for this {mode} - a different
        number from net worth change, which also reflects account valuation.
      </p>

      <h2 className="section-heading">Last 6 months</h2>
      <MiniBarTrend trend={trend} showIncome />
      <div className="table-scroll">
        <table className="accounts-table">
          <thead>
            <tr>
              <th scope="col">Month</th>
              <th scope="col" className="col-balance">
                Income
              </th>
              <th scope="col" className="col-balance">
                Spending
              </th>
              <th scope="col" className="col-balance">
                Net
              </th>
            </tr>
          </thead>
          <tbody>
            {trend.map((point) => (
              <tr key={point.month}>
                <td>{periodLabel("month", point.month.slice(0, 7))}</td>
                <td className="col-balance">
                  <span className="amount-positive">
                    {formatCents(point.income_cents)}
                  </span>
                </td>
                <td className="col-balance">
                  <span className="amount-owed">
                    {formatCents(point.spending_cents)}
                  </span>
                </td>
                <td className="col-balance">
                  {formatCents(point.income_cents - point.spending_cents)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

// ---------- Net Worth ----------

function NetWorthTab({
  from,
  to,
  mode,
}: {
  from: string;
  to: string;
  mode: PeriodMode;
}) {
  const [breakdown, setBreakdown] = useState<NetWorthBreakdown | null>(null);
  const [history, setHistory] = useState<NetWorthPoint[]>([]);
  const [change, setChange] = useState<NetWorthChange | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setError(null);
    setBreakdown(null);
    setChange(null);
    Promise.all([
      accountingApi.getNetWorth(),
      accountingApi.getNetWorthHistory(),
      accountingApi.getNetWorthChange(from, to),
    ])
      .then(([nw, hist, ch]) => {
        setBreakdown(nw);
        setHistory(hist);
        setChange(ch);
      })
      .catch((err) => setError(String(err)));
  }, [from, to]);

  if (error) {
    return (
      <p className="form-error" role="alert">
        {error}
      </p>
    );
  }
  if (!breakdown || !change) return <p className="muted">Loading…</p>;

  return (
    <div>
      <p className="headline-number">
        {formatCents(breakdown.net_worth_cents)}
      </p>
      <p
        className={
          change.change_cents >= 0
            ? "muted amount-positive"
            : "muted amount-owed"
        }
      >
        {change.change_cents >= 0 ? "+" : ""}
        {formatCents(change.change_cents)} this {mode}
      </p>

      <div className="table-scroll">
        <table className="accounts-table">
          <tbody>
            <tr>
              <td>Assets</td>
              <td className="col-balance">
                {formatCents(breakdown.total_assets_cents)}
              </td>
            </tr>
            <tr>
              <td>Liabilities</td>
              <td className="col-balance">
                {formatCents(breakdown.total_liabilities_cents)}
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <h2 className="section-heading">Trend (last 6 months)</h2>
      <div className="table-scroll">
        <table className="accounts-table">
          <thead>
            <tr>
              <th scope="col">Month</th>
              <th scope="col" className="col-balance">
                Net worth
              </th>
            </tr>
          </thead>
          <tbody>
            {history.map((point) => (
              <tr key={point.as_of_date}>
                <td>{point.as_of_date}</td>
                <td className="col-balance">
                  {formatCents(point.net_worth_cents)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <h2 className="section-heading">Account contributions this {mode}</h2>
      {change.contributions.length === 0 ? (
        <p className="muted">No account activity this {mode}.</p>
      ) : (
        <ul className="review-list">
          {change.contributions
            .filter((c) => c.change_cents !== 0)
            .map((c) => (
              <li
                key={c.account_id}
                className="review-item review-item-compact"
              >
                <span>{c.name}</span>
                <span
                  className={
                    c.change_cents >= 0 ? "amount-positive" : "amount-owed"
                  }
                >
                  {c.change_cents >= 0 ? "+" : ""}
                  {formatCents(c.change_cents)}
                </span>
              </li>
            ))}
        </ul>
      )}
    </div>
  );
}

// ---------- Page shell ----------

function InsightsPage() {
  const [tab, setTab] = useState<InsightsTab>("spending");
  const [mode, setMode] = useState<PeriodMode>("month");
  const [value, setValue] = useState(() => currentPeriodValue("month"));

  const handleModeChange = (newMode: PeriodMode) => {
    setMode(newMode);
    setValue(currentPeriodValue(newMode));
  };

  const comparison = periodComparison(mode, value);
  const previousValue = shiftPeriod(mode, value, -1);
  const compareSuffix = comparisonSuffix(
    mode,
    previousValue,
    comparison.isPartial,
  );

  return (
    <section aria-labelledby="insights-heading">
      <div className="accounts-toolbar">
        <h1 id="insights-heading">Insights</h1>
      </div>

      <PeriodNav
        mode={mode}
        value={value}
        onModeChange={handleModeChange}
        onValueChange={setValue}
      />

      <div className="insights-tabs" role="tablist" aria-label="Insights">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={tab === t.id}
            className={
              tab === t.id
                ? "insights-tab-button insights-tab-button-active"
                : "insights-tab-button"
            }
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </div>

      {tab === "spending" && (
        <SpendingTab
          from={comparison.currentFrom}
          to={comparison.currentTo}
          previousFrom={comparison.previousFrom}
          previousTo={comparison.previousTo}
          compareSuffix={compareSuffix}
          mode={mode}
        />
      )}
      {tab === "income" && (
        <IncomeTab
          from={comparison.currentFrom}
          to={comparison.currentTo}
          previousFrom={comparison.previousFrom}
          previousTo={comparison.previousTo}
          compareSuffix={compareSuffix}
          mode={mode}
        />
      )}
      {tab === "merchants" && (
        <MerchantsTab
          from={comparison.currentFrom}
          to={comparison.currentTo}
          previousFrom={comparison.previousFrom}
          previousTo={comparison.previousTo}
        />
      )}
      {tab === "cashflow" && (
        <CashFlowTab
          from={comparison.currentFrom}
          to={comparison.currentTo}
          mode={mode}
        />
      )}
      {tab === "networth" && (
        <NetWorthTab
          from={comparison.currentFrom}
          to={comparison.currentTo}
          mode={mode}
        />
      )}
    </section>
  );
}

export default InsightsPage;
