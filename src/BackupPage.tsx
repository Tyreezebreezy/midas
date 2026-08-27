import { useState } from "react";
import type { BackupResult, RestoreValidation } from "./api/midas";
import { backupApi, transactionsApi } from "./api/midas";

function downloadBlob(filename: string, blob: Blob) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

function base64ToBytes(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

async function fileToBase64(file: File): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  const chunkSize = 8192;
  for (let i = 0; i < bytes.length; i += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunkSize));
  }
  return btoa(binary);
}

function todayStamp(): string {
  return new Date().toISOString().slice(0, 10);
}

function BackupPage() {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [lastBackup, setLastBackup] = useState<BackupResult | null>(null);

  const [restoreFile, setRestoreFile] = useState<{
    name: string;
    base64: string;
  } | null>(null);
  const [restorePreview, setRestorePreview] =
    useState<RestoreValidation | null>(null);
  const [restoreDone, setRestoreDone] = useState<RestoreValidation | null>(
    null,
  );

  async function handleBackup() {
    setError(null);
    setBusy(true);
    try {
      const result = await backupApi.create();
      downloadBlob(
        result.filename,
        new Blob([base64ToBytes(result.content_base64)], {
          type: "application/x-sqlite3",
        }),
      );
      setLastBackup(result);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleExportCsv() {
    setError(null);
    setBusy(true);
    try {
      const csv = await transactionsApi.exportCsv();
      downloadBlob(
        `midas-transactions-${todayStamp()}.csv`,
        new Blob([csv], { type: "text/csv" }),
      );
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleFileChosen(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    setError(null);
    setBusy(true);
    setRestorePreview(null);
    setRestoreDone(null);
    setRestoreFile(null);
    try {
      const base64 = await fileToBase64(file);
      const preview = await backupApi.previewRestore(base64);
      setRestoreFile({ name: file.name, base64 });
      setRestorePreview(preview);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
      e.target.value = "";
    }
  }

  async function handleConfirmRestore() {
    if (!restoreFile) return;
    setError(null);
    setBusy(true);
    try {
      const result = await backupApi.restore(restoreFile.base64);
      setRestoreDone(result);
      setRestorePreview(null);
      setRestoreFile(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  function cancelRestore() {
    setRestoreFile(null);
    setRestorePreview(null);
  }

  return (
    <section aria-labelledby="backup-heading">
      <h2 id="backup-heading">Backup &amp; Restore</h2>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      <h2 className="section-heading">Backup</h2>
      <p className="muted">
        Downloads a complete copy of your MIDAS database - accounts,
        transactions, budgets, everything. Keep it somewhere safe; it's
        everything needed to restore your data on this or another computer.
      </p>
      <button
        type="button"
        className="btn-primary"
        onClick={handleBackup}
        disabled={busy}
      >
        {busy ? "Working…" : "Download backup"}
      </button>
      {lastBackup && (
        <p className="muted">
          Saved {lastBackup.filename} - {lastBackup.account_count} accounts,{" "}
          {lastBackup.transaction_count} transactions.
        </p>
      )}

      <h2 className="section-heading">Export transactions (CSV)</h2>
      <p className="muted">
        A plain spreadsheet of every transaction - readable in Excel or Google
        Sheets, with or without MIDAS installed.
      </p>
      <button
        type="button"
        className="btn-secondary"
        onClick={handleExportCsv}
        disabled={busy}
      >
        Export CSV
      </button>

      <h2 className="section-heading">Restore from backup</h2>
      <p className="muted">
        Choose a MIDAS backup file. Nothing changes until you review what's in
        it and explicitly confirm - and your current database is always
        preserved on disk first, never deleted.
      </p>
      <div className="field">
        <label htmlFor="restore-file">Backup file</label>
        <input
          id="restore-file"
          type="file"
          accept=".sqlite3,.db"
          onChange={handleFileChosen}
          disabled={busy}
        />
      </div>

      {restorePreview && restoreFile && (
        <div className="review-item">
          <p>
            <strong>{restoreFile.name}</strong>: schema version{" "}
            {restorePreview.schema_version}, {restorePreview.account_count}{" "}
            accounts, {restorePreview.transaction_count} transactions.
            {restorePreview.needs_migration &&
              " This backup will be upgraded to the current schema when restored."}
          </p>
          <p className="form-error">
            Restoring replaces MIDAS's current data with this backup's data.
            Your current database will be saved to a timestamped file first, but
            MIDAS will use this backup going forward.
          </p>
          <div className="dialog-actions">
            <button
              type="button"
              className="btn-secondary"
              onClick={cancelRestore}
              disabled={busy}
            >
              Cancel
            </button>
            <button
              type="button"
              className="btn-primary"
              onClick={handleConfirmRestore}
              disabled={busy}
            >
              {busy ? "Restoring…" : "Restore this backup"}
            </button>
          </div>
        </div>
      )}

      {restoreDone && (
        <div className="review-item">
          <p>
            Restored: {restoreDone.account_count} accounts,{" "}
            {restoreDone.transaction_count} transactions.
          </p>
          <button
            type="button"
            className="btn-primary"
            onClick={() => window.location.reload()}
          >
            Reload MIDAS
          </button>
        </div>
      )}
    </section>
  );
}

export default BackupPage;
