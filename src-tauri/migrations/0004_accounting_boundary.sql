-- Phase 4A: accounting boundary hardening.
--
-- 1. Widen transaction_type to add 'opening_balance' - a structural anchor
--    transaction (never spending/income/transfer) that lets MIDAS establish
--    a trustworthy balance baseline when only partial history is imported.
--    SQLite has no ALTER TABLE ... ALTER COLUMN for CHECK constraints, so
--    the table is rebuilt (ADR-0007).
CREATE TABLE transactions_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id),
    transaction_date TEXT NOT NULL,
    posted_date TEXT,
    amount_cents INTEGER NOT NULL,
    transaction_type TEXT NOT NULL CHECK (
        transaction_type IN ('purchase', 'income', 'transfer', 'refund', 'opening_balance')
    ),
    raw_description TEXT NOT NULL,
    merchant TEXT NOT NULL,
    category TEXT,
    subcategory TEXT,
    notes TEXT,
    tags TEXT,
    is_pending INTEGER CHECK (is_pending IN (0, 1)),
    is_archived INTEGER NOT NULL DEFAULT 0 CHECK (is_archived IN (0, 1)),
    review_status TEXT NOT NULL DEFAULT 'clear' CHECK (
        review_status IN ('clear', 'needs_review')
    ),
    source_import_id INTEGER REFERENCES imports (id),
    source_row_hash TEXT,
    transfer_pair_transaction_id INTEGER REFERENCES transactions (id),
    refund_of_transaction_id INTEGER REFERENCES transactions (id),
    recurring_series_id INTEGER REFERENCES recurring_series (id),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

INSERT INTO transactions_new (
    id, account_id, transaction_date, posted_date, amount_cents, transaction_type,
    raw_description, merchant, category, subcategory, notes, tags, is_pending,
    is_archived, review_status, source_import_id, source_row_hash,
    transfer_pair_transaction_id, refund_of_transaction_id, recurring_series_id,
    created_at, updated_at
)
SELECT
    id, account_id, transaction_date, posted_date, amount_cents, transaction_type,
    raw_description, merchant, category, subcategory, notes, tags, is_pending,
    is_archived, review_status, source_import_id, source_row_hash,
    transfer_pair_transaction_id, refund_of_transaction_id, recurring_series_id,
    created_at, updated_at
FROM transactions;

DROP TABLE transactions;
ALTER TABLE transactions_new RENAME TO transactions;

CREATE INDEX idx_transactions_dedup ON transactions (account_id, source_row_hash);
CREATE INDEX idx_transactions_account ON transactions (account_id, is_archived);

-- At most one opening-balance anchor per account - the reconcile mechanism
-- (accounting.rs) upserts this single row rather than accumulating many.
CREATE UNIQUE INDEX idx_one_opening_balance_per_account
    ON transactions (account_id)
    WHERE transaction_type = 'opening_balance';

-- 2. accounts.balance_cents is superseded by a live SUM(transactions.amount_cents)
--    computation (ADR-0007) - drop the stored column so there is exactly one
--    source of truth for balance and no code path can let it drift from the
--    ledger. Any previously-entered balance_cents value is intentionally not
--    migrated into an opening_balance transaction: this repository has never
--    held real financial data (dev/test fixtures only, deleted after each
--    verification pass - see every phase spec's Security section), so there
--    is nothing real to preserve, and inventing a migration heuristic for
--    data that doesn't exist would be speculative complexity.
ALTER TABLE accounts DROP COLUMN balance_cents;
