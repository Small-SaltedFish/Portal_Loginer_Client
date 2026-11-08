param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
if (-not $SkipBuild) { & (Join-Path $PSScriptRoot 'dev.ps1') build }
$taskCompiler = Join-Path $taskRoot '.tools/nsis-download/nsis-3.11/makensis.exe'
if (-not (Test-Path -LiteralPath $taskCompiler)) { $taskCompiler = (Get-Command makensis -ErrorAction Stop).Source }
New-Item -ItemType Directory -Force (Join-Path $taskRoot 'dist') | Out-Null
& $taskCompiler /INPUTCHARSET UTF8 /V3 (Join-Path $taskRoot 'installer/windows.nsi')
if ($LASTEXITCODE -ne 0) { throw 'Windows installer build failed' }
Get-Item -LiteralPath (Join-Path $taskRoot 'dist/PortalLoginer-0.2.8-Setup.exe') | Select-Object FullName, Length
$taskPackage = Join-Path $taskRoot 'dist/PortalLoginer-0.2.8-Setup.exe'
$taskHash = (Get-FileHash -LiteralPath $taskPackage -Algorithm SHA256).Hash.ToLowerInvariant()
"$taskHash  PortalLoginer-0.2.8-Setup.exe" | Set-Content -LiteralPath "$taskPackage.sha256" -Encoding ascii

& $taskCompiler /INPUTCHARSET UTF8 /V3 (Join-Path $taskRoot 'installer/cleanup.nsi')
if ($LASTEXITCODE -ne 0) { throw 'Cleanup program build failed' }
$taskCleanup=Join-Path $taskRoot 'dist/PortalLoginer-0.2.8-Cleanup.exe'
$taskCleanupHash=(Get-FileHash -LiteralPath $taskCleanup -Algorithm SHA256).Hash.ToLowerInvariant()
"$taskCleanupHash  PortalLoginer-0.2.8-Cleanup.exe" | Set-Content -LiteralPath "$taskCleanup.sha256" -Encoding ascii
