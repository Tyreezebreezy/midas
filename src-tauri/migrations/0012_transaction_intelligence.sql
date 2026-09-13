-- Phase 012: Transaction Intelligence (specs/phase-012).
--
-- Split transactions: a parent transaction's amount_cents remains the one
-- and only ledger movement (balance/reconciliation never touch this table)
-- - splits are a reporting-layer breakdown of what that single movement was
-- actually for, never a second set of ledger rows. "Has splits" is derived
-- from whether any child rows exist for a transaction, not a separate
-- denormalized flag on the parent that could drift out of sync.
--
-- Everything else this batch needs already exists from Foundation Batch 1
-- (transactions.payee_id, transactions.is_one_time) - this batch makes them
-- settable, it does not add columns for them.
CREATE TABLE transaction_splits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    transaction_id INTEGER NOT NULL REFERENCES transactions (id),
    category TEXT,
    subcategory TEXT,
    amount_cents INTEGER NOT NULL,
    notes TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_transaction_splits_transaction ON transaction_splits (transaction_id);
