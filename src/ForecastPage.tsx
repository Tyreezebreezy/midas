import { useEffect, useState } from "react";
import {
  accountsApi,
  forecastApi,
  type Account,
  type ForecastResult,
} from "./api/midas";
import { CASH_TYPES } from "./lib/accountTypes";
import { formatCents, useMasked } from "./lib/currency";
import { formatDate } from "./lib/dateFormat";

const HORIZONS = [7, 14, 30] as const;

function ForecastPage() {
  useMasked();
  const [accounts, setAccounts] = useState<Account[] | null>(null);
  const [selectedAccountId, setSelectedAccountId] = useState<
    number | "combined"
  >("combined");
  const [horizonDays, setHorizonDays] = useState<(typeof HORIZONS)[number]>(30);
  const [result, setResult] = useState<ForecastResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    accountsApi
      .list(false)
      .then((all) =>
        setAccounts(all.filter((a) => CASH_TYPES.has(a.account_type))),
      )
      .catch((err) => setError(String(err)));
  }, []);

  useEffect(() => {
    if (!accounts) return;
    const ids =
      selectedAccountId === "combined"
        ? accounts.map((a) => a.id)
        : [selectedAccountId];
    if (ids.length === 0) {
      setResult(null);
      return;
    }
    setError(null);
    forecastApi
      .getForecast(ids, horizonDays)
      .then(setResult)
      .catch((err) => setError(String(err)));
  }, [accounts, selectedAccountId, horizonDays]);

  if (error) {
    return (
      <section aria-labelledby="forecast-heading">
        <div className="page-toolbar">
          <h1 id="forecast-heading">Forecast</h1>
        </div>
        <p className="form-error" role="alert">
          {error}
        </p>
      </section>
    );
  }

  if (!accounts) {
    return (
      <section aria-labelledby="forecast-heading">
        <div className="page-toolbar">
          <h1 id="forecast-heading">Forecast</h1>
        </div>
        <p className="muted">Loading…</p>
      </section>
    );
  }

  if (accounts.length === 0) {
    return (
      <section aria-labelledby="forecast-heading">
        <div className="page-toolbar">
          <h1 id="forecast-heading">Forecast</h1>
        </div>
        <p className="muted">
          Add a checking, savings, or cash account to see a near-term cash
          forecast.
        </p>
      </section>
    );
  }

  return (
    <section aria-labelledby="forecast-heading">
      <div className="page-toolbar">
        <h1 id="forecast-heading">Forecast</h1>
      </div>
      <p className="muted">
        Based on known account balances and confirmed upcoming bills and income
        - never a guess about future discretionary spending.
      </p>

      <div className="insights-period-nav">
        <div className="page-toolbar-actions" role="group" aria-label="Account">
          <button
            type="button"
            aria-pressed={selectedAccountId === "combined"}
            className={
              selectedAccountId === "combined"
                ? "btn-secondary btn-mode-active"
                : "btn-secondary"
            }
            onClick={() => setSelectedAccountId("combined")}
          >
            All cash accounts
          </button>
          {accounts.map((a) => (
            <button
              key={a.id}
              type="button"
              aria-pressed={selectedAccountId === a.id}
              className={
                selectedAccountId === a.id
                  ? "btn-secondary btn-mode-active"
                  : "btn-secondary"
              }
              onClick={() => setSelectedAccountId(a.id)}
            >
              {a.name}
            </button>
          ))}
        </div>
        <div
          className="insights-mode-switch"
          role="group"
          aria-label="Forecast horizon"
        >
          {HORIZONS.map((h) => (
            <button
              key={h}
              type="button"
              aria-pressed={horizonDays === h}
              className={
                horizonDays === h
                  ? "btn-secondary btn-mode-active"
                  : "btn-secondary"
              }
              onClick={() => setHorizonDays(h)}
            >
              {h} days
            </button>
          ))}
        </div>
      </div>

      {!result ? (
        <p className="muted">Loading…</p>
      ) : (
        <>
          <div className="insights-cashflow-summary">
            <div>
              <p className="muted">Current cash</p>
              <p className="headline-number">
                {formatCents(result.current_balance_cents)}
              </p>
            </div>
            <div>
              <p className="muted">Projected in {horizonDays} days</p>
              <p className="headline-number">
                {formatCents(result.ending_balance_cents)}
              </p>
            </div>
            <div>
              <p className="muted">Lowest projected</p>
              <p className="headline-number">
                {formatCents(result.lowest_balance_cents)}
              </p>
              <p className="muted">
                on {formatDate(result.lowest_balance_date)}
              </p>
            </div>
          </div>

          {result.reserve_warning && (
            <p className="form-error" role="alert">
              Projected balance falls below your{" "}
              {formatCents(result.reserve_warning.reserve_cents)} reserve on{" "}
              {formatDate(result.reserve_warning.date)}.
            </p>
          )}

          <h2 className="section-heading">Upcoming events</h2>
          {result.events.length === 0 ? (
            <p className="muted">
              No known bills or income expected in the next {horizonDays} days.
            </p>
          ) : (
            <div className="table-scroll">
              <table className="accounts-table">
                <thead>
                  <tr>
                    <th scope="col">Date</th>
                    <th scope="col">Event</th>
                    <th scope="col" className="col-balance">
                      Amount
                    </th>
                    <th scope="col" className="col-balance">
                      Projected balance
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {result.events.map((event, i) => (
                    <tr
                      key={`${event.date}-${event.label}-${i}`}
                      className={
                        event.certainty === "estimated"
                          ? "forecast-event-estimated"
                          : undefined
                      }
                    >
                      <td>{formatDate(event.date)}</td>
                      <td>
                        {event.label}{" "}
                        <span className="muted">
                          (
                          {event.certainty === "estimated"
                            ? "Estimated"
                            : "Expected"}
                          )
                        </span>
                      </td>
                      <td className="col-balance">
                        <span
                          className={
                            event.amount_cents < 0
                              ? "amount-owed"
                              : "amount-positive"
                          }
                        >
                          {event.certainty === "estimated" ? "~" : ""}
                          {event.amount_cents >= 0 ? "+" : ""}
                          {formatCents(event.amount_cents)}
                        </span>
                      </td>
                      <td className="col-balance">
                        {formatCents(event.projected_balance_cents)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
    </section>
  );
}

export default ForecastPage;
