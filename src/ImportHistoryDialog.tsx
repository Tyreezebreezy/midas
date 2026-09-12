import { useEffect, useRef, useState } from "react";
import type { ImportRecord, Transaction, UndoPreview } from "./api/midas";
import { importHistoryApi, transactionsApi } from "./api/midas";
import { formatCents } from "./lib/currency";
import { formatDate } from "./lib/dateFormat";

interface ImportHistoryDialogProps {
  onClose: () => void;
}

function ImportHistoryDialog({ onClose }: ImportHistoryDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [records, setRecords] = useState<ImportRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<ImportRecord | null>(null);
  const [detailTransactions, setDetailTransactions] = useState<Transaction[]>(
    [],
  );
  const [undoPreview, setUndoPreview] = useState<UndoPreview | null>(null);
  const [confirmingUndo, setConfirmingUndo] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      setRecords(await importHistoryApi.list());
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  async function openDetail(record: ImportRecord) {
    setSelected(record);
    setConfirmingUndo(false);
    setUndoPreview(null);
    setError(null);
    try {
      setDetailTransactions(
        await transactionsApi.list({
          source_import_id: record.id,
          include_archived: true,
        }),
      );
    } catch (err) {
      setError(String(err));
    }
  }

  async function startUndo() {
    if (!selected) return;
    setBusy(true);
    setError(null);
    try {
      setUndoPreview(await importHistoryApi.previewUndo(selected.id));
      setConfirmingUndo(true);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function confirmUndo() {
    if (!selected) return;
    setBusy(true);
    setError(null);
    try {
      await importHistoryApi.undo(selected.id);
      setConfirmingUndo(false);
      await refresh();
      const refreshed = (await importHistoryApi.list()).find(
        (r) => r.id === selected.id,
      );
      if (refreshed) await openDetail(refreshed);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <dialog
      ref={dialogRef}
      className="account-dialog import-history-dialog"
      aria-labelledby="import-history-title"
      onClose={onClose}
    >
      <h2 id="import-history-title">
        {selected ? selected.source_filename : "Import History"}
      </h2>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {!selected && (
        <>
          {loading ? (
            <p className="muted">Loading…</p>
          ) : records.length === 0 ? (
            <p className="muted">No imports yet.</p>
          ) : (
            <ul className="import-history-list">
              {records.map((r) => (
                <li key={r.id}>
                  <button
                    type="button"
                    className="import-history-item"
                    onClick={() => openDetail(r)}
                  >
                    <span className="import-history-filename">
                      {r.source_filename}
                      {r.undone_at && (
                        <span className="badge-archived">Undone</span>
                      )}
                    </span>
                    <span className="muted">{r.account_name}</span>
                    <span className="import-history-counts">
                      {r.committed_count} imported · {r.duplicate_count}{" "}
                      duplicates · {r.needs_review_count} need review
                    </span>
                    <span className="muted">{formatDate(r.imported_at)}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
          <div className="dialog-actions">
            <button type="button" className="btn-primary" onClick={onClose}>
              Close
            </button>
          </div>
        </>
      )}

      {selected && (
        <div className="import-step">
          <ul className="import-summary-counts">
            <li>{selected.account_name}</li>
            <li>{formatDate(selected.imported_at)}</li>
            {selected.import_profile_name && (
              <li>Profile: {selected.import_profile_name}</li>
            )}
          </ul>
          <ul className="import-summary-counts">
            <li>
              <strong>{selected.committed_count}</strong> imported
            </li>
            <li>
              <strong>{selected.duplicate_count}</strong> duplicates
            </li>
            <li>
              <strong>{selected.needs_review_count}</strong> needed review
            </li>
            {selected.excluded_count > 0 && (
              <li>
                <strong>{selected.excluded_count}</strong> excluded
              </li>
            )}
          </ul>

          {selected.undone_at ? (
            <p className="muted">
              Undone {formatDate(selected.undone_at)} -{" "}
              {selected.undone_removed_count} transaction
              {selected.undone_removed_count === 1 ? "" : "s"} removed,{" "}
              {selected.remaining_transaction_count} remain
              {selected.remaining_transaction_count === 1 ? "s" : ""}.
            </p>
          ) : (
            <p className="muted">
              {selected.remaining_transaction_count} transaction
              {selected.remaining_transaction_count === 1 ? "" : "s"} from this
              import still in the ledger.
            </p>
          )}

          <div className="table-scroll import-preview-scroll">
            <table className="accounts-table">
              <thead>
                <tr>
                  <th scope="col">Date</th>
                  <th scope="col">Description</th>
                  <th scope="col" className="col-balance">
                    Amount
                  </th>
                </tr>
              </thead>
              <tbody>
                {detailTransactions.map((t) => (
                  <tr
                    key={t.id}
                    className={t.is_archived ? "row-archived" : undefined}
                  >
                    <td>{formatDate(t.transaction_date)}</td>
                    <td>{t.merchant}</td>
                    <td className="col-balance">
                      {formatCents(t.amount_cents)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {confirmingUndo && undoPreview && (
            <p className="import-recognition-banner" role="alert">
              Undo {selected.source_filename}?{" "}
              {undoPreview.total_transaction_count} transaction
              {undoPreview.total_transaction_count === 1 ? "" : "s"} were
              created by this import. {undoPreview.removable_count} can be
              safely removed.{" "}
              {undoPreview.protected_count > 0 &&
                `${undoPreview.protected_count} were modified after import and will remain.`}
            </p>
          )}

          <div className="dialog-actions">
            <button
              type="button"
              className="btn-secondary"
              onClick={() => setSelected(null)}
            >
              Back
            </button>
            {!selected.undone_at &&
              selected.remaining_transaction_count > 0 &&
              (confirmingUndo ? (
                <button
                  type="button"
                  className="btn-primary"
                  onClick={confirmUndo}
                  disabled={busy}
                >
                  {busy ? "Undoing…" : "Confirm undo"}
                </button>
              ) : (
                <button
                  type="button"
                  className="btn-secondary"
                  onClick={startUndo}
                  disabled={busy}
                >
                  Undo import
                </button>
              ))}
          </div>
        </div>
      )}
    </dialog>
  );
}

export default ImportHistoryDialog;
