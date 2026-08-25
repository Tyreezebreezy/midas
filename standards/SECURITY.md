# Security standard

Use a practical NIST SSDF-inspired baseline proportionate to project risk. MIDAS handles private financial data, so treat filesystem and persistence review as seriously as network review even though MIDAS is offline by default.

## Required review

- Inventory direct and transitive dependencies; review lockfile changes and licenses.
- Run secret and vulnerability scans (Gitleaks, `cargo audit`/`npm audit`, Trivy); interpret results against the shipped dependency graph.
- Review native/API command allowlists, filesystem scopes, network endpoints, secrets, logs, temporary files, caches, CSP/browser policy, installer contents, and artifact hashes.
- Document real limitations. Security exceptions require explicit owner acceptance and may not be hidden as test waivers.

## Authority rules

- Default to no network, no broad filesystem access, no shell execution, and no remote webview content.
- Grant a Tauri capability only to the exact window, command, platform, and scope that needs it. Document why in an ADR when a new capability is added (e.g. file dialog for CSV import, scoped fs access for backup/export).
- Never log secrets or sensitive user data. Do not commit `.env` files, credentials, tokens, private keys, real bank statements, or real personal financial data.
- Review third-party workflows, presets, extensions, and scripts before execution. A declared requirement is not a runtime sandbox.

## MIDAS-specific rules

- No analytics or telemetry unless the owner explicitly approves it.
- No outbound network call during ordinary local operation (import, review, budgeting, dashboard). If a future feature needs one, it is a new capability requiring an ADR and owner approval.
- SQLite database and imported source files stay under the user's local app data directory. Never write financial data outside the app's own scoped storage.
- CSV/import parsing is defensive: malformed rows go to Needs Review, never silently dropped or silently guessed into the ledger.

## Evidence

Record scanner name/version, command, scope, timestamp, exit status, findings, dispositions, and any owner acceptance in the active phase QA record (`templates/qa/TEMPLATE.md`).
