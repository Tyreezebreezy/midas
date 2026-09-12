import { useEffect, useState } from "react";
import type { Account } from "./api/midas";
import { accountsApi } from "./api/midas";
import AccountForm from "./AccountForm";
import ReconcileBalanceForm from "./ReconcileBalanceForm";
import ReconciliationHistoryDialog from "./ReconciliationHistoryDialog";
import ReconciliationWorkspace from "./ReconciliationWorkspace";
import { ACCOUNT_TYPE_LABELS, LIABILITY_TYPES } from "./lib/accountTypes";
import { formatCents } from "./lib/currency";

function AccountsPage() {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [showArchived, setShowArchived] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<Account | null | "new">(null);
  const [reconciling, setReconciling] = useState<Account | null>(null);
  const [reconcileWorkspace, setReconcileWorkspace] = useState<Account | null>(
    null,
  );
  const [reconcileHistory, setReconcileHistory] = useState<Account | null>(
    null,
  );

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      setAccounts(await accountsApi.list(showArchived));
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showArchived]);

  async function toggleArchived(account: Account) {
    try {
      await accountsApi.setArchived(account.id, !account.is_archived);
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <section aria-labelledby="accounts-heading" className="accounts-page">
      <div className="accounts-toolbar">
        <h1 id="accounts-heading">Accounts</h1>
        <div className="accounts-toolbar-actions">
          <label className="show-archived-toggle">
            <input
              type="checkbox"
              checked={showArchived}
              onChange={(e) => setShowArchived(e.target.checked)}
            />
            Show archived
          </label>
          <button
            type="button"
            className="btn-primary"
            onClick={() => setEditing("new")}
          >
            Add account
          </button>
        </div>
      </div>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {loading ? (
        <p className="muted">Loading accounts…</p>
      ) : accounts.length === 0 ? (
        <p className="muted">
          {showArchived
            ? "No accounts yet."
            : "No active accounts yet. Add your first account to get started."}
        </p>
      ) : (
        <div className="table-scroll">
          <table className="accounts-table">
            <thead>
              <tr>
                <th scope="col">Name</th>
                <th scope="col">Institution</th>
                <th scope="col">Type</th>
                <th scope="col" className="col-balance">
                  Balance
                </th>
                <th scope="col">Net worth</th>
                <th scope="col">
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {accounts.map((account) => (
                <tr
                  key={account.id}
                  className={account.is_archived ? "row-archived" : undefined}
                >
                  <td>
                    {account.name}
                    {account.is_archived && (
                      <span className="badge-archived">Archived</span>
                    )}
                  </td>
                  <td>{account.institution ?? "—"}</td>
                  <td>{ACCOUNT_TYPE_LABELS[account.account_type]}</td>
                  <td className="col-balance">
                    <span
                      className={
                        LIABILITY_TYPES.has(account.account_type)
                          ? "amount-owed"
                          : undefined
                      }
                    >
                      {formatCents(account.balance_cents)}
                    </span>
                  </td>
                  <td>
                    {account.include_in_net_worth ? "Included" : "Excluded"}
                  </td>
                  <td className="col-actions">
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => setEditing(account)}
                    >
                      Edit
                    </button>
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => setReconciling(account)}
                    >
                      Update balance
                    </button>
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => setReconcileWorkspace(account)}
                      disabled={account.is_archived}
                    >
                      Reconcile
                    </button>
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => setReconcileHistory(account)}
                    >
                      Reconciliation history
                    </button>
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => toggleArchived(account)}
                    >
                      {account.is_archived ? "Unarchive" : "Archive"}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {editing !== null && (
        <AccountForm
          account={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            refresh();
          }}
        />
      )}

      {reconciling !== null && (
        <ReconcileBalanceForm
          account={reconciling}
          onClose={() => setReconciling(null)}
          onSaved={() => {
            setReconciling(null);
            refresh();
          }}
        />
      )}

      {reconcileWorkspace !== null && (
        <ReconciliationWorkspace
          account={reconcileWorkspace}
          onClose={() => setReconcileWorkspace(null)}
          onCompleted={() => {
            setReconcileWorkspace(null);
            refresh();
          }}
        />
      )}

      {reconcileHistory !== null && (
        <ReconciliationHistoryDialog
          account={reconcileHistory}
          onClose={() => setReconcileHistory(null)}
        />
      )}
    </section>
  );
}

export default AccountsPage;
