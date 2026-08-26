import { useEffect, useState } from "react";
import { safeToSpendApi } from "./api/midas";
import {
  centsToDollarInput,
  formatCents,
  parseDollarsToCents,
} from "./lib/currency";

function SafeToSpendPage() {
  const [result, setResult] = useState<Awaited<
    ReturnType<typeof safeToSpendApi.get>
  > | null>(null);
  const [reserveInput, setReserveInput] = useState("0.00");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [savingReserve, setSavingReserve] = useState(false);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      const [safeToSpend, reserveCents] = await Promise.all([
        safeToSpendApi.get(),
        safeToSpendApi.getReserve(),
      ]);
      setResult(safeToSpend);
      setReserveInput(centsToDollarInput(reserveCents));
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
  }, []);

  async function handleReserveSubmit(e: React.FormEvent) {
    e.preventDefault();
    const cents = parseDollarsToCents(reserveInput);
    if (cents === null || cents < 0) {
      setError("Reserve must be a non-negative number, e.g. 500.00.");
      return;
    }
    setSavingReserve(true);
    setError(null);
    try {
      await safeToSpendApi.setReserve(cents);
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setSavingReserve(false);
    }
  }

  return (
    <section aria-labelledby="safetospend-heading">
      <div className="accounts-toolbar">
        <h1 id="safetospend-heading">Safe to Spend</h1>
      </div>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {loading ? (
        <p className="muted">Loading…</p>
      ) : !result ? null : result.status === "incomplete" ? (
        <>
          <p className="muted">{result.reason}</p>
          <p className="muted">
            What MIDAS does know: {formatCents(result.eligible_cash_cents)} in
            eligible cash.
          </p>
        </>
      ) : (
        <>
          <p className="headline-number">
            {formatCents(result.safe_to_spend_cents)}
          </p>
          <p className="muted">Until your next payday, {result.next_payday}</p>

          <h2 className="section-heading">How this was calculated</h2>
          <div className="table-scroll">
            <table className="accounts-table">
              <tbody>
                <tr>
                  <td>Eligible cash</td>
                  <td className="col-balance">
                    {formatCents(result.eligible_cash_cents)}
                  </td>
                </tr>
                <tr>
                  <td>− Obligations before payday</td>
                  <td className="col-balance">
                    {formatCents(result.obligations_before_payday_cents)}
                  </td>
                </tr>
                <tr>
                  <td>− Reserved for savings goals</td>
                  <td className="col-balance">
                    {formatCents(result.reserved_goal_commitments_cents)}
                  </td>
                </tr>
                <tr>
                  <td>− Your required reserve</td>
                  <td className="col-balance">
                    {formatCents(result.owner_required_reserve_cents)}
                  </td>
                </tr>
                <tr>
                  <td>
                    <strong>= Safe to Spend</strong>
                  </td>
                  <td className="col-balance">
                    <strong>{formatCents(result.safe_to_spend_cents)}</strong>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </>
      )}

      <h2 className="section-heading">Your required reserve</h2>
      <p className="muted">
        An amount you never want counted as spendable, regardless of what's
        eligible - an emergency cushion, for example.
      </p>
      <form onSubmit={handleReserveSubmit} className="field mapping-row">
        <label htmlFor="reserve-amount">Reserve (USD)</label>
        <input
          id="reserve-amount"
          inputMode="decimal"
          value={reserveInput}
          onChange={(e) => setReserveInput(e.target.value)}
        />
        <button type="submit" className="btn-primary" disabled={savingReserve}>
          {savingReserve ? "Saving…" : "Save"}
        </button>
      </form>
    </section>
  );
}

export default SafeToSpendPage;
