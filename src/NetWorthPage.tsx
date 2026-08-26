import { useEffect, useState } from "react";
import { accountingApi } from "./api/midas";
import type { NetWorthBreakdown, NetWorthPoint } from "./api/midas";
import { formatCents } from "./lib/currency";

const monthFormatter = new Intl.DateTimeFormat("en-US", {
  month: "short",
  year: "numeric",
  timeZone: "UTC",
});

function monthLabel(isoDate: string): string {
  return monthFormatter.format(new Date(isoDate));
}

function NetWorthPage() {
  const [breakdown, setBreakdown] = useState<NetWorthBreakdown | null>(null);
  const [history, setHistory] = useState<NetWorthPoint[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    (async () => {
      setLoading(true);
      setError(null);
      try {
        const [nw, hist] = await Promise.all([
          accountingApi.getNetWorth(),
          accountingApi.getNetWorthHistory(),
        ]);
        setBreakdown(nw);
        setHistory(hist);
      } catch (err) {
        setError(String(err));
      } finally {
        setLoading(false);
      }
    })();
  }, []);

  return (
    <section aria-labelledby="networth-heading">
      <div className="accounts-toolbar">
        <h1 id="networth-heading">Net Worth</h1>
      </div>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {loading ? (
        <p className="muted">Loading…</p>
      ) : (
        breakdown && (
          <>
            <p className="headline-number">
              {formatCents(breakdown.net_worth_cents)}
            </p>

            <div className="table-scroll">
              <table className="accounts-table">
                <tbody>
                  <tr>
                    <td>Total assets</td>
                    <td className="col-balance">
                      {formatCents(breakdown.total_assets_cents)}
                    </td>
                  </tr>
                  <tr>
                    <td>Total liabilities</td>
                    <td className="col-balance">
                      {formatCents(breakdown.total_liabilities_cents)}
                    </td>
                  </tr>
                  <tr>
                    <td>
                      <strong>Net worth</strong>
                    </td>
                    <td className="col-balance">
                      <strong>{formatCents(breakdown.net_worth_cents)}</strong>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>

            <h2 className="section-heading">Last 6 months</h2>
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
                      <td>{monthLabel(point.as_of_date)}</td>
                      <td className="col-balance">
                        {formatCents(point.net_worth_cents)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </>
        )
      )}
    </section>
  );
}

export default NetWorthPage;
