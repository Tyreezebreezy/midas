import { useEffect, useRef, useState } from "react";
import type {
  Account,
  ColumnMapping,
  CsvPreview,
  ImportSummary,
  StagingResult,
} from "./api/midas";
import { importApi } from "./api/midas";
import { formatCents } from "./lib/currency";

interface CsvImportWizardProps {
  accounts: Account[];
  onClose: () => void;
  onImported: () => void;
}

type Step = "select" | "map" | "preview" | "result";

const EMPTY_MAPPING: ColumnMapping = {
  date_column: null,
  posted_date_column: null,
  description_column: null,
  amount_column: null,
  debit_column: null,
  credit_column: null,
  category_column: null,
};

function mappingUsable(m: ColumnMapping): boolean {
  return (
    Boolean(m.date_column) &&
    Boolean(m.description_column) &&
    (Boolean(m.amount_column) ||
      (Boolean(m.debit_column) && Boolean(m.credit_column)))
  );
}

function CsvImportWizard({
  accounts,
  onClose,
  onImported,
}: CsvImportWizardProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [step, setStep] = useState<Step>("select");
  const [accountId, setAccountId] = useState<number>(accounts[0]?.id ?? 0);
  const [filename, setFilename] = useState("");
  const [csvContent, setCsvContent] = useState("");
  const [csvPreview, setCsvPreview] = useState<CsvPreview | null>(null);
  const [useDebitCredit, setUseDebitCredit] = useState(false);
  const [mapping, setMapping] = useState<ColumnMapping>(EMPTY_MAPPING);
  const [staging, setStaging] = useState<StagingResult | null>(null);
  const [forceImport, setForceImport] = useState<Set<number>>(new Set());
  const [summary, setSummary] = useState<ImportSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  async function handleFileChange(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    setError(null);
    setBusy(true);
    try {
      const content = await file.text();
      const preview = await importApi.previewCsv(content);
      setFilename(file.name);
      setCsvContent(content);
      setCsvPreview(preview);
      setMapping(preview.suggested_mapping);
      setUseDebitCredit(
        !preview.suggested_mapping.amount_column &&
          Boolean(preview.suggested_mapping.debit_column),
      );
      setStep("map");
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleStage() {
    setError(null);
    setBusy(true);
    try {
      const effectiveMapping: ColumnMapping = useDebitCredit
        ? { ...mapping, amount_column: null }
        : { ...mapping, debit_column: null, credit_column: null };
      setMapping(effectiveMapping);
      const result = await importApi.stage(
        accountId,
        csvContent,
        effectiveMapping,
      );
      setStaging(result);
      setForceImport(new Set());
      setStep("preview");
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleCommit() {
    setError(null);
    setBusy(true);
    try {
      const result = await importApi.commit(
        accountId,
        filename,
        csvContent,
        mapping,
        Array.from(forceImport),
      );
      setSummary(result);
      setStep("result");
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  function toggleForce(rowIndex: number) {
    setForceImport((prev) => {
      const next = new Set(prev);
      if (next.has(rowIndex)) next.delete(rowIndex);
      else next.add(rowIndex);
      return next;
    });
  }

  const headers = csvPreview?.headers ?? [];
  const willImportCount = (staging?.valid_count ?? 0) + forceImport.size;

  return (
    <dialog
      ref={dialogRef}
      className="account-dialog import-dialog"
      aria-labelledby="import-title"
      onClose={onClose}
    >
      <h2 id="import-title">Import CSV{filename ? `: ${filename}` : ""}</h2>
      <ol className="import-steps" aria-label="Import progress">
        {(["select", "map", "preview", "result"] as Step[]).map((s) => (
          <li
            key={s}
            className={s === step ? "current" : undefined}
            aria-current={s === step ? "step" : undefined}
          >
            {s[0].toUpperCase() + s.slice(1)}
          </li>
        ))}
      </ol>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {step === "select" && (
        <div className="import-step">
          <div className="field">
            <label htmlFor="import-account">Import into account</label>
            <select
              id="import-account"
              value={accountId}
              onChange={(e) => setAccountId(Number(e.target.value))}
            >
              {accounts.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                </option>
              ))}
            </select>
          </div>
          <div className="field">
            <label htmlFor="import-file">CSV file</label>
            <input
              id="import-file"
              ref={fileInputRef}
              type="file"
              accept=".csv,text/csv"
              onChange={handleFileChange}
              disabled={busy}
            />
          </div>
          <details className="csv-help">
            <summary>Where do I get a CSV?</summary>
            <p>
              Most banks and credit-card providers let you download your
              transaction history from their website. Look for "Download",
              "Export", or "Download transactions" under your account activity
              and choose CSV when available.
            </p>
          </details>
          <div className="dialog-actions">
            <button type="button" className="btn-secondary" onClick={onClose}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {step === "map" && csvPreview && (
        <div className="import-step">
          <p className="muted">
            {csvPreview.row_count} rows detected. Confirm which columns hold
            what.
          </p>
          <MappingRow
            label="Date"
            value={mapping.date_column}
            headers={headers}
            onChange={(v) => setMapping({ ...mapping, date_column: v })}
          />
          <MappingRow
            label="Posted date (optional)"
            value={mapping.posted_date_column}
            headers={headers}
            onChange={(v) => setMapping({ ...mapping, posted_date_column: v })}
          />
          <MappingRow
            label="Description"
            value={mapping.description_column}
            headers={headers}
            onChange={(v) => setMapping({ ...mapping, description_column: v })}
          />
          <MappingRow
            label="Category (optional)"
            value={mapping.category_column}
            headers={headers}
            onChange={(v) => setMapping({ ...mapping, category_column: v })}
          />

          <div className="field field-checkbox">
            <label htmlFor="import-debit-credit">
              <input
                id="import-debit-credit"
                type="checkbox"
                checked={useDebitCredit}
                onChange={(e) => setUseDebitCredit(e.target.checked)}
              />
              This file has separate debit/credit columns instead of one signed
              amount column
            </label>
          </div>

          {useDebitCredit ? (
            <>
              <MappingRow
                label="Debit"
                value={mapping.debit_column}
                headers={headers}
                onChange={(v) => setMapping({ ...mapping, debit_column: v })}
              />
              <MappingRow
                label="Credit"
                value={mapping.credit_column}
                headers={headers}
                onChange={(v) => setMapping({ ...mapping, credit_column: v })}
              />
            </>
          ) : (
            <MappingRow
              label="Amount"
              value={mapping.amount_column}
              headers={headers}
              onChange={(v) => setMapping({ ...mapping, amount_column: v })}
            />
          )}

          <div className="dialog-actions">
            <button
              type="button"
              className="btn-secondary"
              onClick={onClose}
              disabled={busy}
            >
              Cancel
            </button>
            <button
              type="button"
              className="btn-primary"
              onClick={handleStage}
              disabled={
                busy ||
                !mappingUsable(
                  useDebitCredit
                    ? { ...mapping, amount_column: null }
                    : { ...mapping, debit_column: null, credit_column: null },
                )
              }
            >
              {busy ? "Reading…" : "Preview"}
            </button>
          </div>
        </div>
      )}

      {step === "preview" && staging && (
        <div className="import-step">
          <ul className="import-summary-counts">
            <li>
              <strong>{staging.valid_count}</strong> ready to import
            </li>
            <li>
              <strong>{staging.exact_duplicate_count}</strong> exact duplicates
              (skipped automatically)
            </li>
            <li>
              <strong>{staging.probable_duplicate_count}</strong> possible
              duplicates (your decision)
            </li>
            <li>
              <strong>{staging.invalid_count}</strong> could not be parsed (sent
              to Needs Review)
            </li>
          </ul>
          <p className="muted">
            <strong>{willImportCount}</strong> transaction
            {willImportCount === 1 ? "" : "s"} will be imported.
          </p>

          <div className="table-scroll import-preview-scroll">
            <table className="accounts-table">
              <thead>
                <tr>
                  <th scope="col">Row</th>
                  <th scope="col">Date</th>
                  <th scope="col">Description</th>
                  <th scope="col" className="col-balance">
                    Amount
                  </th>
                  <th scope="col">Status</th>
                  <th scope="col">
                    <span className="sr-only">Import anyway</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {staging.rows.map((row) => (
                  <tr key={row.row_index}>
                    <td>{row.row_index + 1}</td>
                    <td>{row.proposed?.transaction_date ?? "—"}</td>
                    <td>
                      {row.proposed?.raw_description ??
                        Object.values(row.raw)[0] ??
                        "—"}
                    </td>
                    <td className="col-balance">
                      {row.proposed
                        ? formatCents(row.proposed.amount_cents)
                        : "—"}
                    </td>
                    <td>
                      <RowStatusBadge row={row} />
                    </td>
                    <td>
                      {row.status === "probable_duplicate" && (
                        <label className="force-import-toggle">
                          <input
                            type="checkbox"
                            checked={forceImport.has(row.row_index)}
                            onChange={() => toggleForce(row.row_index)}
                          />
                          Import anyway
                        </label>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <div className="dialog-actions">
            <button
              type="button"
              className="btn-secondary"
              onClick={() => setStep("map")}
              disabled={busy}
            >
              Back
            </button>
            <button
              type="button"
              className="btn-primary"
              onClick={handleCommit}
              disabled={busy || willImportCount === 0}
            >
              {busy
                ? "Importing…"
                : `Import ${willImportCount} transaction${willImportCount === 1 ? "" : "s"}`}
            </button>
          </div>
        </div>
      )}

      {step === "result" && summary && (
        <div className="import-step">
          <p className="import-result-headline">
            <strong>{summary.committed_count}</strong> transaction
            {summary.committed_count === 1 ? "" : "s"} imported.
          </p>
          <ul className="import-summary-counts">
            <li>{summary.exact_duplicate_count} exact duplicates skipped</li>
            <li>{summary.sent_to_review_count} sent to Needs Review</li>
            <li>{summary.invalid_count} malformed rows sent to Needs Review</li>
          </ul>
          <div className="dialog-actions">
            <button type="button" className="btn-primary" onClick={onImported}>
              Done
            </button>
          </div>
        </div>
      )}
    </dialog>
  );
}

function MappingRow({
  label,
  value,
  headers,
  onChange,
}: {
  label: string;
  value: string | null;
  headers: string[];
  onChange: (v: string | null) => void;
}) {
  return (
    <div className="field mapping-row">
      <label>{label}</label>
      <select
        value={value ?? ""}
        onChange={(e) =>
          onChange(e.target.value === "" ? null : e.target.value)
        }
      >
        <option value="">Not mapped</option>
        {headers.map((h) => (
          <option key={h} value={h}>
            {h}
          </option>
        ))}
      </select>
    </div>
  );
}

function RowStatusBadge({ row }: { row: StagingResult["rows"][number] }) {
  switch (row.status) {
    case "valid":
      return <span className="badge-valid">Ready</span>;
    case "invalid":
      return (
        <span className="badge-review" title={row.errors.join("; ")}>
          Invalid
        </span>
      );
    case "exact_duplicate":
      return <span className="badge-archived">Already imported</span>;
    case "probable_duplicate":
      return <span className="badge-review">Possible duplicate</span>;
  }
}

export default CsvImportWizard;
