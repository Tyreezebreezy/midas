# ADR-0003: SQLite persistence via rusqlite, sync, PRAGMA user_version migrations

- Status: accepted
- Date: 2026-08-25
- Supersedes: none

## Context and problem

Phase 1 needs durable local SQLite persistence for accounts and settings: reliable transactions, deterministic migrations, constraints, straightforward testing, maintainable Rust code, safe local persistence. The owner explicitly directed evaluating the smallest appropriate implementation rather than defaulting to the most capable one, and ruled out adding async database architecture unless MIDAS actually needs it.

## Options considered

1. `rusqlite` (sync, direct SQLite bindings) + a hand-rolled migration runner using SQLite's built-in `PRAGMA user_version`.
2. `sqlx` (async, compile-time-checked queries, connection pool) with its `migrate!` macro.
3. `rusqlite` + the `rusqlite_migration` crate (a small purpose-built migration wrapper) instead of hand-rolling the runner.

## Decision

Option 1. `rusqlite = { features = ["bundled"] }` for a single mutex-guarded `Connection` held in Tauri-managed state (`db::DbState`), plus a ~20-line hand-rolled migration runner (`db::migrate`) that reads `PRAGMA user_version`, applies any `src-tauri/migrations/*.sql` file whose index exceeds it inside a transaction, and bumps the pragma. `bundled` statically compiles SQLite into the binary so the installer has no external SQLite dependency.

## Rationale

- MIDAS is a local, single-user desktop app. There is no concurrent-writer load to justify `sqlx`'s connection pool or async runtime - a single mutex-guarded synchronous connection is not a shortcut, it is the correctly-sized solution, and it keeps every Tauri command a plain sync function (no `tokio` dependency anywhere in the app).
- `sqlx`'s compile-time query checking needs either a live database or a committed `.sqlx` offline cache to build - real value at scale, unjustified ceremony for nine hand-written queries in one phase.
- `PRAGMA user_version` is SQLite's own built-in mechanism for exactly this purpose (an integer stamped in the database file header, incremented as schema versions apply). Using it directly is a native-platform-feature choice over the `rusqlite_migration` crate: the runner is small enough (~20 lines) that the crate would trade a real dependency for saving less code than its own `Cargo.lock` entry costs. Reconsider `rusqlite_migration` (or move to `sqlx`) only if migrations grow branching/conditional logic the flat ordered-list runner can't express cleanly.
- Constraints (`CHECK`, `NOT NULL`) are enforced in the schema itself (`migrations/0001_initial.sql`), not only in Rust, so the database stays correct even if a future caller bypasses the `accounts`/`settings` Rust modules.
- Money is stored as `INTEGER` cents (`balance_cents`), never `REAL`/float, to avoid floating-point drift in financial data - a direct application of the constitution's "financial correctness outranks convenience" principle.

## Consequences and accepted tradeoffs

- No async story anywhere in the Rust backend yet. If a later phase needs genuine background work (e.g., a long-running import), that is a new, separately-justified decision, not an implicit consequence of this one.
- The migration runner has no down-migrations (forward-only). Acceptable for a pre-release local app with no shipped schema to roll back from yet; revisit before any release that ships a schema needing a reversible migration.
- `bundled` adds SQLite's own compile time to `cargo build` (observed: full clean build, including this and the rest of Phase 1's new dependencies, still finishes in about 50s) in exchange for a self-contained installer with no system SQLite dependency - the correct tradeoff for a distributable desktop app.
