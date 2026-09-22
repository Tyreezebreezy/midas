[CmdletBinding()]
param(
    [int]$TimeoutSeconds = 90
)

# Lightweight native smoke check against the disposable test identity
# (com.breezy.midas.test) - never production, never the day-to-day dev
# identity. Launches `npm run tauri:test`, waits for the app to respond
# over CDP, runs scripts/smoke-check.mjs, then tears everything down and
# deletes the disposable test app-data. Safe to run repeatedly.

$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
$port = 9222

function Remove-TestIdentityData {
    $paths = @(
        (Join-Path $env:APPDATA 'com.breezy.midas.test'),
        (Join-Path $env:LOCALAPPDATA 'com.breezy.midas.test')
    )
    foreach ($p in $paths) {
        if (Test-Path $p) { Remove-Item -Recurse -Force $p -ErrorAction SilentlyContinue }
    }
}

function Stop-TestInstance {
    # Killing midas.exe brings down the whole `tauri dev` supervision
    # chain (node -> cargo -> cargo -> midas.exe) beneath it.
    cmd /c "taskkill /F /IM midas.exe /T >nul 2>&1"
}

Push-Location $repository
try {
    Write-Host '--- Starting disposable test-identity instance ---'
    Stop-TestInstance
    Remove-TestIdentityData

    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$port"
    $proc = Start-Process -FilePath 'cmd.exe' -ArgumentList '/c', 'npm.cmd run tauri:test' `
        -WorkingDirectory $repository -WindowStyle Hidden -PassThru

    Write-Host "Waiting up to $TimeoutSeconds s for CDP on port $port..."
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $ready = $false
    while ((Get-Date) -lt $deadline) {
        try {
            $resp = Invoke-RestMethod -Uri "http://127.0.0.1:$port/json" -TimeoutSec 2 -ErrorAction Stop
            if ($resp) { $ready = $true; break }
        }
        catch { Start-Sleep -Milliseconds 1000 }
    }
    if (-not $ready) {
        throw "Test instance never became CDP-ready within $TimeoutSeconds s."
    }
    Write-Host 'CDP ready.'

    Write-Host '--- Running smoke-check.mjs ---'
    & node (Join-Path $PSScriptRoot 'smoke-check.mjs') --port $port
    $smokeExit = $LASTEXITCODE
    if ($smokeExit -ne 0) { throw 'smoke-check.mjs reported a failure.' }

    Write-Host 'MIDAS_SMOKE=PASS'
}
finally {
    Write-Host '--- Tearing down test instance and disposable data ---'
    Stop-TestInstance
    Start-Sleep -Milliseconds 500
    Remove-TestIdentityData
    Remove-Item Env:\WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -ErrorAction SilentlyContinue
    Pop-Location
}
