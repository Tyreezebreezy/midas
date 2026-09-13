-- Recurring Intelligence (product handoff Batch 6, specs/phase-014). Extends
-- Phase 3's recurring_series (merchant-based detection, proposed/confirmed/
-- rejected) rather than replacing it - the detection shape (>=3 consistent
-- occurrences, per-merchant grouping, idempotent re-scan) already works and
-- is kept. SQLite has no ALTER TABLE ... ALTER COLUMN for CHECK constraints,
-- so the table is rebuilt, same technique as migrations 0004/0007.
--
-- New concepts, kept as three separate things per the handoff's own
-- distinction (never the same row): the pattern (this table), an expected
-- occurrence (recurring_occurrences, a projection - never a ledger
-- movement), and a real transaction (unchanged, transactions table).
CREATE TABLE recurring_series_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id),
    payee_id INTEGER REFERENCES payees (id),
    merchant TEXT NOT NULL,
    -- First-class direction (never inferred from amount sign - a liability
    -- account's recurring charge is a positive amount_cents per ADR-0004,
    -- which must still classify as an expense, not income).
    direction TEXT NOT NULL DEFAULT 'expense' CHECK (
        direction IN ('expense', 'income', 'transfer')
    ),
    kind TEXT NOT NULL DEFAULT 'other' CHECK (
        kind IN ('subscription', 'bill', 'membership', 'recurring_income', 'other')
    ),
    cadence TEXT NOT NULL CHECK (
        cadence IN ('weekly', 'biweekly', 'monthly', 'quarterly', 'annual', 'semimonthly')
    ),
    -- Only meaningful when cadence = 'semimonthly' (two days-of-month, same
    -- shape as pay_cycle.rs's own PaySchedule::Semimonthly).
    semimonthly_day_1 INTEGER,
    semimonthly_day_2 INTEGER,
    expected_amount_cents INTEGER NOT NULL,
    next_expected_date TEXT NOT NULL,
    category TEXT,
    status TEXT NOT NULL DEFAULT 'proposed' CHECK (
        status IN ('proposed', 'confirmed', 'rejected', 'inactive')
    ),
    -- Only meaningful when status = 'confirmed' - organizational intent, not
    -- an automation promise (MIDAS never cancels anything).
    planning_to_cancel INTEGER NOT NULL DEFAULT 0 CHECK (planning_to_cancel IN (0, 1)),
    notes TEXT,
    created_source TEXT NOT NULL DEFAULT 'detected' CHECK (
        created_source IN ('detected', 'manual')
    ),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

INSERT INTO recurring_series_new (
    id, account_id, merchant, cadence, expected_amount_cents, next_expected_date,
    category, status, created_at, updated_at, direction
)
SELECT
    id, account_id, merchant, cadence, expected_amount_cents, next_expected_date,
    category, status, created_at, updated_at,
    CASE WHEN expected_amount_cents >= 0 THEN 'income' ELSE 'expense' END
FROM recurring_series;

DROP TABLE recurring_series;
ALTER TABLE recurring_series_new RENAME TO recurring_series;

CREATE INDEX idx_recurring_series_account ON recurring_series (account_id, status);
CREATE INDEX idx_recurring_series_payee ON recurring_series (payee_id);

-- transactions.recurring_series_id already exists (migration 0003) and
-- still means "this real transaction is part of this series' history" -
-- unchanged. A separate, narrower link (below) records which *specific*
-- expected occurrence a transaction fulfilled.

-- An expected occurrence is a projection, never a ledger row: it has no
-- amount_cents that sums into any balance, and matching a transaction to
-- one never creates, edits, or duplicates that transaction. Persisted
-- (not regenerated on every read) so a skipped/matched/missed decision
-- and restart persistence are simple - regenerated idempotently out to a
-- rolling horizon rather than in bulk far into the future (section 26).
CREATE TABLE recurring_occurrences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    series_id INTEGER NOT NULL REFERENCES recurring_series (id),
    expected_date TEXT NOT NULL,
    -- Snapshot at generation time, not a live read of the series' current
    -- expected_amount_cents - so a later price-change on the series doesn't
    -- retroactively rewrite what an already-generated occurrence expected.
    expected_amount_cents INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'upcoming' CHECK (
        status IN ('upcoming', 'due', 'matched', 'missed', 'skipped', 'needs_review')
    ),
    matched_transaction_id INTEGER REFERENCES transactions (id),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (series_id, expected_date)
);

CREATE INDEX idx_recurring_occurrences_series ON recurring_occurrences (series_id, expected_date);
CREATE INDEX idx_recurring_occurrences_status ON recurring_occurrences (status, expected_date);
