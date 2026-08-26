import { useEffect, useState } from "react";
import type { CategoryTotal, MonthlyFlow, Transaction } from "./api/midas";
import { reportsApi, transactionsApi } from "./api/midas";
import { formatCents } from "./lib/currency";
import { currentMonthValue, monthRange, shiftMonth } from "./lib/monthValue";

const monthFormatter = new Intl.DateTimeFormat("en-US", {
  month: "short",
  year: "numeric",
  timeZone: "UTC",
});

function monthLabel(isoDate: string): string {
  return monthFormatter.format(new Date(isoDate));
}

function CategoryDrilldown({
  category,
  from,
  to,
}: {
  category: string;
  from: string;
  to: string;
}) {
  const [transactions, setTransactions] = useState<Transaction[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setTransactions(null);
    setError(null);
    transactionsApi
      .list({
        category: category === "Uncategorized" ? null : category,
        date_from: from,
        date_to: to,
      })
      .then(setTransactions)
      .catch((err) => setError(String(err)));
  }, [category, from, to]);

  if (error) {
    return (
      <p className="form-error" role="alert">
        {error}
      </p>
    );
  }
  if (!transactions) return <p className="muted">Loading…</p>;
  if (transactions.length === 0) {
    return <p className="muted">No transactions found.</p>;
  }

  return (
    <div className="table-scroll">
      <table className="accounts-table">
        <tbody>
          {transactions.map((t) => (
            <tr key={t.id}>
              <td>{t.transaction_date}</td>
              <td>{t.merchant}</td>
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
  );
}

function ReportsPage() {
  const [monthValue, setMonthValue] = useState(currentMonthValue);
  const [categories, setCategories] = useState<CategoryTotal[]>([]);
  const [trend, setTrend] = useState<MonthlyFlow[]>([]);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const { from, to } = monthRange(monthValue);

  useEffect(() => {
    setLoading(true);
    setError(null);
    setExpanded(null);
    Promise.all([
      reportsApi.getSpendingByCategory(from, to),
      reportsApi.getIncomeVsSpendingTrend(),
    ])
      .then(([cats, flow]) => {
        setCategories(cats);
        setTrend(flow);
      })
      .catch((err) => setError(String(err)))
      .finally(() => setLoading(false));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [monthValue]);

  const totalSpent = categories.reduce((sum, c) => sum + c.spent_cents, 0);

  return (
    <section aria-labelledby="reports-heading">
      <div className="accounts-toolbar">
        <h1 id="reports-heading">Reports</h1>
        <div className="accounts-toolbar-actions">
          <button
            type="button"
            className="btn-secondary"
            onClick={() => setMonthValue((m) => shiftMonth(m, -1))}
          >
            ← Prior month
          </button>
          <strong>{monthLabel(`${monthValue}-01`)}</strong>
          <button
            type="button"
            className="btn-secondary"
            onClick={() => setMonthValue((m) => shiftMonth(m, 1))}
            disabled={monthValue >= currentMonthValue()}
          >
            Next month →
          </button>
        </div>
      </div>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      <h2 className="section-heading">Spending by category</h2>
      {loading ? (
        <p className="muted">Loading…</p>
      ) : categories.length === 0 ? (
        <p className="muted">No spending recorded this month.</p>
      ) : (
        <ul className="review-list">
          {categories.map((c) => (
            <li key={c.category} className="review-item">
              <button
                type="button"
                className="category-row"
                onClick={() =>
                  setExpanded(expanded === c.category ? null : c.category)
                }
                aria-expanded={expanded === c.category}
              >
                <span>{c.category}</span>
                <span>
                  {formatCents(c.spent_cents)}
                  <span className="muted">
                    {" "}
                    ({Math.round((c.spent_cents / (totalSpent || 1)) * 100)}%)
                  </span>
                </span>
              </button>
              {expanded === c.category && (
                <CategoryDrilldown category={c.category} from={from} to={to} />
              )}
            </li>
          ))}
        </ul>
      )}

      <h2 className="section-heading">Income vs. spending, last 6 months</h2>
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
                <td>{monthLabel(point.month)}</td>
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
    </section>
  );
}

export default ReportsPage;
