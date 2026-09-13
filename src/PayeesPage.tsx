import { useEffect, useRef, useState } from "react";
import type { Payee, PayeeInput, PayeeType } from "./api/midas";
import { payeesApi } from "./api/midas";

const PAYEE_TYPE_LABELS: Record<PayeeType, string> = {
  merchant: "Merchant",
  company: "Company",
  person: "Person",
};

function PayeeForm({
  payee,
  onClose,
  onSaved,
}: {
  payee: Payee | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [name, setName] = useState(payee?.name ?? "");
  const [payeeType, setPayeeType] = useState<PayeeType>(
    payee?.payee_type ?? "merchant",
  );
  const [notes, setNotes] = useState(payee?.notes ?? "");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (name.trim() === "") {
      setError("Name is required.");
      return;
    }
    const input: PayeeInput = {
      name: name.trim(),
      payee_type: payeeType,
      notes: notes.trim() === "" ? null : notes.trim(),
    };
    setSaving(true);
    setError(null);
    try {
      if (payee) {
        await payeesApi.update(payee.id, input);
      } else {
        await payeesApi.create(input);
      }
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
      aria-labelledby="payee-form-title"
      onClose={onClose}
    >
      <form method="dialog" onSubmit={handleSubmit}>
        <h2 id="payee-form-title">{payee ? "Edit payee" : "Add payee"}</h2>

        <div className="field">
          <label htmlFor="payee-name" className="field-required">
            Name
          </label>
          <input
            id="payee-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            autoFocus
          />
        </div>

        <div className="field">
          <label htmlFor="payee-type">Type</label>
          <select
            id="payee-type"
            value={payeeType}
            onChange={(e) => setPayeeType(e.target.value as PayeeType)}
          >
            {(Object.keys(PAYEE_TYPE_LABELS) as PayeeType[]).map((t) => (
              <option key={t} value={t}>
                {PAYEE_TYPE_LABELS[t]}
              </option>
            ))}
          </select>
        </div>

        <div className="field">
          <label htmlFor="payee-notes">Notes</label>
          <textarea
            id="payee-notes"
            value={notes}
            onChange={(e) => setNotes(e.target.value)}
            rows={2}
          />
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

function MergeDialog({
  source,
  candidates,
  onClose,
  onMerged,
}: {
  source: Payee;
  candidates: Payee[];
  onClose: () => void;
  onMerged: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [targetId, setTargetId] = useState<number | "">(
    candidates[0]?.id ?? "",
  );
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  async function handleMerge() {
    if (targetId === "") return;
    setSaving(true);
    setError(null);
    try {
      await payeesApi.merge(source.id, targetId);
      onMerged();
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
      aria-labelledby="merge-payee-title"
      onClose={onClose}
    >
      <h2 id="merge-payee-title">Merge "{source.name}" into…</h2>
      <p className="muted">
        Every transaction linked to "{source.name}" moves to the payee you
        choose below. "{source.name}" is archived, not deleted, so its history
        stays intact.
      </p>

      <div className="field">
        <label htmlFor="merge-target">Canonical payee</label>
        <select
          id="merge-target"
          value={targetId}
          onChange={(e) =>
            setTargetId(e.target.value === "" ? "" : Number(e.target.value))
          }
        >
          {candidates.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </select>
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
        <button
          type="button"
          className="btn-primary"
          onClick={handleMerge}
          disabled={saving || targetId === ""}
        >
          {saving ? "Merging…" : "Merge"}
        </button>
      </div>
    </dialog>
  );
}

function PayeesPage() {
  const [payees, setPayees] = useState<Payee[]>([]);
  const [search, setSearch] = useState("");
  const [showArchived, setShowArchived] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<Payee | null | "new">(null);
  const [merging, setMerging] = useState<Payee | null>(null);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      setPayees(await payeesApi.list(showArchived));
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

  async function toggleArchived(payee: Payee) {
    try {
      await payeesApi.setArchived(payee.id, !payee.is_archived);
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  const filtered = payees.filter((p) =>
    p.name.toLowerCase().includes(search.trim().toLowerCase()),
  );

  return (
    <section aria-labelledby="payees-heading">
      <div className="page-toolbar">
        <h3 id="payees-heading">Payees</h3>
        <div className="page-toolbar-actions">
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
            Add payee
          </button>
        </div>
      </div>

      <input
        type="search"
        className="search-input"
        placeholder="Search payees…"
        value={search}
        onChange={(e) => setSearch(e.target.value)}
        aria-label="Search payees"
      />

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {loading ? (
        <p className="muted">Loading…</p>
      ) : filtered.length === 0 ? (
        <p className="muted">No payees yet.</p>
      ) : (
        <div className="table-scroll">
          <table className="accounts-table">
            <thead>
              <tr>
                <th scope="col">Name</th>
                <th scope="col">Type</th>
                <th scope="col">Transactions</th>
                <th scope="col">
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((p) => (
                <tr
                  key={p.id}
                  className={p.is_archived ? "row-archived" : undefined}
                >
                  <td>
                    {p.name}
                    {p.merged_into_payee_id !== null && (
                      <span className="badge-archived">Merged</span>
                    )}
                    {p.is_archived && p.merged_into_payee_id === null && (
                      <span className="badge-archived">Archived</span>
                    )}
                  </td>
                  <td>{PAYEE_TYPE_LABELS[p.payee_type]}</td>
                  <td>{p.transaction_count}</td>
                  <td className="col-actions">
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => setEditing(p)}
                    >
                      Edit
                    </button>
                    {p.merged_into_payee_id === null && (
                      <button
                        type="button"
                        className="btn-link"
                        onClick={() => setMerging(p)}
                        disabled={
                          payees.filter(
                            (o) =>
                              o.id !== p.id && o.merged_into_payee_id === null,
                          ).length === 0
                        }
                      >
                        Merge into…
                      </button>
                    )}
                    {p.merged_into_payee_id === null && (
                      <button
                        type="button"
                        className="btn-link"
                        onClick={() => toggleArchived(p)}
                      >
                        {p.is_archived ? "Unarchive" : "Archive"}
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {editing !== null && (
        <PayeeForm
          payee={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            refresh();
          }}
        />
      )}

      {merging !== null && (
        <MergeDialog
          source={merging}
          candidates={payees.filter(
            (p) => p.id !== merging.id && p.merged_into_payee_id === null,
          )}
          onClose={() => setMerging(null)}
          onMerged={() => {
            setMerging(null);
            refresh();
          }}
        />
      )}
    </section>
  );
}

export default PayeesPage;
