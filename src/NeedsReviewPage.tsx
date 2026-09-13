import { useEffect, useState } from "react";
import type {
  Account,
  IssueType,
  NeedsReviewItem,
  RecurringOccurrence,
  Transaction,
} from "./api/midas";
import {
  accountsApi,
  needsReviewApi,
  recurringApi,
  transactionsApi,
} from "./api/midas";
import { formatCents } from "./lib/currency";
import { formatDate } from "./lib/dateFormat";
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

// A handful of review items carry a machine-readable proposal in
// proposed_resolution_json (recurring_occurrences.rs / payees.rs) instead of
// (or alongside) a plain-English detected_reason - parsed defensively since
// it's untyped JSON coming from the database, not a Rust-checked shape.
function parseAmbiguousRecurringMatch(json: string | null): number[] | null {
  if (!json) return null;
  try {
    const parsed = JSON.parse(json) as {
      kind?: string;
      occurrence_ids?: unknown;
    };
    if (
      parsed.kind === "ambiguous_recurring_match" &&
      Array.isArray(parsed.occurrence_ids)
    ) {
      return parsed.occurrence_ids.filter(
        (id): id is number => typeof id === "number",
      );
    }
  } catch {
    // Not JSON, or not this shape - render as plain text instead.
  }
  return null;
}

function AmbiguousRecurringMatchResolver({
  transactionId,
  occurrenceIds,
  onLinked,
}: {
  transactionId: number;
  occurrenceIds: number[];
  onLinked: () => void;
}) {
  const [candidates, setCandidates] = useState<RecurringOccurrence[] | null>(
    null,
  );
  const [linking, setLinking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    Promise.all(occurrenceIds.map((id) => recurringApi.getOccurrence(id)))
      .then((results) =>
        setCandidates(
          results.filter((o): o is RecurringOccurrence => o !== null),
        ),
      )
      .catch((err) => setError(String(err)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function pick(occurrenceId: number) {
    setLinking(true);
    setError(null);
    try {
      await recurringApi.linkOccurrence(occurrenceId, transactionId);
      onLinked();
    } catch (err) {
      setError(String(err));
    } finally {
      setLinking(false);
    }
  }

  if (error) {
    return (
      <p className="form-error" role="alert">
        {error}
      </p>
    );
  }
  if (candidates === null) {
    return <p className="muted">Loading candidates…</p>;
  }

  return (
    <div className="review-item-actions ambiguous-match-choices">
      {candidates.map((c) => (
        <button
          key={c.id}
          type="button"
          className="btn-secondary"
          disabled={linking}
          onClick={() => pick(c.id)}
        >
          This is {formatDate(c.expected_date)} ·{" "}
          {formatCents(Math.abs(c.expected_amount_cents))}
        </button>
      ))}
    </div>
  );
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
          {items.map((item) => {
            const ambiguousOccurrenceIds = parseAmbiguousRecurringMatch(
              item.proposed_resolution_json,
            );
            return (
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
                {item.resolution_status === "open" &&
                  ambiguousOccurrenceIds !== null &&
                  item.transaction_id !== null && (
                    <AmbiguousRecurringMatchResolver
                      transactionId={item.transaction_id}
                      occurrenceIds={ambiguousOccurrenceIds}
                      onLinked={() => resolve(item.id, "resolved")}
                    />
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
            );
          })}
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
