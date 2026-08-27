import { useEffect, useState } from "react";
import type { Account, Transaction, TransactionFilter } from "./api/midas";
import { accountsApi, transactionsApi } from "./api/midas";
import AccountForm from "./AccountForm";
import CsvImportWizard from "./CsvImportWizard";
import TransactionForm from "./TransactionForm";
import { SUGGESTED_CATEGORIES } from "./lib/categories";
import { formatCents } from "./lib/currency";
import { formatDate } from "./lib/dateFormat";
import { TRANSACTION_TYPE_LABELS } from "./lib/transactionTypes";

function TransactionsPage() {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [transactions, setTransactions] = useState<Transaction[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    accountsApi
      .list(false)
      .then(setAccounts)
      .catch((err) => setError(String(err)));
  }, []);
  const [search, setSearch] = useState("");
  const [accountId, setAccountId] = useState<number | "">("");
  const [category, setCategory] = useState("");
  const [dateFrom, setDateFrom] = useState("");
  const [dateTo, setDateTo] = useState("");
  const [editing, setEditing] = useState<Transaction | null | "new">(null);
  const [importing, setImporting] = useState(false);
  const [addingAccount, setAddingAccount] = useState(false);

  async function refreshAccounts() {
    setAccounts(await accountsApi.list(false));
  }

  const filter: TransactionFilter = {
    account_id: accountId === "" ? null : accountId,
    search: search.trim() === "" ? null : search.trim(),
    category: category === "" ? null : category,
    date_from: dateFrom === "" ? null : dateFrom,
    date_to: dateTo === "" ? null : dateTo,
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filterKey]);

  function accountName(id: number): string {
    return accounts.find((a) => a.id === id)?.name ?? `Account #${id}`;
  }

  async function toggleArchived(t: Transaction) {
    try {
      await transactionsApi.setArchived(t.id, !t.is_archived);
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <section aria-labelledby="transactions-heading">
      <div className="accounts-toolbar">
        <h1 id="transactions-heading">Transactions</h1>
        <div className="accounts-toolbar-actions">
          <button
            type="button"
            className="btn-secondary"
            onClick={() => setImporting(true)}
            disabled={accounts.length === 0}
          >
            Import CSV
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
      </div>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {loading ? (
        <p className="muted">Loading transactions…</p>
      ) : transactions.length === 0 ? (
        <p className="muted">No transactions match these filters.</p>
      ) : (
        <div className="table-scroll">
          <table className="accounts-table">
            <thead>
              <tr>
                <th scope="col">Date</th>
                <th scope="col">Account</th>
                <th scope="col">Merchant</th>
                <th scope="col">Category</th>
                <th scope="col">Type</th>
                <th scope="col" className="col-balance">
                  Amount
                </th>
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
                  <td>{formatDate(t.transaction_date)}</td>
                  <td>{accountName(t.account_id)}</td>
                  <td
                    title={
                      t.merchant !== t.raw_description
                        ? t.raw_description
                        : undefined
                    }
                  >
                    {t.merchant}
                  </td>
                  <td>
                    {t.category ?? <span className="muted">Uncategorized</span>}
                  </td>
                  <td>{TRANSACTION_TYPE_LABELS[t.transaction_type]}</td>
                  <td className="col-balance">
                    <span
                      className={
                        t.amount_cents < 0 ? "amount-owed" : "amount-positive"
                      }
                    >
                      {formatCents(t.amount_cents)}
                    </span>
                  </td>
                  <td>
                    {t.needs_review && (
                      <span className="badge-review">Needs review</span>
                    )}
                    {t.is_archived && (
                      <span className="badge-archived">Archived</span>
                    )}
                  </td>
                  <td className="col-actions">
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => setEditing(t)}
                    >
                      Edit
                    </button>
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => toggleArchived(t)}
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

      {importing && (
        <CsvImportWizard
          accounts={accounts}
          onClose={() => setImporting(false)}
          onImported={() => {
            setImporting(false);
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
    </section>
  );
}

export default TransactionsPage;
