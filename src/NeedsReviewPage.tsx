import { useEffect, useState } from "react";
import type {
  Account,
  IssueType,
  NeedsReviewItem,
  Transaction,
} from "./api/midas";
import { accountsApi, needsReviewApi, transactionsApi } from "./api/midas";
import TransactionForm from "./TransactionForm";

const ISSUE_TYPE_LABELS: Record<IssueType, string> = {
  malformed_row: "Malformed row",
  missing_required_field: "Missing required field",
  uncertain_category: "Uncertain category",
  uncertain_merchant: "Uncertain merchant",
  suspected_duplicate: "Suspected duplicate",
  ambiguous_debit_credit: "Ambiguous debit/credit",
  ambiguous_transfer: "Ambiguous transfer",
  other: "Other",
};

function formatSourceRow(json: string | null): string {
  if (!json) return "";
  try {
    const parsed = JSON.parse(json) as Record<string, string>;
    return Object.entries(parsed)
      .map(([k, v]) => `${k}: ${v}`)
      .join(" · ");
  } catch {
    return json;
  }
}

function NeedsReviewPage() {
  const [items, setItems] = useState<NeedsReviewItem[]>([]);
  const [showAll, setShowAll] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [fixing, setFixing] = useState<Transaction | null>(null);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      setItems(await needsReviewApi.list(!showAll));
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showAll]);

  useEffect(() => {
    accountsApi
      .list(true)
      .then(setAccounts)
      .catch((err) => setError(String(err)));
  }, []);

  async function resolve(id: number, action: "resolved" | "dismissed") {
    try {
      await needsReviewApi.resolve(id, action);
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  async function handleFix(transactionId: number) {
    setError(null);
    try {
      const all = await transactionsApi.list({ include_archived: true });
      const match = all.find((t) => t.id === transactionId);
      if (!match) {
        setError("That transaction no longer exists.");
        return;
      }
      setFixing(match);
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <section aria-labelledby="review-heading">
      <div className="accounts-toolbar">
        <h1 id="review-heading">Needs Review</h1>
        <label className="show-archived-toggle">
          <input
            type="checkbox"
            checked={showAll}
            onChange={(e) => setShowAll(e.target.checked)}
          />
          Show resolved and dismissed
        </label>
      </div>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {loading ? (
        <p className="muted">Loading…</p>
      ) : items.length === 0 ? (
        <p className="muted">Nothing needs your attention right now.</p>
      ) : (
        <ul className="review-list">
          {items.map((item) => (
            <li
              key={item.id}
              className={`review-item review-item-${item.resolution_status}`}
            >
              <div className="review-item-header">
                <span className="badge-review">
                  {ISSUE_TYPE_LABELS[item.issue_type]}
                </span>
                <span className="muted">{item.created_at.slice(0, 10)}</span>
                {item.resolution_status !== "open" && (
                  <span className="badge-archived">
                    {item.resolution_status}
                  </span>
                )}
              </div>
              <p>{item.detected_reason}</p>
              {item.source_row_json && (
                <p className="review-source muted">
                  {formatSourceRow(item.source_row_json)}
                </p>
              )}
              <div className="review-item-actions">
                {item.transaction_id !== null && (
                  <button
                    type="button"
                    className="btn-link"
                    onClick={() => handleFix(item.transaction_id!)}
                  >
                    Fix transaction
                  </button>
                )}
                {item.resolution_status === "open" && (
                  <>
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => resolve(item.id, "resolved")}
                    >
                      Mark resolved
                    </button>
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => resolve(item.id, "dismissed")}
                    >
                      Dismiss
                    </button>
                  </>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}

      {fixing && (
        <TransactionForm
          accounts={accounts}
          defaultAccountId={fixing.account_id}
          transaction={fixing}
          onClose={() => setFixing(null)}
          onSaved={() => {
            setFixing(null);
            refresh();
          }}
        />
      )}
    </section>
  );
}

export default NeedsReviewPage;
