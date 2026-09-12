-- OFX/QFX source adapter (specs/phase-010). Migrations 0008 and 0009 are
-- immutable once committed/pushed - this is the next migration, not an
-- edit to either.

-- Remembers which MIDAS account a structured OFX/QFX source account maps
-- to, so a later import from the same statement recognizes it instead of
-- asking again (product handoff section 15 - only ever set when the owner
-- explicitly opts in to "Remember this account mapping", never guessed).
-- Distinct from `import_profiles` (which describes how to interpret CSV
-- *columns*): this maps a structured source *account identity*
-- (institution + account number/type) to a MIDAS account - a different
-- concept the CSV model has no use for. `bank_id`/`source_account_type`
-- are nullable because not every OFX account carries them (a credit card's
-- <CCACCTFROM> has no routing-style bank id; some statements omit type).
-- COALESCE-in-the-index treats NULL and '' as the same "absent" identity
-- consistently, since SQLite's UNIQUE treats NULL as distinct from every
-- other NULL and would otherwise allow duplicate mappings with a missing
-- bank_id.
CREATE TABLE ofx_account_mappings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    bank_id TEXT,
    source_account_id TEXT NOT NULL CHECK (length(trim(source_account_id)) > 0),
    source_account_type TEXT,
    account_id INTEGER NOT NULL REFERENCES accounts (id),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE UNIQUE INDEX idx_ofx_account_mappings_identity
    ON ofx_account_mappings (
        COALESCE(bank_id, ''),
        source_account_id,
        COALESCE(source_account_type, '')
    );

-- Minimal statement-level metadata (product handoff section 28): the
-- OFX/QFX ledger balance and its as-of date, when the statement provides
-- one. Nothing else about the source document is preserved (no raw-file
-- storage - see section 24), so this is the one fact that would otherwise
-- be permanently lost after import; keeping it now avoids a
-- reparsing/import-history limitation for a future Reconciliation batch.
-- Always NULL for CSV imports.
ALTER TABLE imports ADD COLUMN statement_balance_cents INTEGER;
ALTER TABLE imports ADD COLUMN statement_balance_date TEXT;
