import { useEffect, useRef, useState } from "react";
import type {
  Account,
  MatchType,
  MerchantRule,
  MerchantRuleInput,
} from "./api/midas";
import { accountsApi, rulesApi } from "./api/midas";
import { SUGGESTED_CATEGORIES } from "./lib/categories";

const MATCH_TYPE_LABELS: Record<MatchType, string> = {
  exact: "is exactly",
  contains: "contains",
  starts_with: "starts with",
};

function RuleForm({
  accounts,
  onClose,
  onSaved,
}: {
  accounts: Account[];
  onClose: () => void;
  onSaved: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [matchType, setMatchType] = useState<MatchType>("contains");
  const [matchValue, setMatchValue] = useState("");
  const [accountId, setAccountId] = useState<number | "">("");
  const [merchant, setMerchant] = useState("");
  const [category, setCategory] = useState("");
  const [priority, setPriority] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (matchValue.trim() === "") {
      setError("Match text is required.");
      return;
    }
    if (merchant.trim() === "" && category.trim() === "") {
      setError("Set a merchant rewrite, a category, or both.");
      return;
    }
    const input: MerchantRuleInput = {
      match_type: matchType,
      match_value: matchValue.trim(),
      account_id: accountId === "" ? null : accountId,
      normalized_merchant: merchant.trim() === "" ? null : merchant.trim(),
      category: category.trim() === "" ? null : category.trim(),
      subcategory: null,
      priority,
    };
    setSaving(true);
    setError(null);
    try {
      await rulesApi.create(input);
      onSaved();
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <dialog
      ref={dialogRef}
      className="account-dialog"
      aria-labelledby="rule-form-title"
      onClose={onClose}
    >
      <form method="dialog" onSubmit={handleSubmit}>
        <h2 id="rule-form-title">Add rule</h2>

        <div className="field mapping-row">
          <label htmlFor="rule-match-type">When description</label>
          <select
            id="rule-match-type"
            value={matchType}
            onChange={(e) => setMatchType(e.target.value as MatchType)}
          >
            {(Object.keys(MATCH_TYPE_LABELS) as MatchType[]).map((t) => (
              <option key={t} value={t}>
                {MATCH_TYPE_LABELS[t]}
              </option>
            ))}
          </select>
        </div>

        <div className="field">
          <label htmlFor="rule-match-value">Match text</label>
          <input
            id="rule-match-value"
            value={matchValue}
            onChange={(e) => setMatchValue(e.target.value)}
            placeholder="e.g. coffee shop"
            required
            autoFocus
          />
        </div>

        <div className="field">
          <label htmlFor="rule-account">Applies to</label>
          <select
            id="rule-account"
            value={accountId}
            onChange={(e) =>
              setAccountId(e.target.value === "" ? "" : Number(e.target.value))
            }
          >
            <option value="">All accounts</option>
            {accounts.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name} only
              </option>
            ))}
          </select>
        </div>

        <div className="field">
          <label htmlFor="rule-merchant">Rewrite merchant to</label>
          <input
            id="rule-merchant"
            value={merchant}
            onChange={(e) => setMerchant(e.target.value)}
            placeholder="Optional"
          />
        </div>

        <div className="field">
          <label htmlFor="rule-category">Assign category</label>
          <input
            id="rule-category"
            list="rule-category-options"
            value={category}
            onChange={(e) => setCategory(e.target.value)}
            placeholder="Optional"
          />
          <datalist id="rule-category-options">
            {SUGGESTED_CATEGORIES.map((c) => (
              <option key={c} value={c} />
            ))}
          </datalist>
        </div>

        <div className="field">
          <label htmlFor="rule-priority">Priority</label>
          <input
            id="rule-priority"
            type="number"
            value={priority}
            onChange={(e) => setPriority(Number(e.target.value))}
          />
          <p className="field-hint">
            Higher priority wins when more than one rule matches.
          </p>
        </div>

        {error && (
          <p className="form-error" role="alert">
            {error}
          </p>
        )}

        <div className="dialog-actions">
          <button
            type="button"
            className="btn-secondary"
            onClick={onClose}
            disabled={saving}
          >
            Cancel
          </button>
          <button type="submit" className="btn-primary" disabled={saving}>
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </form>
    </dialog>
  );
}

function RulesPage() {
  const [rules, setRules] = useState<MerchantRule[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      const [ruleList, accountList] = await Promise.all([
        rulesApi.list(),
        accountsApi.list(false),
      ]);
      setRules(ruleList);
      setAccounts(accountList);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
  }, []);

  function accountName(id: number | null): string {
    if (id === null) return "All accounts";
    return accounts.find((a) => a.id === id)?.name ?? `Account #${id}`;
  }

  async function handleDelete(id: number) {
    try {
      await rulesApi.delete(id);
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <section aria-labelledby="rules-heading">
      <div className="accounts-toolbar">
        <h1 id="rules-heading">Rules</h1>
        <button
          type="button"
          className="btn-primary"
          onClick={() => setCreating(true)}
        >
          Add rule
        </button>
      </div>
      <p className="muted">
        Rules normalize merchant names and assign categories automatically the
        next time you import a CSV. They never change transactions you've
        already imported.
      </p>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {loading ? (
        <p className="muted">Loading rules…</p>
      ) : rules.length === 0 ? (
        <p className="muted">No rules yet.</p>
      ) : (
        <div className="table-scroll">
          <table className="accounts-table">
            <thead>
              <tr>
                <th scope="col">When description</th>
                <th scope="col">Applies to</th>
                <th scope="col">Merchant</th>
                <th scope="col">Category</th>
                <th scope="col">Priority</th>
                <th scope="col">
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {rules.map((rule) => (
                <tr key={rule.id}>
                  <td>
                    {MATCH_TYPE_LABELS[rule.match_type]} "{rule.match_value}"
                  </td>
                  <td>{accountName(rule.account_id)}</td>
                  <td>
                    {rule.normalized_merchant ?? (
                      <span className="muted">—</span>
                    )}
                  </td>
                  <td>{rule.category ?? <span className="muted">—</span>}</td>
                  <td>{rule.priority}</td>
                  <td className="col-actions">
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => handleDelete(rule.id)}
                    >
                      Delete
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {creating && (
        <RuleForm
          accounts={accounts}
          onClose={() => setCreating(false)}
          onSaved={() => {
            setCreating(false);
            refresh();
          }}
        />
      )}
    </section>
  );
}

export default RulesPage;
