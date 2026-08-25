CREATE TABLE imports (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id),
    source_filename TEXT NOT NULL,
    file_hash TEXT NOT NULL,
    row_count INTEGER NOT NULL,
    committed_count INTEGER NOT NULL,
    duplicate_count INTEGER NOT NULL,
    needs_review_count INTEGER NOT NULL,
    column_mapping_json TEXT NOT NULL,
    imported_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- Sign convention (ADR-0004): amount_cents is signed relative to the effect
-- on THIS account's own balance - positive increases it, negative decreases
-- it - for every account type uniformly. transaction_type carries the
-- semantic classification that spending/income reports actually read.
CREATE TABLE transactions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id),
    transaction_date TEXT NOT NULL,
    posted_date TEXT,
    amount_cents INTEGER NOT NULL,
    transaction_type TEXT NOT NULL CHECK (
        transaction_type IN ('purchase', 'income', 'transfer', 'refund')
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
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- Load-bearing for duplicate/idempotence detection (not decorative - every
-- staged import row checks this on the hot path).
CREATE INDEX idx_transactions_dedup ON transactions (account_id, source_row_hash);
CREATE INDEX idx_transactions_account ON transactions (account_id, is_archived);

CREATE TABLE needs_review_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_type TEXT NOT NULL CHECK (
        issue_type IN (
            'malformed_row', 'missing_required_field', 'uncertain_category',
            'uncertain_merchant', 'suspected_duplicate',
            'ambiguous_debit_credit', 'ambiguous_transfer', 'other'
        )
    ),
    transaction_id INTEGER REFERENCES transactions (id),
    import_id INTEGER REFERENCES imports (id),
    source_row_json TEXT,
    detected_reason TEXT NOT NULL,
    proposed_resolution_json TEXT,
    resolution_status TEXT NOT NULL DEFAULT 'open' CHECK (
        resolution_status IN ('open', 'resolved', 'dismissed')
    ),
    resolved_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_needs_review_status ON needs_review_items (resolution_status);
