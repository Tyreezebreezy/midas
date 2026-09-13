import { useEffect, useState } from "react";
import type { Tab } from "./App";
import type {
  Account,
  DataHealth,
  RecurringOccurrence,
  ReviewDomain,
  ReviewItem,
  Signal,
  Transaction,
} from "./api/midas";
import {
  accountsApi,
  dataHealthApi,
  needsReviewApi,
  recurringApi,
  reviewApi,
  signalsApi,
  transactionsApi,
} from "./api/midas";
import { formatCents, maskMoneyInText, useMasked } from "./lib/currency";
import { formatDate } from "./lib/dateFormat";
import TransactionForm from "./TransactionForm";

type ReviewTab = "attention" | "signals" | "health";

const DOMAIN_LABELS: Record<ReviewDomain, string> = {
  transactions: "Transactions",
  imports: "Imports",
  recurring: "Recurring",
  reconciliation: "Reconciliation",
};

const DOMAIN_FILTERS: (ReviewDomain | "all")[] = [
  "all",
  "transactions",
  "imports",
  "recurring",
  "reconciliation",
];

// --- Ambiguous recurring match: pick which expected occurrence this
// transaction actually fulfills (reuses Batch 6's own linking commands,
// never a second matching implementation, section 17). ---
function AmbiguousMatchResolver({
  transactionId,
  occurrenceIds,
  onResolved,
}: {
  transactionId: number;
  occurrenceIds: number[];
  onResolved: () => void;
}) {
  const [candidates, setCandidates] = useState<RecurringOccurrence[] | null>(
    null,
  );
  const [busy, setBusy] = useState(false);
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
    setBusy(true);
    setError(null);
    try {
      await recurringApi.linkOccurrence(occurrenceId, transactionId);
      onResolved();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  if (error) {
    return (
      <p className="form-error" role="alert">
        {error}
      </p>
    );
  }
  if (!candidates) return <p className="muted">Loading candidates…</p>;

  return (
    <div className="review-item-actions">
      {candidates.map((c) => (
        <button
          key={c.id}
          type="button"
          className="btn-secondary"
          disabled={busy}
          onClick={() => pick(c.id)}
        >
          This is {formatDate(c.expected_date)} ·{" "}
          {formatCents(Math.abs(c.expected_amount_cents))}
        </button>
      ))}
    </div>
  );
}

function ReviewItemCard({
  item,
  onChanged,
  onOpenTransaction,
  onNavigate,
}: {
  item: ReviewItem;
  onChanged: () => void;
  onOpenTransaction: (transactionId: number) => void;
  onNavigate: (tab: Tab) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function run(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
      onChanged();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <li className="review-item">
      <div className="review-item-header">
        <span className={`badge-review badge-priority-${item.priority}`}>
          {item.priority} priority
        </span>
        <span className="muted">{DOMAIN_LABELS[item.domain]}</span>
      </div>
      <p>
        <strong>{item.title}</strong>
      </p>
      <p className="muted">{maskMoneyInText(item.evidence)}</p>
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {item.kind === "uncategorized" && item.transaction_id !== null && (
        <div className="review-item-actions">
          <button
            type="button"
            className="btn-secondary"
            onClick={() => onOpenTransaction(item.transaction_id!)}
          >
            Categorize
          </button>
        </div>
      )}

      {item.kind === "malformed_row" && item.needs_review_item_id !== null && (
        <div className="review-item-actions">
          <button
            type="button"
            className="btn-link"
            disabled={busy}
            onClick={() =>
              run(() =>
                needsReviewApi.resolve(item.needs_review_item_id!, "dismissed"),
              )
            }
          >
            Dismiss
          </button>
        </div>
      )}

      {item.kind === "suspected_duplicate" &&
        item.needs_review_item_id !== null && (
          <div className="review-item-actions">
            <button
              type="button"
              className="btn-secondary"
              disabled={busy}
              onClick={() =>
                run(() => reviewApi.keepDuplicate(item.needs_review_item_id!))
              }
            >
              Keep Both
            </button>
            <button
              type="button"
              className="btn-link"
              disabled={busy}
              onClick={() =>
                run(() =>
                  needsReviewApi.resolve(
                    item.needs_review_item_id!,
                    "dismissed",
                  ),
                )
              }
            >
              Mark Duplicate
            </button>
            {item.duplicate_of_transaction_id !== null && (
              <button
                type="button"
                className="btn-link"
                onClick={() =>
                  onOpenTransaction(item.duplicate_of_transaction_id!)
                }
              >
                View existing transaction
              </button>
            )}
          </div>
        )}

      {item.kind === "ambiguous_recurring_match" &&
        item.transaction_id !== null &&
        item.occurrence_ids.length > 0 && (
          <>
            <AmbiguousMatchResolver
              transactionId={item.transaction_id}
              occurrenceIds={item.occurrence_ids}
              onResolved={onChanged}
            />
            {item.needs_review_item_id !== null && (
              <div className="review-item-actions">
                <button
                  type="button"
                  className="btn-link"
                  disabled={busy}
                  onClick={() =>
                    run(() =>
                      needsReviewApi.resolve(
                        item.needs_review_item_id!,
                        "dismissed",
                      ),
                    )
                  }
                >
                  Neither
                </button>
              </div>
            )}
          </>
        )}

      {item.kind === "payee_merge_recurring_conflict" &&
        item.needs_review_item_id !== null && (
          <div className="review-item-actions">
            <button
              type="button"
              className="btn-secondary"
              onClick={() => onNavigate("bills")}
            >
              View Recurring
            </button>
            <button
              type="button"
              className="btn-link"
              disabled={busy}
              onClick={() =>
                run(() =>
                  needsReviewApi.resolve(
                    item.needs_review_item_id!,
                    "resolved",
                  ),
                )
              }
            >
              Mark resolved
            </button>
          </div>
        )}

      {item.kind === "recurring_proposed" && item.series_id !== null && (
        <div className="review-item-actions">
          <button
            type="button"
            className="btn-secondary"
            disabled={busy}
            onClick={() => run(() => recurringApi.confirm(item.series_id!))}
          >
            Confirm
          </button>
          <button
            type="button"
            className="btn-link"
            disabled={busy}
            onClick={() => run(() => recurringApi.reject(item.series_id!))}
          >
            Not Recurring
          </button>
        </div>
      )}

      {item.kind === "recurring_missed" && item.series_id !== null && (
        <div className="review-item-actions">
          {item.occurrence_ids[0] !== undefined && (
            <button
              type="button"
              className="btn-secondary"
              disabled={busy}
              onClick={() =>
                run(() => recurringApi.skipOccurrence(item.occurrence_ids[0]))
              }
            >
              Mark Skipped
            </button>
          )}
          <button
            type="button"
            className="btn-link"
            disabled={busy}
            onClick={() => run(() => recurringApi.setInactive(item.series_id!))}
          >
            Mark Pattern Inactive
          </button>
          <button
            type="button"
            className="btn-link"
            onClick={() => onNavigate("bills")}
          >
            View Recurring
          </button>
        </div>
      )}

      {item.kind === "reconciliation_needs_review" && (
        <div className="review-item-actions">
          <button
            type="button"
            className="btn-secondary"
            onClick={() => onNavigate("accounts")}
          >
            Review Reconciliation
          </button>
        </div>
      )}
    </li>
  );
}

function NeedsAttentionTab({ onNavigate }: { onNavigate: (tab: Tab) => void }) {
  const [items, setItems] = useState<ReviewItem[] | null>(null);
  const [filter, setFilter] = useState<ReviewDomain | "all">("all");
  const [error, setError] = useState<string | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [editing, setEditing] = useState<Transaction | null>(null);

  async function refresh() {
    try {
      setItems(await reviewApi.list());
    } catch (err) {
      setError(String(err));
    }
  }

  useEffect(() => {
    refresh();
    accountsApi
      .list(true)
      .then(setAccounts)
      .catch((err) => setError(String(err)));
  }, []);

  async function openTransaction(transactionId: number) {
    setError(null);
    try {
      const all = await transactionsApi.list({ include_archived: true });
      const match = all.find((t) => t.id === transactionId);
      if (!match) {
        setError("That transaction no longer exists.");
        return;
      }
      setEditing(match);
    } catch (err) {
      setError(String(err));
    }
  }

  if (error) {
    return (
      <p className="form-error" role="alert">
        {error}
      </p>
    );
  }
  if (!items) return <p className="muted">Loading…</p>;

  const filtered =
    filter === "all" ? items : items.filter((i) => i.domain === filter);

  return (
    <div>
      <div
        className="review-filters"
        role="group"
        aria-label="Filter by domain"
      >
        {DOMAIN_FILTERS.map((f) => (
          <button
            key={f}
            type="button"
            aria-pressed={filter === f}
            className={
              filter === f ? "btn-secondary btn-mode-active" : "btn-secondary"
            }
            onClick={() => setFilter(f)}
          >
            {f === "all" ? "All" : DOMAIN_LABELS[f]}
          </button>
        ))}
      </div>

      {filtered.length === 0 ? (
        <div className="review-empty-state">
          <h2>You're caught up</h2>
          <p className="muted">No items need your attention right now.</p>
        </div>
      ) : (
        <ul className="review-list">
          {filtered.map((item) => (
            <ReviewItemCard
              key={item.key}
              item={item}
              onChanged={refresh}
              onOpenTransaction={openTransaction}
              onNavigate={onNavigate}
            />
          ))}
        </ul>
      )}

      {editing && (
        <TransactionForm
          accounts={accounts}
          defaultAccountId={editing.account_id}
          transaction={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            refresh();
          }}
        />
      )}
    </div>
  );
}

const SEVERITY_LABELS: Record<Signal["severity"], string> = {
  info: "Info",
  attention: "Attention",
  important: "Important",
};

const NAVIGATION_TARGETS: Record<Signal["navigation"], Tab> = {
  insights: "insights",
  budgets: "budgets",
  forecast: "forecast",
  recurring: "bills",
};

const NAVIGATION_LABELS: Record<Signal["navigation"], string> = {
  insights: "Insights",
  budgets: "Budget",
  forecast: "Forecast",
  recurring: "Recurring",
};

function SignalsTab({ onNavigate }: { onNavigate: (tab: Tab) => void }) {
  const [signals, setSignals] = useState<Signal[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    signalsApi
      .list()
      .then(setSignals)
      .catch((err) => setError(String(err)));
  }, []);

  if (error) {
    return (
      <p className="form-error" role="alert">
        {error}
      </p>
    );
  }
  if (!signals) return <p className="muted">Loading…</p>;
  if (signals.length === 0) {
    return (
      <div className="review-empty-state">
        <h2>Nothing notable</h2>
        <p className="muted">No financial signals right now.</p>
      </div>
    );
  }

  return (
    <ul className="review-list">
      {signals.map((s) => (
        <li key={s.key} className="review-item">
          <div className="review-item-header">
            <span className={`badge-review badge-severity-${s.severity}`}>
              {SEVERITY_LABELS[s.severity]}
            </span>
          </div>
          <p>
            <strong>{s.title}</strong>
          </p>
          <p className="muted">{maskMoneyInText(s.evidence)}</p>
          <div className="review-item-actions">
            <button
              type="button"
              className="btn-link"
              onClick={() => onNavigate(NAVIGATION_TARGETS[s.navigation])}
            >
              View {NAVIGATION_LABELS[s.navigation]} →
            </button>
          </div>
        </li>
      ))}
    </ul>
  );
}

