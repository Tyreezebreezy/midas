import { useEffect, useRef, useState } from "react";
import type { Account, ImportProfile } from "./api/midas";
import { accountsApi, importProfilesApi } from "./api/midas";

const SOURCE_TYPE_LABELS: Record<ImportProfile["source_type"], string> = {
  csv: "CSV",
  ofx: "OFX",
  qfx: "QFX",
};

function RenameProfileDialog({
  profile,
  onClose,
  onSaved,
}: {
  profile: ImportProfile;
  onClose: () => void;
  onSaved: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [name, setName] = useState(profile.name);
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
    setSaving(true);
    setError(null);
    try {
      await importProfilesApi.update(profile.id, {
        name: name.trim(),
        account_id: profile.account_id,
        source_type: profile.source_type,
        column_mapping: profile.column_mapping,
      });
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
      aria-labelledby="rename-import-profile-title"
      onClose={onClose}
    >
      <form method="dialog" onSubmit={handleSubmit}>
        <h2 id="rename-import-profile-title">Rename import profile</h2>
        <div className="field">
          <label htmlFor="import-profile-name" className="field-required">
            Name
          </label>
          <input
            id="import-profile-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            autoFocus
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

// Import profile management (section 55) - a lightweight surface over
// Batch 3's existing profile backend, never a second import pipeline. A
// profile is created implicitly the first time a CSV mapping is saved
// during import, so this section only ever lists/renames/archives what
// already exists - no "Add profile" button.
function ImportProfilesSection() {
  const [profiles, setProfiles] = useState<ImportProfile[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [showArchived, setShowArchived] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<ImportProfile | null>(null);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      const [profileList, accountList] = await Promise.all([
        importProfilesApi.list(showArchived),
        accountsApi.list(true),
      ]);
      setProfiles(profileList);
      setAccounts(accountList);
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

  async function toggleArchived(profile: ImportProfile) {
    try {
      await importProfilesApi.setArchived(profile.id, !profile.is_archived);
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  function accountName(id: number | null): string {
    if (id === null) return "Any account";
    return accounts.find((a) => a.id === id)?.name ?? `Account #${id}`;
  }

  return (
    <section aria-labelledby="import-profiles-heading">
      <div className="page-toolbar">
        <h3 id="import-profiles-heading">Import Profiles</h3>
        <div className="page-toolbar-actions">
          <label className="show-archived-toggle">
            <input
              type="checkbox"
              checked={showArchived}
              onChange={(e) => setShowArchived(e.target.checked)}
            />
            Show archived
          </label>
        </div>
      </div>
      <p className="page-description muted">
        Saved column mappings from past CSV imports. MIDAS suggests one
        automatically next time a file's headers match.
      </p>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {loading ? (
        <p className="muted">Loading…</p>
      ) : profiles.length === 0 ? (
        <p className="muted">No import profiles yet.</p>
      ) : (
        <div className="table-scroll">
          <table className="accounts-table">
            <thead>
              <tr>
                <th scope="col">Name</th>
                <th scope="col">Source</th>
                <th scope="col">Account</th>
                <th scope="col">
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {profiles.map((p) => (
                <tr
                  key={p.id}
                  className={p.is_archived ? "row-archived" : undefined}
                >
                  <td>
                    {p.name}
                    {p.is_archived && (
                      <span className="badge-archived">Archived</span>
                    )}
                  </td>
                  <td>{SOURCE_TYPE_LABELS[p.source_type]}</td>
                  <td>{accountName(p.account_id)}</td>
                  <td className="col-actions">
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => setRenaming(p)}
                    >
                      Rename
                    </button>
                    <button
                      type="button"
                      className="btn-link"
                      onClick={() => toggleArchived(p)}
                    >
                      {p.is_archived ? "Restore" : "Archive"}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {renaming !== null && (
        <RenameProfileDialog
          profile={renaming}
          onClose={() => setRenaming(null)}
          onSaved={() => {
            setRenaming(null);
            refresh();
          }}
        />
      )}
    </section>
  );
}

export default ImportProfilesSection;
