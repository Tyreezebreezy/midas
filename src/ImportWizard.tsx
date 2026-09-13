import { useEffect, useId, useRef, useState } from "react";
import type {
  Account,
  ColumnMapping,
  CsvPreview,
  ImportProfile,
  ImportSummary,
  OfxPreview,
  StagingResult,
} from "./api/midas";
import { importApi, importProfilesApi, ofxApi } from "./api/midas";
import { formatCents } from "./lib/currency";
import { formatDate } from "./lib/dateFormat";

interface ImportWizardProps {
  accounts: Account[];
  onClose: () => void;
  onImported: () => void;
  onReviewNeeded?: () => void;
}

type SourceKind = "csv" | "ofx";
type Step = "select" | "map" | "preview" | "result";

const EMPTY_MAPPING: ColumnMapping = {
  date_column: null,
  posted_date_column: null,
  description_column: null,
  amount_column: null,
  debit_column: null,
  credit_column: null,
  category_column: null,
  external_id_column: null,
  invert_amount: false,
};

function mappingUsable(m: ColumnMapping): boolean {
  return (
    Boolean(m.date_column) &&
    Boolean(m.description_column) &&
    (Boolean(m.amount_column) ||
      (Boolean(m.debit_column) && Boolean(m.credit_column)))
  );
}

function suggestProfileName(filename: string): string {
  return filename
    .replace(/\.csv$/i, "")
    .replace(/[_-]+/g, " ")
    .trim();
}

function combineSummaries(summaries: ImportSummary[]): ImportSummary {
  return summaries.reduce(
    (total, s) => ({
      import_id: s.import_id,
      committed_count: total.committed_count + s.committed_count,
      duplicate_count: total.duplicate_count + s.duplicate_count,
      sent_to_review_count: total.sent_to_review_count + s.sent_to_review_count,
      invalid_count: total.invalid_count + s.invalid_count,
      excluded_count: total.excluded_count + s.excluded_count,
    }),
    {
      import_id: 0,
      committed_count: 0,
      duplicate_count: 0,
      sent_to_review_count: 0,
      invalid_count: 0,
      excluded_count: 0,
    },
  );
}

