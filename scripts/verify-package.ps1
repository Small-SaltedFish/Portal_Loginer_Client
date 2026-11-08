$ErrorActionPreference='Stop'
$taskRoot=Split-Path -Parent $PSScriptRoot
Set-Location $taskRoot
foreach($taskName in @('portal-service.exe','portal-settings.exe','portal-tray.exe')) {
 $taskBytes=[IO.File]::ReadAllBytes((Join-Path $taskRoot "target/release/$taskName"))
 $taskPe=[BitConverter]::ToInt32($taskBytes,0x3c)
 $taskSubsystem=[BitConverter]::ToUInt16($taskBytes,$taskPe+92)
 if($taskSubsystem -ne 2){throw "$taskName creates a console"}
 Write-Output "$taskName subsystem=WindowsGUI"
 $taskHash=(Get-FileHash "target/release/$taskName").Hash
 if($taskHash -ne (Get-FileHash "dist/windows-x86_64-dev/$taskName").Hash){throw "Stale staging file: $taskName"}
}
$taskAudit=Join-Path $taskRoot 'target/installer-audit-0.2.6'
& 7z.exe x '-y' "-o$taskAudit" 'dist/PortalLoginer-0.2.6-Setup.exe' | Out-Null
if($LASTEXITCODE -ne 0){throw 'Installer extraction failed'}
foreach($taskName in @('portal-service.exe','portal-settings.exe','portal-tray.exe')) {
 $taskExpected=(Get-FileHash "target/release/$taskName").Hash
 $taskMatches=@(Get-ChildItem $taskAudit -Recurse -File -Filter $taskName | Where-Object{(Get-FileHash $_.FullName).Hash -eq $taskExpected})
 if($taskMatches.Count -eq 0){throw "Installer mismatch $taskName"}
 Write-Output "$taskName installer SHA256 matched"
}
Add-Type -AssemblyName System.IO.Compression.FileSystem
$taskZip=[IO.Compression.ZipFile]::OpenRead((Join-Path $taskRoot 'dist/PortalLoginer-0.2.6-windows-x86_64-dev.zip'))
try {
 foreach($taskEntry in $taskZip.Entries){
  $taskSource=Join-Path $taskRoot "dist/windows-x86_64-dev/$($taskEntry.FullName)"
  $taskStream=$taskEntry.Open();try{$taskSha=[Security.Cryptography.SHA256]::Create();try{$taskActual=[Convert]::ToHexString($taskSha.ComputeHash($taskStream))}finally{$taskSha.Dispose()}}finally{$taskStream.Dispose()}
  if($taskActual -ne (Get-FileHash $taskSource).Hash){throw "ZIP mismatch $($taskEntry.FullName)"}
 }
 Write-Output "ZIP entries matched: $($taskZip.Entries.Count)"
}finally{$taskZip.Dispose()}
$taskExpectedSetup=(Get-Content 'dist/PortalLoginer-0.2.6-Setup.exe.sha256').Split(' ')[0]
if($taskExpectedSetup -ne (Get-FileHash 'dist/PortalLoginer-0.2.6-Setup.exe').Hash){throw 'Setup checksum mismatch'}
Write-Output 'Setup checksum matched'
# Verify cleanup code is usable even when all installed application files are gone.
foreach($taskPair in @(@('dist/PortalLoginer-0.2.6-Cleanup.exe','target/cleanup-audit-0.2.6'),@('target/installer-audit-0.2.6/Uninstall.exe','target/uninstaller-audit-0.2.6'))) {
 $taskEmbeddedAudit=Join-Path $taskRoot $taskPair[1]
 & 7z.exe x '-y' "-o$taskEmbeddedAudit" (Join-Path $taskRoot $taskPair[0]) | Out-Null
 if($LASTEXITCODE -ne 0){throw 'Embedded cleanup extraction failed'}
 $taskEmbedded=Get-ChildItem $taskEmbeddedAudit -Recurse -File -Filter portal-uninstall-helper.exe | Select-Object -First 1
 if(-not $taskEmbedded){throw 'Self-contained cleanup helper missing'}
 if((Get-FileHash $taskEmbedded.FullName).Hash -ne (Get-FileHash (Join-Path $taskRoot 'target/release/portal-service.exe')).Hash){throw 'Embedded cleanup helper mismatch'}
 Write-Output "$($taskPair[0]) contains matching cleanup helper"
}
$taskExpectedCleanup=(Get-Content (Join-Path $taskRoot 'dist/PortalLoginer-0.2.6-Cleanup.exe.sha256')).Split(' ')[0]
if($taskExpectedCleanup -ne (Get-FileHash (Join-Path $taskRoot 'dist/PortalLoginer-0.2.6-Cleanup.exe')).Hash){throw 'Cleanup checksum mismatch'}