const HEALTH_STATE_LABELS: Record<DataHealth["transactions"]["state"], string> =
  {
    good: "Good",
    needs_attention: "Needs Attention",
  };

function HealthRow({
  label,
  state,
  lines,
}: {
  label: string;
  state: DataHealth["transactions"]["state"];
  lines: string[];
}) {
  return (
    <div className="review-item data-health-row">
      <div className="review-item-header">
        <strong>{label}</strong>
        <span className={`badge-review badge-health-${state}`}>
          {HEALTH_STATE_LABELS[state]}
        </span>
      </div>
      {lines.map((line) => (
        <p key={line} className="muted">
          {line}
        </p>
      ))}
    </div>
  );
}

function DataHealthTab() {
  const [health, setHealth] = useState<DataHealth | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    dataHealthApi
      .get()
      .then(setHealth)
      .catch((err) => setError(String(err)));
  }, []);

  if (error) {
    return (
      <p className="form-error" role="alert">
        {error}
      </p>
    );
  }
  if (!health) return <p className="muted">Loading…</p>;

  return (
    <div className="data-health-grid">
      <HealthRow
        label="Transactions"
        state={health.transactions.state}
        lines={[
          health.transactions.uncategorized_count === 0
            ? "All categorized"
            : `${health.transactions.uncategorized_count} uncategorized`,
          health.transactions.possible_duplicate_count === 0
            ? "0 possible duplicates"
            : `${health.transactions.possible_duplicate_count} possible duplicates`,
        ]}
      />
      <HealthRow
        label="Imports"
        state={health.imports.state}
        lines={[
          health.imports.last_import_date
            ? `Last import: ${formatDate(health.imports.last_import_date)}`
            : "No imports yet",
          `Unresolved: ${health.imports.unresolved_count}`,
        ]}
      />
      <HealthRow
        label="Reconciliation"
        state={health.reconciliation.state}
        lines={[
          health.reconciliation.invalid_count === 0
            ? "0 accounts need review"
            : `${health.reconciliation.invalid_count} account(s) need review`,
          ...health.reconciliation.accounts.map((a) =>
            a.needs_review
              ? `${a.name}: reconciliation needs review`
              : a.last_reconciled_date
                ? `${a.name}: reconciled through ${formatDate(a.last_reconciled_date)}`
                : `${a.name}: never reconciled`,
          ),
        ]}
      />
      <HealthRow
        label="Recurring"
        state={health.recurring.state}
        lines={[
          health.recurring.pending_review_count === 0
            ? "0 items need review"
            : `${health.recurring.pending_review_count} item(s) need review`,
        ]}
      />
    </div>
  );
}

function ReviewPage({ onNavigate }: { onNavigate: (tab: Tab) => void }) {
  useMasked();
  const [tab, setTab] = useState<ReviewTab>("attention");

  return (
    <section aria-labelledby="review-heading">
      <div className="page-toolbar">
        <h1 id="review-heading">Review</h1>
      </div>

      <div className="insights-tabs" role="tablist" aria-label="Review">
        {(
          [
            { id: "attention", label: "Needs Attention" },
            { id: "signals", label: "Signals" },
            { id: "health", label: "Data Health" },
          ] as { id: ReviewTab; label: string }[]
        ).map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={tab === t.id}
            className={
              tab === t.id
                ? "insights-tab-button insights-tab-button-active"
                : "insights-tab-button"
            }
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </div>

      {tab === "attention" && <NeedsAttentionTab onNavigate={onNavigate} />}
      {tab === "signals" && <SignalsTab onNavigate={onNavigate} />}
      {tab === "health" && <DataHealthTab />}
    </section>
  );
}

export default ReviewPage;
