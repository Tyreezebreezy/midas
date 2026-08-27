import { useEffect, useState } from "react";
import type { Account, Cadence, RecurringSeries } from "./api/midas";
import { accountsApi, recurringApi } from "./api/midas";
import { formatCents } from "./lib/currency";

const CADENCE_LABELS: Record<Cadence, string> = {
  weekly: "Weekly",
  biweekly: "Every 2 weeks",
  monthly: "Monthly",
  quarterly: "Quarterly",
  annual: "Annually",
};

function RecurringPage() {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [accountId, setAccountId] = useState<number | "">("");
  const [series, setSeries] = useState<RecurringSeries[]>([]);
  const [loading, setLoading] = useState(true);
  const [detecting, setDetecting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [detectMessage, setDetectMessage] = useState<string | null>(null);

  useEffect(() => {
    accountsApi
      .list(false)
      .then((list) => {
        setAccounts(list);
        if (list.length > 0) setAccountId(list[0].id);
      })
      .catch((err) => setError(String(err)));
  }, []);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      setSeries(await recurringApi.list(accountId === "" ? null : accountId));
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accountId]);

  async function handleDetect() {
    if (accountId === "") return;
    setDetecting(true);
    setDetectMessage(null);
    setError(null);
    try {
      const found = await recurringApi.detect(accountId);
      setDetectMessage(
        found.length === 0
          ? "No new recurring patterns found."
          : `Found ${found.length} new recurring pattern${found.length === 1 ? "" : "s"} to review below.`,
      );
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setDetecting(false);
    }
  }

  async function handleConfirm(id: number) {
    try {
      await recurringApi.confirm(id);
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  async function handleReject(id: number) {
    try {
      await recurringApi.reject(id);
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  const proposed = series.filter((s) => s.status === "proposed");
  const confirmed = series
    .filter((s) => s.status === "confirmed")
    .sort((a, b) => a.next_expected_date.localeCompare(b.next_expected_date));

  return (
    <section aria-labelledby="recurring-heading">
      <div className="accounts-toolbar">
        <h1 id="recurring-heading">Bills</h1>
        <div className="accounts-toolbar-actions">
          <select
            value={accountId}
            onChange={(e) =>
              setAccountId(e.target.value === "" ? "" : Number(e.target.value))
            }
            aria-label="Account"
          >
            {accounts.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name}
              </option>
            ))}
          </select>
          <button
            type="button"
            className="btn-primary"
            onClick={handleDetect}
            disabled={detecting || accountId === ""}
          >
            {detecting ? "Scanning…" : "Detect recurring"}
          </button>
        </div>
      </div>

      {detectMessage && <p className="muted">{detectMessage}</p>}
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {loading ? (
        <p className="muted">Loading…</p>
      ) : (
        <>
          <h2 className="section-heading">Upcoming bills</h2>
          {confirmed.length === 0 ? (
            <p className="muted">No confirmed bills yet.</p>
          ) : (
            <div className="table-scroll">
              <table className="accounts-table">
                <thead>
                  <tr>
                    <th scope="col">Merchant</th>
                    <th scope="col">Cadence</th>
                    <th scope="col" className="col-balance">
                      Expected
                    </th>
                    <th scope="col">Next due</th>
                    <th scope="col">Last occurrence</th>
                    <th scope="col">
                      <span className="sr-only">Flags</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {confirmed.map((s) => (
                    <tr key={s.id}>
                      <td>{s.merchant}</td>
                      <td>{CADENCE_LABELS[s.cadence]}</td>
                      <td className="col-balance">
                        {formatCents(s.expected_amount_cents)}
                      </td>
                      <td>{s.next_expected_date}</td>
                      <td>
                        {s.last_occurrence_date ?? (
                          <span className="muted">—</span>
                        )}
                      </td>
                      <td>
                        {s.is_possibly_missed && (
                          <span className="badge-review">Possibly missed</span>
                        )}
                        {s.is_price_changed && (
                          <span className="badge-review">Price changed</span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          <h2 className="section-heading">Proposed</h2>
          {proposed.length === 0 ? (
            <p className="muted">
              Nothing pending. Run "Detect recurring" after you've imported a
              few months of history.
            </p>
          ) : (
            <ul className="review-list">
              {proposed.map((s) => (
                <li key={s.id} className="review-item">
                  <div className="review-item-header">
                    <strong>{s.merchant}</strong>
                    <span className="muted">{CADENCE_LABELS[s.cadence]}</span>
                    <span>{formatCents(s.expected_amount_cents)}</span>
                  </div>
                  <p className="muted">Next expected {s.next_expected_date}</p>
                  <div className="review-item-actions">
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => handleConfirm(s.id)}
                    >
                      Confirm
                    </button>
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => handleReject(s.id)}
                    >
                      Reject
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </section>
  );
}

export default RecurringPage;
