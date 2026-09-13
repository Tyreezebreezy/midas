import { useEffect, useState } from "react";
import type { Transaction } from "./api/midas";
import { transactionsApi } from "./api/midas";
import { formatCents, useMasked } from "./lib/currency";
import { formatDate } from "./lib/dateFormat";

// Shared by Reports and Budgets - "see the real transactions behind a
// total" is the same drilldown either page wants, reusing
// `transactions::list`'s existing category+date filter rather than a
// second, redundant transaction-browsing backend or component.
function CategoryDrilldown({
  category,
  from,
  to,
}: {
  category: string;
  from: string;
  to: string;
}) {
  useMasked();
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
              <td>{formatDate(t.transaction_date)}</td>
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

export default CategoryDrilldown;
