-- Import Engine v2 (specs/phase-009): small additive columns needed to make
-- `imports` a first-class history record, plus archiving for
-- `import_profiles`. Migration 0008 is treated as immutable once
-- committed/pushed - this is the next migration, not an edit to that one.

-- Rows the user explicitly excluded from an otherwise-importable file.
-- Distinct from duplicate_count (recognized as already present) and
-- needs_review_count (malformed or ambiguous) - this is a deliberate user
-- choice, not something MIDAS detected.
ALTER TABLE imports ADD COLUMN excluded_count INTEGER NOT NULL DEFAULT 0;

-- Undo Import (product handoff section 22-25). NULL until the import is
-- undone. `undone_removed_count` records how many transactions were
-- actually removed at undo time; how many remain (protected by a later
-- edit) is computed live from `transactions.source_import_id`, the same
-- computed-not-stored approach ADR-0007 already uses for account balance,
-- rather than a second count that could drift.
ALTER TABLE imports ADD COLUMN undone_at TEXT;
ALTER TABLE imports ADD COLUMN undone_removed_count INTEGER;

-- Found during native verification: once a profile has been used for even
-- one import, `imports.import_profile_id`'s foreign key makes it
-- permanently undeletable - there was no way to retire a mistaken or
-- one-off profile from the picker. Same archive pattern every other
-- user-editable model in this codebase already uses (accounts, payees,
-- transactions, savings_goals).
ALTER TABLE import_profiles ADD COLUMN is_archived INTEGER NOT NULL DEFAULT 0 CHECK (is_archived IN (0, 1));
