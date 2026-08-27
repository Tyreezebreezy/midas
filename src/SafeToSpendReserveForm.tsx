import { useEffect, useState } from "react";
import { safeToSpendApi } from "./api/midas";
import { centsToDollarInput, parseDollarsToCents } from "./lib/currency";

function SafeToSpendReserveForm() {
  const [reserveInput, setReserveInput] = useState("0.00");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      const reserveCents = await safeToSpendApi.getReserve();
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

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const cents = parseDollarsToCents(reserveInput);
    if (cents === null || cents < 0) {
      setError("Reserve must be a non-negative number, e.g. 500.00.");
      return;
    }
    setSaving(true);
    setError(null);
    setSaved(false);
    try {
      await safeToSpendApi.setReserve(cents);
      setSaved(true);
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div>
      <h3 className="settings-subheading">Required reserve</h3>
      <p className="muted">
        An amount you never want counted as spendable, regardless of what's
        eligible - an emergency cushion, for example.
      </p>
      {loading ? (
        <p className="muted">Loading…</p>
      ) : (
        <form onSubmit={handleSubmit} className="field mapping-row">
          <label htmlFor="reserve-amount">Reserve (USD)</label>
          <input
            id="reserve-amount"
            inputMode="decimal"
            value={reserveInput}
            onChange={(e) => {
              setReserveInput(e.target.value);
              setSaved(false);
            }}
          />
          <button type="submit" className="btn-primary" disabled={saving}>
            {saving ? "Saving…" : "Save"}
          </button>
        </form>
      )}
      {saved && <p className="muted">Saved.</p>}
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

export default SafeToSpendReserveForm;
