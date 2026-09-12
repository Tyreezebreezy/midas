-- Foundation Batch 1: schema/model groundwork for Import Engine v2,
-- reconciliation, payees, tags, splits, and provenance (specs/phase-008).
-- Nothing here changes existing behavior - every new column is nullable or
-- carries a default that reproduces today's behavior exactly, and nothing
-- yet reads them. The accounting boundary (accounting::spending_effect,
-- the live SUM(amount_cents) balance) is untouched.

-- Payees: foundation for a normalized Payee Library. Not yet populated or
-- linked from any UI; transactions.merchant remains the authoritative
-- display value until a later batch implements normalization + linking.
-- merged_into_payee_id lets a future "merge" operation redirect a payee
-- without ever deleting the row other data may still reference.
CREATE TABLE payees (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    payee_type TEXT NOT NULL DEFAULT 'merchant' CHECK (payee_type IN ('merchant', 'company', 'person')),
    notes TEXT,
    is_archived INTEGER NOT NULL DEFAULT 0 CHECK (is_archived IN (0, 1)),
    merged_into_payee_id INTEGER REFERENCES payees (id),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- Tags: normalized many-to-many, replacing the free-text transactions.tags
-- column in spirit (that column is left in place - see spec's explicit
-- non-goals - since it is otherwise unrelated to this new structure and
-- dropping it is not required for this foundation to work). A comma string
-- can't be filtered or joined on efficiently; this can.
CREATE TABLE tags (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE CHECK (length(trim(name)) > 0),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE transaction_tags (
    transaction_id INTEGER NOT NULL REFERENCES transactions (id),
    tag_id INTEGER NOT NULL REFERENCES tags (id),
    PRIMARY KEY (transaction_id, tag_id)
);

CREATE INDEX idx_transaction_tags_tag ON transaction_tags (tag_id);

-- Import profiles: named, reusable column-mapping memory per institution/
-- account (product handoff: "select bank file -> MIDAS recognizes the
-- profile -> preview -> import"). account_id NULL means the profile is not
-- tied to one account, mirroring merchant_rules' existing
-- account-id-nullable-means-global convention. Reuses the same JSON shape
-- as imports.column_mapping_json rather than inventing a second one.
CREATE TABLE import_profiles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    account_id INTEGER REFERENCES accounts (id),
    source_type TEXT NOT NULL DEFAULT 'csv' CHECK (source_type IN ('csv', 'ofx', 'qfx')),
    column_mapping_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- Provenance: which profile (if any) produced an import, and what source
-- format - CSV-only today, but the column exists so a future OFX/QFX/PDF
-- adapter does not need another migration just to say so.
ALTER TABLE imports ADD COLUMN source_type TEXT NOT NULL DEFAULT 'csv' CHECK (source_type IN ('csv', 'ofx', 'qfx', 'pdf'));
ALTER TABLE imports ADD COLUMN import_profile_id INTEGER REFERENCES import_profiles (id);

-- Transaction-level foundation columns.
ALTER TABLE transactions ADD COLUMN payee_id INTEGER REFERENCES payees (id);
ALTER TABLE transactions ADD COLUMN split_parent_transaction_id INTEGER REFERENCES transactions (id);
ALTER TABLE transactions ADD COLUMN external_transaction_id TEXT;
-- Distinct from is_pending (a bank-reported temporary-hold state): this is
-- the owner's own statement-reconciliation lifecycle, unrelated to whether
-- the bank has posted the transaction yet.
ALTER TABLE transactions ADD COLUMN reconciliation_status TEXT NOT NULL DEFAULT 'unreconciled'
    CHECK (reconciliation_status IN ('unreconciled', 'cleared', 'reconciled'));
-- An analytical flag (product handoff section 8/new-cycle section 7): lets
-- a future budget-suggestion or recurring-detection pass exclude a
-- transaction the owner has marked as a one-off from a baseline average,
-- without needing a special category for it.
ALTER TABLE transactions ADD COLUMN is_one_time INTEGER NOT NULL DEFAULT 0 CHECK (is_one_time IN (0, 1));

CREATE INDEX idx_transactions_payee ON transactions (payee_id);
CREATE INDEX idx_transactions_split_parent ON transactions (split_parent_transaction_id);
CREATE INDEX idx_transactions_reconciliation ON transactions (account_id, reconciliation_status);
-- A bank-guaranteed-unique ID (OFX/QFX) should actually enforce uniqueness
-- once a future importer populates it - a partial index so CSV-imported
-- rows (which never set this) never collide with each other on NULL.
CREATE UNIQUE INDEX idx_transactions_external_id_unique
    ON transactions (account_id, external_transaction_id)
    WHERE external_transaction_id IS NOT NULL;
