-- Savings goals (Phase 4B). current_amount_cents is a plain user-set field,
-- not auto-tracked from a linked account or category - nothing in the
-- request asked for that linkage, and inventing one would be exactly the
-- kind of unrequested mechanism this project's own discipline avoids.
CREATE TABLE savings_goals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    target_amount_cents INTEGER NOT NULL CHECK (target_amount_cents > 0),
    current_amount_cents INTEGER NOT NULL DEFAULT 0 CHECK (current_amount_cents >= 0),
    target_date TEXT, -- nullable ISO date
    -- NULL = this goal does not reserve anything from Safe to Spend - the
    -- owner must explicitly set an amount to protect it.
    reserved_amount_cents INTEGER CHECK (
        reserved_amount_cents IS NULL OR reserved_amount_cents >= 0
    ),
    is_archived INTEGER NOT NULL DEFAULT 0 CHECK (is_archived IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
