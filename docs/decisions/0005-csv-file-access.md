# ADR-0005: CSV file access via native file input, not a Tauri dialog/fs plugin

- Status: accepted
- Date: 2026-08-25
- Supersedes: none

## Context and problem

CSV import is the first MIDAS feature needing the user to hand the app a file from disk. The owner directed inspecting the Forge's current Tauri capability conventions before granting any new file authority, and choosing the narrowest mechanism that lets MIDAS access a CSV _because the user explicitly selected it_ - never arbitrary or background filesystem access.

## Options considered

1. `tauri-plugin-dialog` (native "open file" picker, returns a path) + a custom narrow Rust command (`read_csv_file(path)`) that reads that path with an extension/size check. Requires one new capability permission (`dialog:allow-open`).
2. `tauri-plugin-dialog` + `tauri-plugin-fs`, relying on Tauri 2's dialog-selection-grants-temporary-fs-scope interop so the frontend can call the fs plugin's `readTextFile` on exactly the selected path. Requires two new capability permissions and a scoping behavior that is not something I could verify precisely enough to be confident it's configured correctly rather than more permissive than intended.
3. A plain HTML `<input type="file" accept=".csv">` element. WebView2 (Tauri's Windows backend) is Chromium-based and shows the real native OS file picker for this, exactly like any browser. The selected `File` object's name and text content (`file.name`, `await file.text()`) are read entirely through the standard Web File API, mediated by the webview itself - no Tauri IPC call, no Rust filesystem access, and no capability/permission entry of any kind.

## Decision

Option 3. `src/CsvImportWizard.tsx` uses a native file input; the browser handles the OS picker and the read. The parsed file's text content (and filename) are passed to a Tauri command (`stage_import`) purely for **parsing**, not for file access - Rust never touches the filesystem for CSV import at all in Phase 2. `src-tauri/capabilities/default.json` gains **zero** new permission entries.

## Rationale

- **Established precedent in this exact ecosystem**: SIREN (`C:\Dev\siren`) already reads user-selected TXT/MD/PDF documents and has an empty `capabilities/default.json` (`"permissions": []`) and no `tauri-plugin-dialog`/`tauri-plugin-fs` dependency in its `Cargo.toml` - confirming file selection in a Forge-derived app is not expected to require a Tauri plugin at all.
- **Narrower than the requested framing, in the requester's own favor**: the instruction asked for "the narrowest file-dialog/file-read permission necessary." The narrowest possible permission is _no permission_ - Option 3 achieves the identical user experience (native OS picker, user-initiated, one file at a time) with strictly less native authority than Options 1 or 2, satisfying the goal more completely than either.
- **Option 1** is safe and well-understood but still adds a real capability entry and a custom command whose `path` argument is, at the IPC boundary, just a string - reachable in principle by any frontend code, not only the picker flow (mitigated in Option 1 by an extension/size check, but that mitigation is unnecessary if Option 3 removes the native read surface entirely).
- **Option 2** relies on a specific, less-familiar plugin-interop scoping behavior (dialog selection auto-granting temporary fs scope) that I could not verify precisely enough from documentation-independent knowledge alone to be confident the permission grant is exactly as narrow as intended rather than accidentally broader - not a risk worth taking when Option 3 sidesteps it completely.
- CSP impact: none. The File API operates on an already-selected in-memory `File`/`Blob`; it is not a network request and is unaffected by `connect-src`/`script-src` or any other directive in MIDAS's existing narrow CSP.

## What this grants

- The ability for the user to pick one local file via the OS's native file picker and have its filename and text content read into the webview - exactly the same authority any website has always had via `<input type="file">`, nothing more.

## What this does not grant

- No path-based file access from Rust or JS. MIDAS cannot read a file the user did not explicitly pick through this input.
- No directory listing, no background/automatic scanning, no write access, no access to any file the dialog wasn't used to select.
- No new Tauri capability, permission, or plugin surface whatsoever.

## Consequences and accepted tradeoffs

- Rust-side `stage_import` still validates the incoming CSV text defensively (row/column sanity, size ceiling) since it is untrusted input regardless of how it arrived - this is ordinary input validation, not a capability concern.
- If a future phase needs MIDAS to read a file _without_ the user picking it in the moment (e.g., a saved "watch this folder" import), that is new authority requiring its own ADR and owner approval - not implied by this one.
