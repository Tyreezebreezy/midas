# MIDAS UX Contract: Import transactions

## Intent

Let the owner bring a statement into MIDAS without fear of duplicate, malformed, or silently misclassified ledger data.

## Supported workflow

1. Select an existing account or add one.
2. Select a supported CSV/OFX/QFX source.
3. Inspect detected columns/source metadata and adjust mapping where applicable.
4. Review staged valid, duplicate, and invalid rows.
5. Confirm the import.
6. Review the summary and route ambiguous items to Needs Review.
7. Undo only through the guarded import-history workflow when eligible.

## Required states

- no accounts and inline account creation;
- valid source preview;
- invalid or unsupported source;
- required-field and date/amount parsing errors;
- exact/probable duplicate handling;
- partial valid/invalid input without silent row loss;
- cancellation before commit;
- confirmed commit, provenance, restart persistence, and guarded undo;
- keyboard-visible focus and tested desktop/narrow layouts.

This contract describes implemented behavior only. It does not add bank sync, OCR, cloud storage, automated financial decisions, or support for a format not present in the repository.

## Safety and ownership

No row enters ledger truth before explicit confirmation. Ambiguous input remains reviewable. Deterministic duplicate handling, transaction boundaries, source provenance, backup/restore, and owner confirmation outrank convenience.

## Evidence

- `specs/phase-009-import-engine-v2/spec.md`
- `specs/phase-010-ofx-source-adapter/spec.md`
- `src-tauri/src/import.rs`
- `src-tauri/src/ofx.rs`
- `src/ImportWizard.tsx`
