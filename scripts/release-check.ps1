[CmdletBinding()]
param(
    [switch]$SkipSecurityScans
)

# Release-specific validation, on top of the normal development gate:
# clean tree, on main, in sync with origin, version consistency across
# every place a version is declared, full tests, and security scans.
# Does not build, tag, or publish anything.

$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
Push-Location $repository
try {
    Write-Host '--- Repository state ---'
    $branch = (git rev-parse --abbrev-ref HEAD).Trim()
    if ($branch -ne 'main') {
        throw "Not on main (currently on '$branch')."
    }

    $status = git status --porcelain
    if ($status) {
        throw "Working tree is not clean:`n$status"
    }

    $head = (git rev-parse HEAD).Trim()
    git fetch origin --quiet
    $originMain = (git rev-parse origin/main).Trim()
    if ($head -ne $originMain) {
        throw "HEAD ($head) does not match origin/main ($originMain). Push before releasing."
    }
    Write-Host "HEAD == origin/main == $head"

    Write-Host '--- Version consistency ---'
    $pkg = Get-Content -Raw 'package.json' | ConvertFrom-Json
    $cargoToml = Get-Content -Raw 'src-tauri/Cargo.toml'
    $cargoVersion = [regex]::Match($cargoToml, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
    $tauriConf = Get-Content -Raw 'src-tauri/tauri.conf.json' | ConvertFrom-Json
    $prodConf = Get-Content -Raw 'src-tauri/tauri.prod.conf.json' | ConvertFrom-Json

    $versions = [ordered]@{
        'package.json'          = $pkg.version
        'src-tauri/Cargo.toml'  = $cargoVersion
        'tauri.conf.json'       = $tauriConf.version
    }
    # @(...) forces array semantics - a bare `Select-Object -Unique` result
    # collapses to a plain string when there's exactly one distinct value,
    # and indexing a string with [0] returns its first *character*, not
    # the whole value (found live: this printed "Version aligned at 1").
    $distinct = @($versions.Values | Select-Object -Unique)
    if ($distinct.Count -ne 1) {
        $report = ($versions.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ', '
        throw "Version mismatch across manifests: $report"
    }
    $version = $distinct[0]
    Write-Host "Version aligned at $version across package.json / Cargo.toml / tauri.conf.json"

    if ($tauriConf.identifier -ne 'com.breezy.midas.dev') {
        throw "src-tauri/tauri.conf.json identifier must stay 'com.breezy.midas.dev' (found '$($tauriConf.identifier)'). The production identifier belongs only in tauri.prod.conf.json."
    }
    if ($prodConf.identifier -ne 'com.breezy.midas') {
        throw "src-tauri/tauri.prod.conf.json identifier must be 'com.breezy.midas' (found '$($prodConf.identifier)')."
    }
    Write-Host 'Dev/production identifier isolation confirmed.'

    Write-Host '--- Full development gate (includes financial-domain Rust tests and migration tests) ---'
    & (Join-Path $PSScriptRoot 'verify.ps1')
    if ($LASTEXITCODE -ne 0) { throw 'Development gate failed.' }

    if (-not $SkipSecurityScans) {
        Write-Host '--- Security scans ---'

        $gitleaks = Get-Command gitleaks -ErrorAction SilentlyContinue
        if (-not $gitleaks) {
            throw 'gitleaks is not on PATH. Install it or re-run with -SkipSecurityScans only if this was already run separately.'
        }
        & gitleaks detect --redact
        if ($LASTEXITCODE -ne 0) { throw 'Gitleaks found a potential secret.' }

        $trivy = Get-Command trivy -ErrorAction SilentlyContinue
        if (-not $trivy) {
            throw 'trivy is not on PATH. Install it or re-run with -SkipSecurityScans only if this was already run separately.'
        }
        & trivy fs --scanners vuln,secret --severity CRITICAL,HIGH --exit-code 1 --skip-version-check .
        if ($LASTEXITCODE -ne 0) { throw 'Trivy found a new CRITICAL or HIGH finding. Review before releasing.' }
    }
    else {
        Write-Host 'Security scans skipped by request (-SkipSecurityScans).'
    }

    Write-Host "MIDAS_RELEASE_CHECK=PASS ($version @ $head)"
}
finally {
    Pop-Location
}
