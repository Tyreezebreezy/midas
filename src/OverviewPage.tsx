import { useEffect, useState } from "react";
import type { Tab } from "./App";
import type { OverviewResult } from "./api/midas";
import { overviewApi } from "./api/midas";
import { formatCents } from "./lib/currency";
import { SCHEDULE_TYPE_LABELS } from "./PayCycleSettings";
import { TRANSACTION_TYPE_LABELS } from "./lib/transactionTypes";

function OverviewPage({ onNavigate }: { onNavigate: (tab: Tab) => void }) {
  const [overview, setOverview] = useState<OverviewResult | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    overviewApi
      .get()
      .then(setOverview)
      .catch((err) => setError(String(err)))
      .finally(() => setLoading(false));
  }, []);

  if (loading) {
    return (
      <section aria-labelledby="overview-heading">
        <h1 id="overview-heading">Overview</h1>
        <p className="muted">Loading…</p>
      </section>
    );
  }

  if (error || !overview) {
    return (
      <section aria-labelledby="overview-heading">
        <h1 id="overview-heading">Overview</h1>
        <p className="form-error" role="alert">
          {error ?? "Could not load the overview."}
        </p>
      </section>
    );
  }

  const sts = overview.safe_to_spend;

  return (
    <section aria-labelledby="overview-heading">
      <h1 id="overview-heading">Overview</h1>

      <div className="overview-hero">
        <h2>Safe to spend</h2>
        {sts.status === "complete" ? (
          <>
            <p className="hero-number">
              {formatCents(sts.safe_to_spend_cents)}
            </p>
            <p className="muted">Until your next payday, {sts.next_payday}</p>
            <p className="hero-summary-line">
              {formatCents(sts.eligible_cash_cents)} cash −{" "}
              {formatCents(sts.obligations_before_payday_cents)} bills −{" "}
              {formatCents(
                sts.reserved_goal_commitments_cents +
                  sts.owner_required_reserve_cents,
              )}{" "}
              reserved
            </p>
            <details className="hero-breakdown">
              <summary>View breakdown</summary>
              <div className="table-scroll">
                <table className="accounts-table">
                  <tbody>
                    <tr>
                      <td>Eligible cash</td>
                      <td className="col-balance">
                        {formatCents(sts.eligible_cash_cents)}
                      </td>
                    </tr>
                    <tr>
                      <td>− Obligations before payday</td>
                      <td className="col-balance">
                        {formatCents(sts.obligations_before_payday_cents)}
                      </td>
                    </tr>
                    <tr>
                      <td>− Reserved for savings goals</td>
                      <td className="col-balance">
                        {formatCents(sts.reserved_goal_commitments_cents)}
                      </td>
                    </tr>
                    <tr>
                      <td>− Your required reserve</td>
                      <td className="col-balance">
                        {formatCents(sts.owner_required_reserve_cents)}
                      </td>
                    </tr>
                    <tr>
                      <td>
                        <strong>= Safe to Spend</strong>
                      </td>
                      <td className="col-balance">
                        <strong>{formatCents(sts.safe_to_spend_cents)}</strong>
                      </td>
                    </tr>
                  </tbody>
                </table>
              </div>
            </details>
          </>
        ) : (
          <>
            <p className="hero-number">
              {formatCents(sts.eligible_cash_cents)}
            </p>
            <p className="muted">Eligible cash - no pay schedule set yet</p>
            <p className="hero-summary-line">
              <button
                type="button"
                className="btn-link"
                onClick={() => onNavigate("settings")}
              >
                Set your pay schedule →
              </button>
            </p>
          </>
        )}
      </div>

      <div className="overview-grid overview-grid-primary">
        <div className="overview-card">
          <h2>Upcoming bills</h2>
          {overview.upcoming_bills.length === 0 ? (
            <p className="overview-card-empty">No upcoming bills.</p>
          ) : (
            <ul className="overview-mini-list">
              {overview.upcoming_bills.map((bill) => (
                <li key={bill.id}>
                  <span>{bill.merchant}</span>
                  <span>
                    {formatCents(bill.expected_amount_cents)}
                    <span className="muted"> · {bill.next_expected_date}</span>
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>

        <div className="overview-card">
          <h2>Current pay cycle</h2>
          {overview.pay_cycle ? (
            <>
              <p className="headline-number">
                {overview.pay_cycle.next_payday}
              </p>
              <p className="muted">
                Next payday ·{" "}
                {
                  SCHEDULE_TYPE_LABELS[
                    overview.pay_cycle.schedule.schedule_type
                  ]
                }
              </p>
            </>
          ) : (
            <p className="overview-card-empty">
              <button
                type="button"
                className="btn-link"
                onClick={() => onNavigate("settings")}
              >
                Set your pay schedule →
              </button>
            </p>
          )}
        </div>

        <div className="overview-card">
          <h2>Spending</h2>
          <p className="headline-number">
            {formatCents(overview.spending_this_month_cents)}
          </p>
          <p className="muted">
            {overview.pay_cycle
              ? `${formatCents(overview.pay_cycle.spending_this_cycle_cents)} this pay cycle`
              : "This month so far"}
          </p>
          <p className="muted">
            Income this month: {formatCents(overview.income_this_month_cents)}
          </p>
        </div>
      </div>

      <div className="overview-grid">
        <div className="overview-card">
          <h2>Needs review</h2>
          <p className="headline-number">{overview.needs_review_count}</p>
          {overview.needs_review_items.length === 0 ? (
            <p className="overview-card-empty">Nothing needs review.</p>
          ) : (
            <ul className="overview-mini-list">
              {overview.needs_review_items.slice(0, 3).map((item) => (
                <li key={item.id}>
                  <span>{item.detected_reason}</span>
                </li>
              ))}
            </ul>
          )}
        </div>

        <div className="overview-card">
          <h2>Savings goals</h2>
          {overview.savings_goals.length === 0 ? (
            <p className="overview-card-empty">
              <button
                type="button"
                className="btn-link"
                onClick={() => onNavigate("goals")}
              >
                Set a savings goal →
              </button>
            </p>
          ) : (
            <ul className="overview-mini-list">
              {overview.savings_goals.map((goal) => (
                <li key={goal.id}>
                  <span>{goal.name}</span>
                  <span>
                    {formatCents(goal.current_amount_cents)} /{" "}
                    {formatCents(goal.target_amount_cents)}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>

      <h2 className="section-heading">Cash position and net worth</h2>
      <dl className="stat-strip">
        <div className="stat-strip-item">
          <dt>Eligible cash</dt>
          <dd>{formatCents(sts.eligible_cash_cents)}</dd>
        </div>
        <div className="stat-strip-item">
          <dt>Total assets</dt>
          <dd>{formatCents(overview.net_worth.total_assets_cents)}</dd>
        </div>
        <div className="stat-strip-item">
          <dt>Total liabilities</dt>
          <dd>{formatCents(overview.net_worth.total_liabilities_cents)}</dd>
        </div>
        <div className="stat-strip-item">
          <dt>Net worth</dt>
          <dd>{formatCents(overview.net_worth.net_worth_cents)}</dd>
        </div>
      </dl>

      <h2 className="section-heading">Budget health</h2>
      {overview.budget_health.length === 0 ? (
        <p className="muted">
          <button
            type="button"
            className="btn-link"
            onClick={() => onNavigate("budgets")}
          >
            Set up a budget →
          </button>
        </p>
      ) : (
        <div className="table-scroll">
          <table className="accounts-table">
            <thead>
              <tr>
                <th scope="col">Category</th>
                <th scope="col" className="col-balance">
                  Planned
                </th>
                <th scope="col" className="col-balance">
                  Actual
                </th>
                <th scope="col">Status</th>
              </tr>
            </thead>
            <tbody>
              {overview.budget_health.map((b) => (
                <tr key={b.id}>
                  <td>{b.category}</td>
                  <td className="col-balance">
                    {formatCents(b.planned_amount_cents)}
                  </td>
                  <td className="col-balance">
                    {formatCents(b.actual_amount_cents)}
                  </td>
                  <td>
                    {b.status === "over" ? (
                      <span className="badge-review">Over</span>
                    ) : b.status === "near" ? (
                      <span className="badge-review">Near</span>
                    ) : (
                      <span className="badge-valid">Under</span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <h2 className="section-heading">Recent transactions</h2>
      {overview.recent_transactions.length === 0 ? (
        <p className="muted">No transactions yet.</p>
      ) : (
        <div className="table-scroll">
          <table className="accounts-table">
            <thead>
              <tr>
                <th scope="col">Date</th>
                <th scope="col">Merchant</th>
                <th scope="col">Type</th>
                <th scope="col" className="col-balance">
                  Amount
                </th>
              </tr>
            </thead>
            <tbody>
              {overview.recent_transactions.map((t) => (
                <tr key={t.id}>
                  <td>{t.transaction_date}</td>
                  <td>{t.merchant}</td>
                  <td>{TRANSACTION_TYPE_LABELS[t.transaction_type]}</td>
                  <td className="col-balance">
                    <span
                      className={
                        t.amount_cents < 0 ? "amount-owed" : "amount-positive"
                      }
                    >
                      {formatCents(t.amount_cents)}
                    </span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

export default OverviewPage;
