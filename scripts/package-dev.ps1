param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
if (-not $SkipBuild) { & (Join-Path $PSScriptRoot 'dev.ps1') build }
& (Join-Path $PSScriptRoot 'package-installer.ps1') -SkipBuild
$taskOutput = Join-Path $taskRoot 'dist/windows-x86_64-dev'
New-Item -ItemType Directory -Path $taskOutput -Force | Out-Null
$taskSources = [ordered]@{
    'portal-service.exe' = 'target/release/portal-service.exe'
    'portal-tray.exe' = 'target/release/portal-tray.exe'
    'portal-settings.exe' = 'target/release/portal-settings.exe'
    'PortalLoginer-0.2.10-Setup.exe' = 'dist/PortalLoginer-0.2.10-Setup.exe'
    'PortalLoginer-0.2.10-Cleanup.exe' = 'dist/PortalLoginer-0.2.10-Cleanup.exe'
    'DESKTOP.md' = 'docs/DESKTOP.md'
    'WINDOWS.md' = 'docs/WINDOWS.md'
    'BASELINE-RESTORATION.md' = 'docs/BASELINE-RESTORATION.md'
    'LICENSE' = 'LICENSE'
    'NOTICE' = 'NOTICE'
    'THIRD-PARTY-NOTICES.txt' = 'THIRD-PARTY-NOTICES.txt'
}
foreach ($taskBinary in @('portal-service.exe', 'portal-tray.exe', 'portal-settings.exe')) {
    $taskInputs = @(Get-ChildItem (Join-Path $taskRoot 'crates/portal-core'), (Join-Path $taskRoot 'crates/portal-service') -Recurse -File |
        Where-Object { $_.Extension -eq '.rs' -or $_.Name -eq 'Cargo.toml' })
    if ($taskBinary -ne 'portal-service.exe') {
        $taskDesktopInputs = @('Cargo.toml', 'src/lib.rs', 'src/model.rs', 'src/client.rs', 'src/platform.rs', 'src/notifications.rs')
        if ($taskBinary -eq 'portal-tray.exe') { $taskDesktopInputs += @('src/tray.rs', 'src/bin/tray.rs') }
        else { $taskDesktopInputs += @('src/settings.rs', 'src/settings_view.rs', 'src/bin/settings.rs') }
        $taskInputs += @($taskDesktopInputs | ForEach-Object { Get-Item -LiteralPath (Join-Path $taskRoot "crates/portal-desktop/$_") })
    }
    $taskNewestSource = $taskInputs | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    $taskBuilt = Get-Item -LiteralPath (Join-Path $taskRoot "target/release/$taskBinary")
    if ($taskBuilt.LastWriteTime -lt $taskNewestSource.LastWriteTime) {
        throw "Build the current workspace before packaging: $taskBinary"
    }
}
foreach ($taskEntry in $taskSources.GetEnumerator()) {
    Copy-Item -LiteralPath (Join-Path $taskRoot $taskEntry.Value) -Destination (Join-Path $taskOutput $taskEntry.Key) -Force
}
$taskChecksums = foreach ($taskName in $taskSources.Keys) {
    $taskHash = Get-FileHash -LiteralPath (Join-Path $taskOutput $taskName) -Algorithm SHA256
    '{0}  {1}' -f $taskHash.Hash.ToLowerInvariant(), $taskName
}
$taskChecksums | Set-Content -LiteralPath (Join-Path $taskOutput 'SHA256SUMS.txt') -Encoding ascii
$taskFiles = @($taskSources.Keys) + @('SHA256SUMS.txt') | ForEach-Object { Join-Path $taskOutput $_ }
$taskArchive = Join-Path $taskRoot 'dist/PortalLoginer-0.2.10-windows-x86_64-dev.zip'
Compress-Archive -LiteralPath $taskFiles -DestinationPath $taskArchive -Force
Get-Item -LiteralPath $taskArchive | Select-Object FullName, Length
