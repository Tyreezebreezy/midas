import { useEffect, useState } from "react";
import type { Transaction } from "./api/midas";
import { transactionsApi } from "./api/midas";
import { formatCents, useMasked } from "./lib/currency";
import { formatDate } from "./lib/dateFormat";

// Merchant Insights' own "see the real transactions" drilldown - the same
// `transactions::list` filter `CategoryDrilldown` already uses, just scoped
// by payee identity instead of category (falling back to a merchant-text
// search for a raw, unnormalized merchant with no canonical payee).
function MerchantDrilldown({
  payeeId,
  merchant,
  from,
  to,
}: {
  payeeId: number | null;
  merchant: string;
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
      .list(
        payeeId != null
          ? { payee_id: payeeId, date_from: from, date_to: to }
          : { search: merchant, date_from: from, date_to: to },
      )
      .then(setTransactions)
      .catch((err) => setError(String(err)));
  }, [payeeId, merchant, from, to]);

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
              <td>{t.category ?? "Uncategorized"}</td>
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

export default MerchantDrilldown;
