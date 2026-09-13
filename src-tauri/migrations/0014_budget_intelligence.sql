-- Budget Intelligence (product handoff Batch 7, specs/phase-015). The
-- existing `budgets` table (0005) is already one row per category *per
-- month* - historical stability and "future month overrides" both fall out
-- of that for free (editing October's row never touches August's), so no
-- new base/override table is introduced. This migration only widens what
-- one row can express: a third classification value, and an opt-in
-- rollover flag. SQLite has no ALTER TABLE ... ALTER COLUMN for CHECK
-- constraints, so the table is rebuilt, same technique as migrations
-- 0004/0007/0013.
CREATE TABLE budgets_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    category TEXT NOT NULL CHECK (length(trim(category)) > 0),
    month TEXT NOT NULL,
    planned_amount_cents INTEGER NOT NULL CHECK (planned_amount_cents >= 0),
    classification TEXT NOT NULL DEFAULT 'flexible' CHECK (
        classification IN ('fixed', 'flexible', 'discretionary')
    ),
    rollover_enabled INTEGER NOT NULL DEFAULT 0 CHECK (rollover_enabled IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (category, month)
);

INSERT INTO budgets_new (
    id, category, month, planned_amount_cents, classification, created_at, updated_at
)
SELECT id, category, month, planned_amount_cents, classification, created_at, updated_at
FROM budgets;

DROP TABLE budgets;
ALTER TABLE budgets_new RENAME TO budgets;

CREATE INDEX idx_budgets_month ON budgets (month);
