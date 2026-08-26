-- Monthly category budgets (Phase 4B). Deliberately not an envelope system:
-- no rollover, no fund transfers between categories - one planned amount
-- per category per month, compared against actual spending computed by
-- accounting::total_spending (transfers excluded, refunds reduce actual).
CREATE TABLE budgets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    category TEXT NOT NULL CHECK (length(trim(category)) > 0),
    month TEXT NOT NULL, -- first-of-month ISO date, e.g. '2026-08-01'
    planned_amount_cents INTEGER NOT NULL CHECK (planned_amount_cents >= 0),
    classification TEXT NOT NULL DEFAULT 'flexible' CHECK (
        classification IN ('fixed', 'flexible')
    ),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (category, month)
);

CREATE INDEX idx_budgets_month ON budgets (month);
