import { useEffect, useState } from "react";
import type { Tab } from "./App";
import type {
  Account,
  ImportRecord,
  Payee,
  ReconciliationStatus,
  Tag,
  Transaction,
  TransactionFilter,
} from "./api/midas";
import {
  accountsApi,
  bulkApi,
  importHistoryApi,
  payeesApi,
  reconciliationApi,
  tagsApi,
  transactionsApi,
} from "./api/midas";
import AccountForm from "./AccountForm";
import BulkReconciledWarning from "./BulkReconciledWarning";
import ImportHistoryDialog from "./ImportHistoryDialog";
import ImportWizard from "./ImportWizard";
import ReconciledEditWarning from "./ReconciledEditWarning";
import TransactionDetails from "./TransactionDetails";
import TransactionForm from "./TransactionForm";
import { SUGGESTED_CATEGORIES } from "./lib/categories";
import { formatCents, useMasked } from "./lib/currency";
import { formatDate } from "./lib/dateFormat";
import { TRANSACTION_TYPE_LABELS } from "./lib/transactionTypes";

interface TransactionsPageProps {
  onNavigate: (tab: Tab) => void;
}

type BulkAction =
  | ""
  | "set_category"
  | "set_payee"
  | "add_tag"
  | "remove_tag"
  | "mark_one_time"
  | "clear_one_time"
  | "archive";

