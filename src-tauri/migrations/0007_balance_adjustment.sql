-- Phase 4C: separate "opening balance" (the account's true historical
-- starting point, set once, never moved) from "balance adjustment" (a dated
-- correction for drift discovered later, landing on its own reconciliation
-- date instead of rewriting history) - see ADR-0009. SQLite has no ALTER
-- TABLE ... ALTER COLUMN for CHECK constraints, so the table is rebuilt
-- again, same as migration 0004.
CREATE TABLE transactions_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id),
    transaction_date TEXT NOT NULL,
    posted_date TEXT,
    amount_cents INTEGER NOT NULL,
    transaction_type TEXT NOT NULL CHECK (
        transaction_type IN (
            'purchase', 'income', 'transfer', 'refund',
            'opening_balance', 'balance_adjustment'
        )
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

CREATE UNIQUE INDEX idx_one_opening_balance_per_account
    ON transactions (account_id)
    WHERE transaction_type = 'opening_balance';

-- At most one balance-adjustment per account per reconciliation date -
-- reconciling twice on the same date corrects that date's adjustment in
-- place; reconciling on a new date creates a new, separate one.
CREATE UNIQUE INDEX idx_one_adjustment_per_account_per_date
    ON transactions (account_id, transaction_date)
    WHERE transaction_type = 'balance_adjustment';
