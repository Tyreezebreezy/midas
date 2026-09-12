-- Phase 011: account reconciliation + running balances (specs/phase-011).
--
-- A reconciliation record represents MIDAS and a real bank/credit-card
-- statement agreeing on an account's balance for one period - a stronger,
-- distinct claim from a transaction's own `reconciliation_status` (which
-- already existed from Foundation Batch 1). The reconciliation *workspace*
-- itself (choosing a period, entering a statement balance, clearing
-- transactions) is a live computation with no draft row - a row here is
-- only ever inserted once, at the moment the difference reaches zero and
-- the owner confirms. `cleared_balance_cents` is deliberately a stored
-- snapshot, not something recomputed later: comparing it against a fresh
-- recomputation at read time is exactly how a later edit to an included
-- transaction is detected as invalidating (see `reconciliation.rs`).
CREATE TABLE reconciliations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id),
    statement_start_date TEXT NOT NULL,
    statement_end_date TEXT NOT NULL,
    statement_ending_balance_cents INTEGER NOT NULL,
    cleared_balance_cents INTEGER NOT NULL,
    -- 'completed': verified and currently trusted. 'needs_review': was
    -- completed, but a later edit to one of its transactions changed the
    -- math (product handoff section 16) - never silently hidden as if
    -- still healthy. 'reopened': the owner explicitly reopened it (section
    -- 18); the record stays for audit history but no longer claims its
    -- period or transactions.
    status TEXT NOT NULL DEFAULT 'completed' CHECK (
        status IN ('completed', 'needs_review', 'reopened')
    ),
    reconciled_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    -- Optional provenance: which import supplied the statement balance this
    -- reconciliation used, when the owner chose to use imported OFX/QFX
    -- statement metadata (section 20) rather than typing it manually.
    source_import_id INTEGER REFERENCES imports (id),
    note TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- Every overlap/history query is scoped to one account and ordered/filtered
-- by date - a composite index serves both.
CREATE INDEX idx_reconciliations_account_period
    ON reconciliations (account_id, statement_end_date);

-- Which reconciliation (if any) a transaction was included in when it last
-- closed. Nullable: most transactions never belong to one. A transaction
-- belongs to at most one reconciliation at a time - periods never overlap
-- (enforced in reconciliation.rs, not schema, since SQLite range checks
-- need a trigger this codebase has no precedent for) - so a single FK
-- column is sufficient; no join table needed.
ALTER TABLE transactions ADD COLUMN reconciliation_id INTEGER REFERENCES reconciliations (id);
CREATE INDEX idx_transactions_reconciliation_id ON transactions (reconciliation_id);

-- OFX statement-period metadata (product handoff section 20). Batch 3
-- preserved the statement's ending balance and as-of date
-- (statement_balance_cents/statement_balance_date) but the parser walked
-- past BANKTRANLIST's DTSTART/DTEND without extracting them - this column
-- pair, plus the matching ofx.rs extraction, closes that gap so a
-- reconciliation can offer "use the imported statement period" as a
-- prefill (never automatic - the owner always confirms, section 7/20).
ALTER TABLE imports ADD COLUMN statement_start_date TEXT;
ALTER TABLE imports ADD COLUMN statement_end_date TEXT;
