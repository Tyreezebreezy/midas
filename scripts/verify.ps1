[CmdletBinding()]
param()

# Canonical entry point for the normal development gate (frontend check,
# Rust fmt/check/clippy/test/audit). Thin wrapper over the original
# validate.ps1 so historical phase records that cite `validate.ps1` by
# name stay accurate; new work should call this one.
& (Join-Path $PSScriptRoot 'validate.ps1') @args
exit $LASTEXITCODE