function TransactionsPage({ onNavigate }: TransactionsPageProps) {
  useMasked();
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [payees, setPayees] = useState<Payee[]>([]);
  const [allTags, setAllTags] = useState<Tag[]>([]);
  const [imports, setImports] = useState<ImportRecord[]>([]);
  const [transactions, setTransactions] = useState<Transaction[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    accountsApi
      .list(false)
      .then(setAccounts)
      .catch((err) => setError(String(err)));
    payeesApi
      .list(true)
      .then(setPayees)
      .catch((err) => setError(String(err)));
    tagsApi
      .list()
      .then(setAllTags)
      .catch((err) => setError(String(err)));
    importHistoryApi
      .list()
      .then(setImports)
      .catch((err) => setError(String(err)));
  }, []);
  const [search, setSearch] = useState("");
  const [accountId, setAccountId] = useState<number | "">("");
  const [category, setCategory] = useState("");
  const [dateFrom, setDateFrom] = useState("");
  const [dateTo, setDateTo] = useState("");
  const [payeeFilter, setPayeeFilter] = useState<number | "">("");
  const [tagFilter, setTagFilter] = useState<number | "">("");
  const [reconciliationFilter, setReconciliationFilter] = useState<
    ReconciliationStatus | ""
  >("");
  const [oneTimeFilter, setOneTimeFilter] = useState<"" | "yes" | "no">("");
  const [editing, setEditing] = useState<Transaction | null | "new">(null);
  const [importing, setImporting] = useState(false);
  const [viewingHistory, setViewingHistory] = useState(false);
  const [addingAccount, setAddingAccount] = useState(false);
  const [viewingDetails, setViewingDetails] = useState<Transaction | null>(
    null,
  );
  const [pendingReconciledAction, setPendingReconciledAction] = useState<{
    transaction: Transaction;
    actionLabel: string;
    run: () => void;
  } | null>(null);
  const [showRunningBalance, setShowRunningBalance] = useState(false);
  const [runningBalances, setRunningBalances] = useState<Map<number, number>>(
    new Map(),
  );
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [bulkAction, setBulkAction] = useState<BulkAction>("");
  const [bulkCategoryValue, setBulkCategoryValue] = useState("");
  const [bulkPayeeValue, setBulkPayeeValue] = useState<number | "">("");
  const [bulkTagValue, setBulkTagValue] = useState<number | "">("");
  const [pendingBulkAction, setPendingBulkAction] = useState<{
    count: number;
    run: () => void;
  } | null>(null);

  async function refreshAccounts() {
    setAccounts(await accountsApi.list(false));
  }

  const filter: TransactionFilter = {
    account_id: accountId === "" ? null : accountId,
    search: search.trim() === "" ? null : search.trim(),
    category: category === "" ? null : category,
    date_from: dateFrom === "" ? null : dateFrom,
    date_to: dateTo === "" ? null : dateTo,
    payee_id: payeeFilter === "" ? null : payeeFilter,
    tag_id: tagFilter === "" ? null : tagFilter,
    reconciliation_status:
      reconciliationFilter === "" ? null : reconciliationFilter,
    is_one_time: oneTimeFilter === "" ? null : oneTimeFilter === "yes",
  };
  const filterKey = JSON.stringify(filter);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      setTransactions(await transactionsApi.list(filter));
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
    setSelectedIds(new Set());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filterKey]);

  function accountName(id: number): string {
    return accounts.find((a) => a.id === id)?.name ?? `Account #${id}`;
  }

  function payeeName(id: number | null): string | null {
    if (id === null) return null;
    return payees.find((p) => p.id === id)?.name ?? null;
  }

  function importFilename(id: number | null): string | null {
    if (id === null) return null;
    return imports.find((i) => i.id === id)?.source_filename ?? null;
  }

  async function toggleArchived(t: Transaction) {
    try {
      await transactionsApi.setArchived(t.id, !t.is_archived);
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  useEffect(() => {
    if (!showRunningBalance || accountId === "") {
      setRunningBalances(new Map());
      return;
    }
    reconciliationApi
      .getRunningBalances(accountId)
      .then((rows) =>
        setRunningBalances(
          new Map(rows.map((r) => [r.transaction_id, r.running_balance_cents])),
        ),
      )
      .catch((err) => setError(String(err)));
  }, [showRunningBalance, accountId, transactions]);

  function requestEdit(t: Transaction) {
    if (t.reconciliation_id !== null) {
      setPendingReconciledAction({
        transaction: t,
        actionLabel: "Changing",
        run: () => setEditing(t),
      });
      return;
    }
    setEditing(t);
  }

  function requestArchiveToggle(t: Transaction) {
    if (t.reconciliation_id !== null && !t.is_archived) {
      setPendingReconciledAction({
        transaction: t,
        actionLabel: "Archiving",
        run: () => toggleArchived(t),
      });
      return;
    }
    toggleArchived(t);
  }

  const allVisibleSelected =
    transactions.length > 0 && transactions.every((t) => selectedIds.has(t.id));

  function toggleSelected(id: number) {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  function toggleSelectAllVisible() {
    setSelectedIds(
      allVisibleSelected ? new Set() : new Set(transactions.map((t) => t.id)),
    );
  }

  function runWithReconciledGate(run: () => Promise<void>) {
    const reconciledCount = transactions.filter(
      (t) => selectedIds.has(t.id) && t.reconciliation_id !== null,
    ).length;
    if (reconciledCount > 0) {
      setPendingBulkAction({ count: reconciledCount, run: () => void run() });
      return;
    }
    void run();
  }

  async function applyBulkAction() {
    if (bulkAction === "" || selectedIds.size === 0) return;
    if (
      (bulkAction === "add_tag" || bulkAction === "remove_tag") &&
      bulkTagValue === ""
    ) {
      return;
    }
    const ids = Array.from(selectedIds);
    const run = async () => {
      setError(null);
      try {
        switch (bulkAction) {
          case "set_category":
            await bulkApi.setCategory(
              ids,
              bulkCategoryValue.trim() === "" ? null : bulkCategoryValue.trim(),
            );
            break;
          case "set_payee":
            await bulkApi.setPayee(
              ids,
              bulkPayeeValue === "" ? null : bulkPayeeValue,
            );
            break;
          case "add_tag":
            await bulkApi.addTag(ids, bulkTagValue as number);
            break;
          case "remove_tag":
            await bulkApi.removeTag(ids, bulkTagValue as number);
            break;
          case "mark_one_time":
            await bulkApi.setOneTime(ids, true);
            break;
          case "clear_one_time":
            await bulkApi.setOneTime(ids, false);
            break;
          case "archive":
            await bulkApi.setArchived(ids, true);
            break;
        }
        setSelectedIds(new Set());
        setBulkAction("");
        setBulkCategoryValue("");
        setBulkPayeeValue("");
        setBulkTagValue("");
        await refresh();
      } catch (err) {
        setError(String(err));
      }
    };
    runWithReconciledGate(run);
  }

  const activePayees = payees.filter((p) => p.merged_into_payee_id === null);

  return (
    <section aria-labelledby="transactions-heading">
      <div className="page-toolbar">
        <h1 id="transactions-heading">Transactions</h1>
        <div className="page-toolbar-actions">
          <button
            type="button"
            className="btn-secondary"
            onClick={() => setImporting(true)}
            disabled={accounts.length === 0}
          >
            Import
          </button>
          <button
            type="button"
            className="btn-secondary"
            onClick={() => setViewingHistory(true)}
          >
            Import History
          </button>
          <button
            type="button"
            className="btn-primary"
            onClick={() => setEditing("new")}
            disabled={accounts.length === 0}
          >
            Add transaction
          </button>
        </div>
      </div>

      {accounts.length === 0 && (
        <p className="muted">
          Create the account in MIDAS before adding or importing its
          transactions.{" "}
          <button
            type="button"
            className="btn-link"
            onClick={() => setAddingAccount(true)}
          >
            Add account
          </button>
        </p>
      )}

      <div className="filters-row">
        <input
          type="search"
          className="search-input"
          placeholder="Search description, merchant, notes…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          aria-label="Search transactions"
        />
        <select
          value={accountId}
          onChange={(e) =>
            setAccountId(e.target.value === "" ? "" : Number(e.target.value))
          }
          aria-label="Filter by account"
        >
          <option value="">All accounts</option>
          {accounts.map((a) => (
            <option key={a.id} value={a.id}>
              {a.name}
            </option>
          ))}
        </select>
        <select
          value={category}
          onChange={(e) => setCategory(e.target.value)}
          aria-label="Filter by category"
        >
          <option value="">All categories</option>
          {SUGGESTED_CATEGORIES.map((c) => (
            <option key={c} value={c}>
              {c}
            </option>
          ))}
        </select>
        <select
          value={payeeFilter}
          onChange={(e) =>
            setPayeeFilter(e.target.value === "" ? "" : Number(e.target.value))
          }
          aria-label="Filter by payee"
        >
          <option value="">All payees</option>
          {activePayees.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
        <select
          value={tagFilter}
          onChange={(e) =>
            setTagFilter(e.target.value === "" ? "" : Number(e.target.value))
          }
          aria-label="Filter by tag"
        >
          <option value="">All tags</option>
          {allTags.map((t) => (
            <option key={t.id} value={t.id}>
              {t.name}
            </option>
          ))}
        </select>
        <select
          value={reconciliationFilter}
          onChange={(e) =>
            setReconciliationFilter(e.target.value as ReconciliationStatus | "")
          }
          aria-label="Filter by reconciliation status"
        >
          <option value="">Any reconciliation status</option>
          <option value="unreconciled">Unreconciled</option>
          <option value="cleared">Cleared</option>
          <option value="reconciled">Reconciled</option>
        </select>
        <select
          value={oneTimeFilter}
          onChange={(e) =>
            setOneTimeFilter(e.target.value as "" | "yes" | "no")
          }
          aria-label="Filter by one-time status"
        >
          <option value="">One-time or not</option>
          <option value="yes">One-time only</option>
          <option value="no">Exclude one-time</option>
        </select>
        <label className="date-filter">
          From
          <input
            type="date"
            value={dateFrom}
            onChange={(e) => setDateFrom(e.target.value)}
          />
        </label>
        <label className="date-filter">
          To
          <input
            type="date"
            value={dateTo}
            onChange={(e) => setDateTo(e.target.value)}
          />
        </label>
        <label
          className="show-archived-toggle"
          title={
            accountId === ""
              ? "Select one account above to show its running balance."
              : undefined
          }
        >
          <input
            type="checkbox"
            checked={showRunningBalance}
            disabled={accountId === ""}
            onChange={(e) => setShowRunningBalance(e.target.checked)}
          />
          Show running balance
        </label>
      </div>

      {selectedIds.size > 0 && (
        <div className="bulk-toolbar" role="toolbar" aria-label="Bulk actions">
          <span className="bulk-toolbar-count">
            {selectedIds.size} selected
          </span>
          <button
            type="button"
            className="btn-link"
            onClick={() => setSelectedIds(new Set())}
          >
            Clear
          </button>
          <label className="sr-only" htmlFor="bulk-action-select">
            Bulk action
          </label>
          <select
            id="bulk-action-select"
            value={bulkAction}
            onChange={(e) => setBulkAction(e.target.value as BulkAction)}
          >
            <option value="">Choose action…</option>
            <option value="set_category">Set category</option>
            <option value="set_payee">Set payee</option>
            <option value="add_tag">Add tag</option>
            <option value="remove_tag">Remove tag</option>
            <option value="mark_one_time">Mark one-time</option>
            <option value="clear_one_time">Clear one-time</option>
            <option value="archive">Archive</option>
          </select>
          {bulkAction === "set_category" && (
            <input
              value={bulkCategoryValue}
              onChange={(e) => setBulkCategoryValue(e.target.value)}
              placeholder="Category (blank clears it)"
              list="txn-bulk-category-options"
              aria-label="Category to set"
            />
          )}
          {bulkAction === "set_category" && (
            <datalist id="txn-bulk-category-options">
              {SUGGESTED_CATEGORIES.map((c) => (
                <option key={c} value={c} />
              ))}
            </datalist>
          )}
          {bulkAction === "set_payee" && (
            <select
              value={bulkPayeeValue}
              onChange={(e) =>
                setBulkPayeeValue(
                  e.target.value === "" ? "" : Number(e.target.value),
                )
              }
              aria-label="Payee to set"
            >
              <option value="">No payee</option>
              {activePayees.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          )}
          {(bulkAction === "add_tag" || bulkAction === "remove_tag") && (
            <select
              value={bulkTagValue}
              onChange={(e) =>
                setBulkTagValue(
                  e.target.value === "" ? "" : Number(e.target.value),
                )
              }
              aria-label="Tag"
            >
              <option value="">Choose tag…</option>
              {allTags.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name}
                </option>
              ))}
            </select>
          )}
          <button
            type="button"
            className="btn-primary"
            onClick={applyBulkAction}
            disabled={
              bulkAction === "" ||
              ((bulkAction === "add_tag" || bulkAction === "remove_tag") &&
                bulkTagValue === "")
            }
          >
            Apply
          </button>
        </div>
      )}

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {loading ? (
        <p className="muted">Loading transactions…</p>
      ) : transactions.length === 0 ? (
        <div className="empty-state">
          {accounts.length === 0 ? (
            <>
              <h2>No accounts yet</h2>
              <p className="muted">
                Create the account in MIDAS before adding or importing its
                transactions.
              </p>
            </>
          ) : search ||
            accountId !== "" ||
            category ||
            dateFrom ||
            dateTo ||
            payeeFilter !== "" ||
            tagFilter !== "" ||
            reconciliationFilter !== "" ||
            oneTimeFilter !== "" ? (
            <>
              <h2>No matching transactions</h2>
              <p className="muted">
                No transactions match these filters. Try clearing one or more.
              </p>
            </>
          ) : (
            <>
              <h2>No transactions yet</h2>
              <p className="muted">Add one manually or import a statement.</p>
              <button
                type="button"
                className="btn-primary"
                onClick={() => setEditing("new")}
              >
                Add transaction
              </button>
            </>
          )}
        </div>
      ) : (
        <div className="table-scroll">
          <table className="accounts-table">
            <thead>
              <tr>
                <th scope="col" className="col-select">
                  <input
                    type="checkbox"
                    checked={allVisibleSelected}
                    onChange={toggleSelectAllVisible}
                    aria-label="Select all visible transactions"
                  />
                </th>
                <th scope="col">Date</th>
                <th scope="col">Merchant</th>
                <th scope="col" className="col-balance">
                  Amount
                </th>
                {showRunningBalance && accountId !== "" && (
                  <th scope="col" className="col-balance">
                    Running balance
                  </th>
                )}
                <th scope="col">Category</th>
                <th scope="col">Account</th>
                <th scope="col">
                  <span className="sr-only">Status</span>
                </th>
                <th scope="col">
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {transactions.map((t) => (
                <tr
                  key={t.id}
                  className={t.is_archived ? "row-archived" : undefined}
                >
                  <td className="col-select">
                    <input
                      type="checkbox"
                      checked={selectedIds.has(t.id)}
                      onChange={() => toggleSelected(t.id)}
                      aria-label={`Select transaction: ${t.merchant}, ${formatDate(t.transaction_date)}, ${formatCents(t.amount_cents)}`}
                    />
                  </td>
                  <td>{formatDate(t.transaction_date)}</td>
                  <td
                    title={
                      t.merchant !== t.raw_description
                        ? t.raw_description
                        : undefined
                    }
                  >
                    {t.merchant}
                  </td>
                  <td className="col-balance">
                    <span
                      className={
                        t.amount_cents < 0 ? "amount-owed" : "amount-positive"
                      }
                    >
                      {formatCents(t.amount_cents)}
                    </span>
                  </td>
                  {showRunningBalance && accountId !== "" && (
                    <td className="col-balance">
                      {runningBalances.has(t.id)
                        ? formatCents(runningBalances.get(t.id)!)
                        : "—"}
                    </td>
                  )}
                  <td>
                    {t.has_splits ? (
                      <span className="muted">Split</span>
                    ) : (
                      (t.category ?? (
                        <span className="muted">Uncategorized</span>
                      ))
                    )}
                  </td>
                  <td>{accountName(t.account_id)}</td>
                  <td>
                    {t.transaction_type !== "purchase" && (
                      <span className="badge-neutral">
                        {TRANSACTION_TYPE_LABELS[t.transaction_type]}
                      </span>
                    )}
                    {t.needs_review && (
                      <span className="badge-review">Needs review</span>
                    )}
                    {t.is_one_time && (
                      <span className="badge-neutral">One-time</span>
                    )}
                    {t.has_splits && (
                      <span className="badge-neutral">Split</span>
                    )}
                    {t.is_archived && (
                      <span className="badge-archived">Archived</span>
                    )}
                  </td>
                  <td className="col-actions">
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => requestEdit(t)}
                    >
                      Edit
                    </button>
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => setViewingDetails(t)}
                    >
                      Details
                    </button>
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => requestArchiveToggle(t)}
                    >
                      {t.is_archived ? "Unarchive" : "Archive"}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {editing !== null && (
        <TransactionForm
          accounts={accounts}
          defaultAccountId={accountId === "" ? null : accountId}
          transaction={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            refresh();
          }}
        />
      )}

      {viewingDetails && (
        <TransactionDetails
          transaction={viewingDetails}
          accountName={accountName(viewingDetails.account_id)}
          payeeName={payeeName(viewingDetails.payee_id)}
          importFilename={importFilename(viewingDetails.source_import_id)}
          onClose={() => setViewingDetails(null)}
        />
      )}

      {importing && (
        <ImportWizard
          accounts={accounts}
          onClose={() => setImporting(false)}
          onImported={() => {
            setImporting(false);
            refresh();
          }}
          onReviewNeeded={() => onNavigate("review")}
        />
      )}

      {viewingHistory && (
        <ImportHistoryDialog
          onClose={() => {
            setViewingHistory(false);
            refresh();
          }}
        />
      )}

      {addingAccount && (
        <AccountForm
          account={null}
          onClose={() => setAddingAccount(false)}
          onSaved={() => {
            setAddingAccount(false);
            refreshAccounts();
          }}
        />
      )}

      {pendingReconciledAction && (
        <ReconciledEditWarning
          transaction={pendingReconciledAction.transaction}
          actionLabel={pendingReconciledAction.actionLabel}
          onCancel={() => setPendingReconciledAction(null)}
          onConfirm={() => {
            pendingReconciledAction.run();
            setPendingReconciledAction(null);
          }}
        />
      )}

      {pendingBulkAction && (
        <BulkReconciledWarning
          count={pendingBulkAction.count}
          actionLabel="Changing"
          onCancel={() => setPendingBulkAction(null)}
          onConfirm={() => {
            pendingBulkAction.run();
            setPendingBulkAction(null);
          }}
        />
      )}
    </section>
  );
}

export default TransactionsPage;