function ImportWizard({
  accounts,
  onClose,
  onImported,
  onReviewNeeded,
}: ImportWizardProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [sourceKind, setSourceKind] = useState<SourceKind>("csv");
  const [step, setStep] = useState<Step>("select");
  const [accountId, setAccountId] = useState<number>(accounts[0]?.id ?? 0);
  const [filename, setFilename] = useState("");

  // CSV-specific state
  const [csvContent, setCsvContent] = useState("");
  const [csvPreview, setCsvPreview] = useState<CsvPreview | null>(null);
  const [useDebitCredit, setUseDebitCredit] = useState(false);
  const [mapping, setMapping] = useState<ColumnMapping>(EMPTY_MAPPING);
  const [profiles, setProfiles] = useState<ImportProfile[]>([]);
  const [selectedProfileId, setSelectedProfileId] = useState<number | null>(
    null,
  );
  const [saveAsProfile, setSaveAsProfile] = useState(false);
  const [newProfileName, setNewProfileName] = useState("");

  // OFX/QFX-specific state
  const [ofxContent, setOfxContent] = useState("");
  const [ofxPreview, setOfxPreview] = useState<OfxPreview | null>(null);
  const [statementIndex, setStatementIndex] = useState(0);
  const [rememberMapping, setRememberMapping] = useState(true);
  const [completedSummaries, setCompletedSummaries] = useState<ImportSummary[]>(
    [],
  );

  // Shared preview/commit state
  const [staging, setStaging] = useState<StagingResult | null>(null);
  const [forceImport, setForceImport] = useState<Set<number>>(new Set());
  const [excludedRows, setExcludedRows] = useState<Set<number>>(new Set());
  const [summary, setSummary] = useState<ImportSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  useEffect(() => {
    importProfilesApi
      .list(false)
      .then(setProfiles)
      .catch(() => {
        // Profile recognition/reuse is a convenience, not required to import.
      });
  }, []);

  function applyProfile(profile: ImportProfile | null) {
    setSelectedProfileId(profile?.id ?? null);
    if (!profile) return;
    setMapping(profile.column_mapping);
    setUseDebitCredit(
      !profile.column_mapping.amount_column &&
        Boolean(profile.column_mapping.debit_column),
    );
  }

  async function handleFileChange(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    setError(null);
    setBusy(true);
    try {
      const content = await file.text();
      setFilename(file.name);

      if (sourceKind === "csv") {
        const preview = await importApi.previewCsv(content, accountId || null);
        setCsvContent(content);
        setCsvPreview(preview);
        setNewProfileName(suggestProfileName(file.name));
        if (preview.profile_recognition.outcome === "clear") {
          applyProfile(preview.profile_recognition.profile);
        } else {
          setSelectedProfileId(null);
          setMapping(preview.suggested_mapping);
          setUseDebitCredit(
            !preview.suggested_mapping.amount_column &&
              Boolean(preview.suggested_mapping.debit_column),
          );
        }
      } else {
        const preview = await ofxApi.preview(content, file.name);
        setOfxContent(content);
        setOfxPreview(preview);
        setStatementIndex(0);
        setCompletedSummaries([]);
        const first = preview.accounts[0];
        if (first?.existing_mapping_account_id) {
          setAccountId(first.existing_mapping_account_id);
        }
      }
      setStep("map");
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  function handleProfileSelect(value: string) {
    if (value === "") {
      applyProfile(null);
      return;
    }
    const profile = profiles.find((p) => p.id === Number(value));
    applyProfile(profile ?? null);
  }

  async function handleStage() {
    setError(null);
    setBusy(true);
    try {
      if (sourceKind === "csv") {
        const effectiveMapping: ColumnMapping = useDebitCredit
          ? { ...mapping, amount_column: null }
          : { ...mapping, debit_column: null, credit_column: null };
        setMapping(effectiveMapping);

        if (saveAsProfile && !selectedProfileId && newProfileName.trim()) {
          const created = await importProfilesApi.create({
            name: newProfileName.trim(),
            account_id: accountId,
            source_type: "csv",
            column_mapping: effectiveMapping,
          });
          setSelectedProfileId(created.id);
          setProfiles((prev) => [...prev, created]);
        }

        const result = await importApi.stage(
          accountId,
          csvContent,
          effectiveMapping,
        );
        setStaging(result);
      } else {
        const result = await ofxApi.stage(
          accountId,
          ofxContent,
          statementIndex,
          filename,
        );
        setStaging(result);
      }
      setForceImport(new Set());
      setExcludedRows(new Set());
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
      if (sourceKind === "csv") {
        const result = await importApi.commit(
          accountId,
          filename,
          csvContent,
          mapping,
          selectedProfileId,
          Array.from(forceImport),
          Array.from(excludedRows),
        );
        setSummary(result);
        setStep("result");
      } else {
        const result = await ofxApi.commit(
          accountId,
          filename,
          ofxContent,
          statementIndex,
          rememberMapping,
          Array.from(forceImport),
          Array.from(excludedRows),
        );
        setCompletedSummaries((prev) => [...prev, result]);
        setSummary(result);
        setStep("result");
      }
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  function continueToNextStatement() {
    if (!ofxPreview) return;
    const nextIndex = statementIndex + 1;
    setStatementIndex(nextIndex);
    setAccountId(
      ofxPreview.accounts[nextIndex]?.existing_mapping_account_id ??
        accounts[0]?.id ??
        0,
    );
    setStaging(null);
    setSummary(null);
    setStep("map");
  }

  function toggleForce(rowIndex: number) {
    setForceImport((prev) => {
      const next = new Set(prev);
      if (next.has(rowIndex)) next.delete(rowIndex);
      else next.add(rowIndex);
      return next;
    });
  }

  function toggleExcluded(rowIndex: number) {
    setExcludedRows((prev) => {
      const next = new Set(prev);
      if (next.has(rowIndex)) next.delete(rowIndex);
      else next.add(rowIndex);
      return next;
    });
  }

  const headers = csvPreview?.headers ?? [];
  const willImportCount =
    (staging?.new_count ?? 0) - excludedRows.size + forceImport.size;
  const relevantProfiles = profiles.filter(
    (p) => p.account_id === null || p.account_id === accountId,
  );
  const recognition = csvPreview?.profile_recognition;
  const currentOfxAccount = ofxPreview?.accounts[statementIndex] ?? null;
  const moreStatementsRemain =
    sourceKind === "ofx" &&
    ofxPreview !== null &&
    statementIndex + 1 < ofxPreview.accounts.length;
  const combinedSummary =
    sourceKind === "ofx" && completedSummaries.length > 1
      ? combineSummaries(completedSummaries)
      : summary;

  return (
    <dialog
      ref={dialogRef}
      className="account-dialog import-dialog"
      aria-labelledby="import-title"
      onClose={onClose}
    >
      <h2 id="import-title">Import{filename ? `: ${filename}` : ""}</h2>
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
          <div className="field field-checkbox amount-direction">
            <span className="field-legend">File type</span>
            <label>
              <input
                type="radio"
                name="source-kind"
                checked={sourceKind === "csv"}
                onChange={() => setSourceKind("csv")}
              />
              CSV - flexible bank/spreadsheet import
            </label>
            <label>
              <input
                type="radio"
                name="source-kind"
                checked={sourceKind === "ofx"}
                onChange={() => setSourceKind("ofx")}
              />
              OFX / QFX - structured bank statement import
            </label>
          </div>
          {sourceKind === "csv" && (
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
          )}
          <div className="field">
            <label htmlFor="import-file">
              {sourceKind === "csv" ? "CSV file" : "OFX/QFX file"}
            </label>
            <input
              id="import-file"
              ref={fileInputRef}
              type="file"
              accept={sourceKind === "csv" ? ".csv,text/csv" : ".ofx,.qfx"}
              onChange={handleFileChange}
              disabled={busy}
            />
          </div>
          {sourceKind === "csv" && (
            <details className="csv-help">
              <summary>Where do I get a CSV?</summary>
              <p>
                Most banks and credit-card providers let you download your
                transaction history from their website. Look for "Download",
                "Export", or "Download transactions" under your account activity
                and choose CSV when available.
              </p>
            </details>
          )}
          {sourceKind === "ofx" && (
            <p className="muted">
              MIDAS reads the account and transaction details directly from the
              file - you'll just confirm which MIDAS account it belongs to next.
            </p>
          )}
          <div className="dialog-actions">
            <button type="button" className="btn-secondary" onClick={onClose}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {step === "map" && sourceKind === "csv" && csvPreview && (
        <div className="import-step">
          <p className="muted">
            {csvPreview.row_count} rows detected. Confirm which columns hold
            what.
          </p>

          {recognition?.outcome === "clear" && (
            <p className="import-recognition-banner">
              Recognized as <strong>{recognition.profile.name}</strong>.{" "}
              <button
                type="button"
                className="btn-link"
                onClick={() => applyProfile(null)}
              >
                Map manually instead
              </button>
            </p>
          )}
          {recognition?.outcome === "ambiguous" && (
            <p className="muted">
              More than one saved profile could apply to this file - choose one
              below or map the columns manually.
            </p>
          )}

          {relevantProfiles.length > 0 && (
            <div className="field">
              <label htmlFor="import-profile-select">Saved mapping</label>
              <select
                id="import-profile-select"
                value={selectedProfileId ?? ""}
                onChange={(e) => handleProfileSelect(e.target.value)}
              >
                <option value="">Map manually</option>
                {relevantProfiles.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
              </select>
            </div>
          )}

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
          <MappingRow
            label="Transaction ID (optional)"
            value={mapping.external_id_column}
            headers={headers}
            onChange={(v) => setMapping({ ...mapping, external_id_column: v })}
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

          <div className="field field-checkbox">
            <label htmlFor="import-invert-amount">
              <input
                id="import-invert-amount"
                type="checkbox"
                checked={mapping.invert_amount}
                onChange={(e) =>
                  setMapping({ ...mapping, invert_amount: e.target.checked })
                }
              />
              This file's amounts are reversed (flip the sign)
            </label>
          </div>

          {selectedProfileId === null && (
            <div className="field field-checkbox">
              <label htmlFor="import-save-profile">
                <input
                  id="import-save-profile"
                  type="checkbox"
                  checked={saveAsProfile}
                  onChange={(e) => setSaveAsProfile(e.target.checked)}
                />
                Save this mapping as an import profile
              </label>
              {saveAsProfile && (
                <input
                  type="text"
                  aria-label="Profile name"
                  placeholder="e.g. Navy Federal Checking"
                  value={newProfileName}
                  onChange={(e) => setNewProfileName(e.target.value)}
                />
              )}
            </div>
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
                ) ||
                (saveAsProfile &&
                  selectedProfileId === null &&
                  !newProfileName.trim())
              }
            >
              {busy ? "Reading…" : "Preview"}
            </button>
          </div>
        </div>
      )}

      {step === "map" && sourceKind === "ofx" && currentOfxAccount && (
        <div className="import-step">
          {ofxPreview && ofxPreview.accounts.length > 1 && (
            <p className="muted">
              Statement {statementIndex + 1} of {ofxPreview.accounts.length} in
              this file.
            </p>
          )}
          <ul className="import-summary-counts">
            <li>
              {currentOfxAccount.kind === "bank"
                ? "Bank account"
                : "Credit card"}
            </li>
            <li>{currentOfxAccount.masked_account_id}</li>
            {currentOfxAccount.account_type && (
              <li>{currentOfxAccount.account_type}</li>
            )}
            <li>
              {currentOfxAccount.transaction_count} transaction
              {currentOfxAccount.transaction_count === 1 ? "" : "s"}
            </li>
          </ul>

          {currentOfxAccount.existing_mapping_account_id && (
            <p className="import-recognition-banner">
              Recognized as{" "}
              <strong>
                {accounts.find(
                  (a) => a.id === currentOfxAccount.existing_mapping_account_id,
                )?.name ?? "a known account"}
              </strong>
              .
            </p>
          )}

          <div className="field">
            <label htmlFor="ofx-account-select">MIDAS account</label>
            <select
              id="ofx-account-select"
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

          <div className="field field-checkbox">
            <label htmlFor="ofx-remember-mapping">
              <input
                id="ofx-remember-mapping"
                type="checkbox"
                checked={rememberMapping}
                onChange={(e) => setRememberMapping(e.target.checked)}
              />
              Remember this account mapping
            </label>
          </div>

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
              disabled={busy || !accountId}
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
              <strong>{staging.new_count}</strong> new
            </li>
            <li>
              <strong>{staging.duplicate_count}</strong> duplicates (skipped
              automatically)
            </li>
            <li>
              <strong>{staging.possible_match_count}</strong> possible matches
              (your decision)
            </li>
            <li>
              <strong>{staging.needs_review_count}</strong> could not be used
              (sent to Review)
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
                    <span className="sr-only">Include</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {staging.rows.map((row) => (
                  <tr key={row.row_index}>
                    <td>{row.row_index + 1}</td>
                    <td>
                      {row.candidate
                        ? formatDate(row.candidate.transaction_date)
                        : "—"}
                    </td>
                    <td>
                      {row.candidate?.raw_description ??
                        Object.values(row.raw)[0] ??
                        "—"}
                    </td>
                    <td className="col-balance">
                      {row.candidate
                        ? formatCents(row.candidate.amount_cents)
                        : "—"}
                    </td>
                    <td>
                      <RowStatusBadge row={row} />
                    </td>
                    <td>
                      {row.status === "new" && (
                        <label className="force-import-toggle">
                          <input
                            type="checkbox"
                            checked={!excludedRows.has(row.row_index)}
                            onChange={() => toggleExcluded(row.row_index)}
                          />
                          Include
                        </label>
                      )}
                      {row.status === "possible_match" && (
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
            {willImportCount === 0 && moreStatementsRemain ? (
              <button
                type="button"
                className="btn-primary"
                onClick={continueToNextStatement}
              >
                Next account
              </button>
            ) : (
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
            )}
          </div>
        </div>
      )}

      {step === "result" && combinedSummary && (
        <div className="import-step">
          <p className="import-result-headline">
            <strong>{combinedSummary.committed_count}</strong> transaction
            {combinedSummary.committed_count === 1 ? "" : "s"} imported.
          </p>
          <ul className="import-summary-counts">
            <li>{combinedSummary.duplicate_count} duplicates skipped</li>
            <li>
              {combinedSummary.sent_to_review_count} possible matches sent to
              Review
            </li>
            <li>{combinedSummary.invalid_count} rows could not be used</li>
            {combinedSummary.excluded_count > 0 && (
              <li>{combinedSummary.excluded_count} excluded by you</li>
            )}
          </ul>
          <div className="dialog-actions">
            {moreStatementsRemain ? (
              <button
                type="button"
                className="btn-primary"
                onClick={continueToNextStatement}
              >
                Import next account
              </button>
            ) : (
              <>
                {combinedSummary.sent_to_review_count +
                  combinedSummary.invalid_count >
                  0 &&
                  onReviewNeeded && (
                    <button
                      type="button"
                      className="btn-secondary"
                      onClick={() => {
                        onImported();
                        onReviewNeeded();
                      }}
                    >
                      Review{" "}
                      {combinedSummary.sent_to_review_count +
                        combinedSummary.invalid_count}
                    </button>
                  )}
                <button
                  type="button"
                  className="btn-primary"
                  onClick={onImported}
                >
                  Done
                </button>
              </>
            )}
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
  const id = useId();
  return (
    <div className="field mapping-row">
      <label htmlFor={id}>{label}</label>
      <select
        id={id}
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
    case "new":
      return <span className="badge-valid">New</span>;
    case "needs_review":
      return (
        <span className="badge-review" title={row.errors.join("; ")}>
          Needs review
        </span>
      );
    case "duplicate":
      return (
        <span className="badge-archived" title={row.match_detail ?? undefined}>
          Duplicate
        </span>
      );
    case "possible_match":
      return (
        <span className="badge-review" title={row.match_detail ?? undefined}>
          Possible match
        </span>
      );
  }
}

export default ImportWizard;
