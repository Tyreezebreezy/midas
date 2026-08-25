-- Merchant normalization and categorization rules (product handoff section 8).
-- Preferred hierarchy is enforced in application code (rules.rs), not here:
-- explicit user decision > existing rule > built-in logic > Needs Review.
CREATE TABLE merchant_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    match_type TEXT NOT NULL CHECK (match_type IN ('exact', 'contains', 'starts_with')),
    match_value TEXT NOT NULL CHECK (length(trim(match_value)) > 0),
    account_id INTEGER REFERENCES accounts (id), -- NULL = applies to every account
    normalized_merchant TEXT,
    category TEXT,
    subcategory TEXT,
    priority INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    -- A rule that neither rewrites the merchant nor assigns a category does nothing.
    CHECK (normalized_merchant IS NOT NULL OR category IS NOT NULL)
);

CREATE TABLE recurring_series (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id),
    merchant TEXT NOT NULL,
    cadence TEXT NOT NULL CHECK (
        cadence IN ('weekly', 'biweekly', 'monthly', 'quarterly', 'annual')
    ),
    expected_amount_cents INTEGER NOT NULL,
    next_expected_date TEXT NOT NULL,
    category TEXT,
    status TEXT NOT NULL DEFAULT 'proposed' CHECK (
        status IN ('proposed', 'confirmed', 'rejected')
    ),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_recurring_series_account ON recurring_series (account_id, status);

-- Links a transaction to the recurring series it was matched against, if any.
-- Nullable and populated only by the detection/confirmation flow - most
-- transactions are not part of a recurring series.
ALTER TABLE transactions ADD COLUMN recurring_series_id INTEGER REFERENCES recurring_series (id);
