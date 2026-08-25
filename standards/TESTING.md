# Testing standard

Tests prove approved behavior; they do not justify unrequested features.

## Minimum gates

1. Static checks: parse configuration, type-check frontend code, and format-check changed languages.
2. Deterministic unit checks for non-trivial branches, parsers, security logic, and money/data-loss paths.
3. Build checks for every shipped target affected by the phase.
4. Rendered UI checks for page identity, meaningful content, console health, visible focus, the primary interaction, and desktop plus one narrow viewport.
5. Regression after Ponytail and security changes.

Record the exact command and result. "Looks good" is not evidence. A skipped check is a limitation, not a pass.

## Financial logic tests are mandatory

Once the corresponding feature exists, cover at minimum:

- transfer exclusion from spending totals
- credit-card payment double-counting prevention
- refund/reimbursement handling
- duplicate import detection
- Safe-to-Spend calculation edge cases, including missing/incomplete inputs
- bills crossing payday boundaries
- recurring amount changes and cadence detection
- budget planned vs. actual
- account inclusion/exclusion from net worth
- import rollback on failure

Use realistic fixture data. Never use real personal bank statements, account numbers, or credentials in the repository or in tests.
