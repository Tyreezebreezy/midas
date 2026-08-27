# Phase 007: Release candidate

- Status: COMPLETE (verification and packaging work) — tagging and publishing are explicitly separate, owner-gated steps per `standards/RELEASE.md`, not yet taken
- Owner gate: APPROVED — a strictly release-focused mandate: freeze scope (no new finance features, only defects required for release readiness), version consistently, build and fresh-install the Windows installer, run a full functional smoke test through the _installed_ app rather than `tauri dev`, verify persistence and disaster recovery, measure the base64 backup/restore IPC approach at scale rather than speculate, verify installer/uninstall/reinstall behavior, run the final security and Ponytail gates, finalize release documentation, produce artifacts with SHA-256 checksums, and report a PASS/PARTIAL/BLOCKED verdict.

## Goal

Turn the Phase 6-hardened application into a real, installable Windows release candidate, and prove every claim about it - installer contents, data safety across install/uninstall, disaster recovery, and scale - against the actual installed binary, not against `tauri dev` or assumption.

## Scope

- **Freeze**: no source-code feature work. The only source-adjacent changes possible under this phase's mandate would be defects found during release verification - none were found that warranted a code change (see Findings below for the one real finding, which is an installer-behavior limitation, not an application defect).
- **Versioning**: confirmed `0.1.0` was already consistent across `Cargo.toml`, `package.json`, and `tauri.conf.json` before this phase started - no change needed.
- **Build**: `npm run tauri build` - a release-profile `midas.exe` and an NSIS installer, `MIDAS_0.1.0_x64-setup.exe`.
- **Fresh-install test, installed-app functional smoke test, persistence/relaunch, disaster recovery, larger backup test, installer/uninstall/reinstall behavior**: all performed against the real installed application over the WebView2 CDP debug port - the same live-verification discipline this project has used since Phase 0, applied here to the packaged artifact instead of the dev server. Full detail and every measured number: `docs/release/0.1.0.md`.
- **Final security gate**: `cargo fmt --check`, `cargo check --locked`, `cargo test --locked`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo audit`, `npm audit`, Gitleaks (history + working tree), Trivy, and a CSP/capabilities re-review.
- **Final Ponytail pass**: no release diff existed to review (this phase made no source changes), so the pass was a confirmatory re-run of Phase 6's repo-wide command/CSS/export cross-reference - still clean, nothing new to remove.
- **Release documentation**: `docs/release/0.1.0.md` (the project's own `templates/release/TEMPLATE.md`, filled in - checklist, artifact table, test results, known limitations, owner decisions required); a new "Installing" section in the README with hash-verification instructions and an explicit uninstall/data-safety statement; the "Known limitations" list updated with this phase's one real finding.
- **Artifacts**: `MIDAS_0.1.0_x64-setup.exe` (2,869,097 bytes) and a matching `.sha256` file beside it, hash cross-verified with two independent tools.

## Explicit non-goals

- Any new finance feature, UI surface, budgeting, reporting, agent integration, or bank sync - explicitly frozen by the owner's own framing.
- An architecture cleanup campaign - the owner explicitly scoped Ponytail to "only remove clearly unnecessary material," and this phase found none beyond Phase 6's already-complete audit.
- Fixing the silent-reinstall-over-existing-install hang with custom NSIS scripting - a real, verified, documented limitation, but implementing an `.onInit` silent-mode check is more installer-architecture work than "a defect required for release readiness" justifies in a scope-frozen phase; recorded for a future decision instead of built now.
- Creating or pushing the `v0.1.0` git tag, or publishing a Gitea release object - `standards/RELEASE.md` (this project's own pre-existing governance, written before this session) lists "obtain the owner release gate" as its own step, strictly before "create an annotated Git tag." This phase produces everything the gate needs to evaluate and stops there.

## Findings

One real, confirmed finding, from the installer/uninstall/reinstall testing (detail and reproduction in `docs/release/0.1.0.md`):

- **Silent reinstall over an existing installation hangs.** `MIDAS_0.1.0_x64-setup.exe /S` run while a previous installation is present does not complete - it waits indefinitely on a `MessageBox` (NSIS's default "already installed" prompt) that silent mode does not suppress. Confirmed by process inspection: the installer process remained alive with zero filesystem progress for 15+ seconds before being terminated. Scoped narrowly: a normal _interactive_ install shows the same dialog to a real user, who can click through it in a few seconds, and installing after a _full_ uninstall (also tested, separately) works correctly - only the silent-plus-already-installed combination hangs. Documented as a known limitation rather than fixed, per the non-goals above.

Everything else tested came back clean on the first or second attempt - including two of my own false alarms, both resolved by being more patient with NSIS's asynchronous self-delete mechanism rather than concluding "broken" from an impatient first check: the uninstaller's own executable and the install directory both took several seconds _after_ the visible process exited to actually disappear, which the test correctly waited out and re-verified rather than reporting as a failure.

## Verified current state

- Builds on Phase 6 (`6033e0a`) - no source changed.
- Version: `0.1.0` across Cargo.toml, package.json, tauri.conf.json (already consistent).
- 174 Rust tests (unchanged from Phase 6 - no source changed).
- Artifacts: `MIDAS_0.1.0_x64-setup.exe`, SHA-256 `8755efce00b6efdeb3a1a8241f29c4c56de563a7bef6dca5a5a8066fb8553547`, cross-verified with `Get-FileHash` and `sha256sum` independently.

## Acceptance criteria (owner's thirteen numbered items)

1. **Freeze scope** - [x] no finance feature work; the one finding is documented, not built around.
2. **Version the release candidate** - [x] already consistent everywhere; no UI/installer version display exists to separately check (the app has never had an About screen).
3. **Fresh-install test** - [x] see `docs/release/0.1.0.md` - built, installed onto a genuinely emptied app-data state, launched the installed binary, migrations verified.
4. **Installed-app functional smoke test** - [x] all twelve named workflows exercised through the installed app's real IPC, every number hand-verified.
5. **Persistence/relaunch** - [x] full close, reopen, every seeded value matched exactly.
6. **Backup disaster-recovery test** - [x] backup preserved externally, live database actually removed (not just archived), restored, every critical value matched exactly.
7. **Larger backup test** - [x] 3,000 transactions seeded through real IPC; backup/preview/restore timings measured (45 ms / 29 ms / 36 ms), not speculated.
8. **Installer/update behavior** - [x] install path and shortcuts verified; uninstall verified to preserve data (checksum-identical) while removing the app; clean reinstall verified; silent reinstall-over-existing found to hang and documented (see Findings).
9. **Final security gate** - [x] all eight named checks clean.
10. **Final Ponytail pass** - [x] confirmatory re-check, nothing new (no release diff existed).
11. **Release documentation** - [x] `docs/release/0.1.0.md` plus README's new "Installing" section and updated known limitations.
12. **Artifacts** - [x] installer, versioned filename, SHA-256, checksum file beside it.
13. **Git/Gitea** - [x] this checkpoint commit will be pushed and `HEAD == origin/main` verified; tagging and any Gitea release publication explicitly held for the owner release gate per `standards/RELEASE.md`.

## Test strategy

Every claim in `docs/release/0.1.0.md` was verified against the real installed application over the WebView2 CDP debug port - account balances, transaction counts, budget actuals, goal statuses, recurring-series confirmation, pay schedules, and net worth were all independently hand-computed and matched against what the app reported, the same discipline this project has applied since Phase 0. File-level claims (uninstall preserving data, artifact identity) were verified with checksums from independent tools, not trusted from a single source.

## Owner decisions

Per `standards/RELEASE.md`, all deferred to the owner release gate, presented alongside this spec and `docs/release/0.1.0.md`:

1. Accept, reject, or request changes to this release candidate.
2. Accept the silent-reinstall-over-existing limitation as documented, or request it be fixed before release.
3. Authorize creating and pushing the annotated `v0.1.0` tag.
4. Authorize (or decline) publishing a Gitea release object.

## Hard-stop conditions

- Any finding requiring a source-code fix beyond what a scope-frozen release phase should do - none arose; the one finding is an NSIS installer behavior, not an application defect.
- Any security or data-safety finding - none arose; uninstall was specifically verified _not_ to touch user data, checksum-confirmed rather than assumed.
- Tagging, pushing a tag, or publishing without the owner release gate - not done, per this project's own pre-existing `standards/RELEASE.md`.
