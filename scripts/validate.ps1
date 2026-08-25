[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot

$required = @(
    'AGENTS.md',
    'PROJECT_STATE.md',
    'constitution\MIDAS_CONSTITUTION.md',
    'standards\SECURITY.md',
    'standards\TESTING.md',
    'standards\RELEASE.md',
    'standards\DEPENDENCIES.md',
    'standards\ACCESSIBILITY.md',
    'docs\product\MIDAS_PRODUCT_CONTEXT_HANDOFF.md',
    'docs\product\MIDAS_CLAUDE_CODE_BUILD_HANDOFF.md',
    'package-lock.json'
)

$missing = $required | Where-Object { -not (Test-Path (Join-Path $repository $_)) }
if ($missing) {
    throw "Missing required paths: $($missing -join ', ')"
}

$tauri = Get-Content -Raw (Join-Path $repository 'src-tauri\tauri.conf.json') | ConvertFrom-Json
if (-not $tauri.app.security.csp) {
    throw 'Tauri production CSP must not be null or empty.'
}

if (-not (Test-Path (Join-Path $repository 'node_modules'))) {
    throw 'Dependencies are absent. Run npm.cmd install first.'
}

Push-Location $repository
try {
    & npm.cmd run check
    if ($LASTEXITCODE -ne 0) { throw 'Frontend validation failed.' }

    & cargo fmt --manifest-path 'src-tauri\Cargo.toml' -- --check
    if ($LASTEXITCODE -ne 0) { throw 'Rust formatting check failed.' }

    & cargo check --locked --manifest-path 'src-tauri\Cargo.toml'
    if ($LASTEXITCODE -ne 0) { throw 'Rust build check failed.' }

    & cargo audit --file 'src-tauri\Cargo.lock'
    if ($LASTEXITCODE -ne 0) { throw 'cargo-audit found an unresolved advisory.' }
}
finally {
    Pop-Location
}

Write-Host 'MIDAS_VALIDATION=PASS'
